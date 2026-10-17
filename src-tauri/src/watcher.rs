//! Watches open documents and the local files their previews show (images etc.).
//! Changes are sent to the windows showing the affected document as `doc-changed` / `dep-changed`.
//!
//! Locking: notify's Windows backend runs our event callback on the same thread that services
//! `watch()`/`unwatch()`, and `watch()` blocks until that thread answers. So the callback only takes
//! `state` briefly, and `watch()`/`unwatch()` are never called while `state` is held. Lock order
//! everywhere else is `watcher` then `state`.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
struct State {
    /// document key -> (document path, window labels showing it)
    docs: HashMap<String, (PathBuf, HashSet<String>)>,
    /// dependency key -> document keys that use it
    deps: HashMap<String, HashSet<String>>,
    /// directory key -> (directory, number of docs/deps inside it)
    dirs: HashMap<String, (PathBuf, usize)>,
}

pub struct DocWatcher {
    state: Mutex<State>,
    watcher: Mutex<Option<RecommendedWatcher>>,
}

#[derive(Clone, Serialize)]
struct ChangePayload {
    path: String,
}

/// Directories to start or stop watching, decided under the state lock and applied after it.
#[derive(Default)]
struct DirChanges {
    watch: Vec<PathBuf>,
    unwatch: Vec<PathBuf>,
}

/// Case- and separator-insensitive key (notify reports `\`; paths from elsewhere may use `/`).
fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase().replace('/', "\\")
}

impl DocWatcher {
    pub fn new(app: AppHandle) -> Self {
        let handle = app.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            if let Some(w) = handle.try_state::<DocWatcher>() {
                for path in &event.paths {
                    w.on_change(&handle, path);
                }
            }
        })
        .ok();
        Self { state: Mutex::new(State::default()), watcher: Mutex::new(watcher) }
    }

    fn on_change(&self, app: &AppHandle, path: &Path) {
        let mut emits: Vec<(String, &'static str, ChangePayload)> = Vec::new();
        {
            let state = self.state.lock().unwrap();
            let k = key(path);
            if let Some((doc_path, labels)) = state.docs.get(&k) {
                let payload = ChangePayload { path: doc_path.to_string_lossy().into() };
                emits.extend(labels.iter().map(|l| (l.clone(), "doc-changed", payload.clone())));
            }
            if let Some(docs) = state.deps.get(&k) {
                for doc in docs {
                    if let Some((doc_path, labels)) = state.docs.get(doc) {
                        let payload = ChangePayload { path: doc_path.to_string_lossy().into() };
                        emits.extend(labels.iter().map(|l| (l.clone(), "dep-changed", payload.clone())));
                    }
                }
            }
        }
        for (label, event, payload) in emits {
            let _ = app.emit_to(label.as_str(), event, payload);
        }
    }

    /// Replaces the set of documents shown by a window.
    pub fn set_window_docs(&self, label: &str, paths: &[PathBuf]) {
        let mut watcher = self.watcher.lock().unwrap();
        let mut changes = DirChanges::default();
        {
            let mut state = self.state.lock().unwrap();
            let wanted: HashSet<String> = paths.iter().map(|p| key(p)).collect();

            // Remove this window from documents it no longer shows.
            let mut dropped = Vec::new();
            for (k, (_, labels)) in state.docs.iter_mut() {
                if !wanted.contains(k) && labels.remove(label) && labels.is_empty() {
                    dropped.push(k.clone());
                }
            }
            for k in dropped {
                if let Some((p, _)) = state.docs.remove(&k) {
                    release_dir(&mut state, &p, &mut changes);
                }
                let orphaned: Vec<String> = state
                    .deps
                    .iter_mut()
                    .filter_map(|(dep, docs)| (docs.remove(&k) && docs.is_empty()).then(|| dep.clone()))
                    .collect();
                for dep in orphaned {
                    state.deps.remove(&dep);
                    release_dir(&mut state, Path::new(&dep), &mut changes);
                }
            }

            for p in paths {
                let k = key(p);
                let added = !state.docs.contains_key(&k);
                state.docs.entry(k).or_insert_with(|| (p.clone(), HashSet::new())).1.insert(label.to_string());
                if added {
                    retain_dir(&mut state, p, &mut changes);
                }
            }
        }
        apply(&mut watcher, changes);
    }

    /// Records that `doc` shows the local file `dep`.
    pub fn add_dependency(&self, doc: &Path, dep: &Path) {
        let mut watcher = self.watcher.lock().unwrap();
        let mut changes = DirChanges::default();
        {
            let mut state = self.state.lock().unwrap();
            let doc_key = key(doc);
            let dep_key = key(dep);
            if !state.docs.contains_key(&doc_key) || dep_key == doc_key {
                return;
            }
            let is_new = !state.deps.contains_key(&dep_key);
            state.deps.entry(dep_key).or_default().insert(doc_key);
            if is_new {
                retain_dir(&mut state, dep, &mut changes);
            }
        }
        apply(&mut watcher, changes);
    }
}

fn retain_dir(state: &mut State, file: &Path, changes: &mut DirChanges) {
    let Some(dir) = file.parent() else { return };
    let entry = state.dirs.entry(key(dir)).or_insert_with(|| (dir.to_path_buf(), 0));
    entry.1 += 1;
    if entry.1 == 1 {
        changes.watch.push(dir.to_path_buf());
    }
}

fn release_dir(state: &mut State, file: &Path, changes: &mut DirChanges) {
    let Some(dir) = file.parent() else { return };
    let k = key(dir);
    let remove = match state.dirs.get_mut(&k) {
        Some(entry) => {
            entry.1 = entry.1.saturating_sub(1);
            entry.1 == 0
        }
        None => false,
    };
    if remove {
        state.dirs.remove(&k);
        changes.unwatch.push(dir.to_path_buf());
    }
}

/// Runs the queued watch/unwatch calls. Called with the `watcher` lock held but not the `state` lock.
fn apply(watcher: &mut Option<RecommendedWatcher>, changes: DirChanges) {
    let Some(w) = watcher.as_mut() else { return };
    for dir in changes.unwatch {
        let _ = w.unwatch(&dir);
    }
    for dir in changes.watch {
        let _ = w.watch(&dir, RecursiveMode::NonRecursive);
    }
}
