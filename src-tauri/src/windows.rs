//! Decides where documents open. By default every document gets its own window; with "Open files in
//! tabs" on, documents open as tabs in the most recently used window. All windows live in one process
//! (a second launch hands its files to this one).

use crate::settings::{Placement, SettingsStore};
use crate::watcher::DocWatcher;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Theme, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OpenRequest {
    Open { path: String, fragment: Option<String> },
    New,
}

#[derive(Default)]
struct Entry {
    docs: Vec<String>,
    pending: Vec<OpenRequest>,
    ready: bool,
}

#[derive(Default)]
struct Registry {
    windows: HashMap<String, Entry>,
    order: Vec<String>, // most recently focused last
    counter: u32,
}

#[derive(Default)]
pub struct WindowRegistry(Mutex<Registry>);

pub fn window_theme(setting: &str) -> Option<Theme> {
    match setting {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    }
}

pub fn create_window(app: &AppHandle, cascade_from: Option<&str>, initial: Vec<OpenRequest>) -> tauri::Result<String> {
    let registry = app.state::<WindowRegistry>();
    let label = {
        let mut r = registry.0.lock().unwrap();
        r.counter += 1;
        let label = format!("w{}", r.counter);
        r.windows.insert(label.clone(), Entry { pending: initial, ..Default::default() });
        r.order.push(label.clone());
        label
    };

    let settings = app.state::<SettingsStore>().get();
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title("MarkView")
        .inner_size(1280.0, 860.0)
        .min_inner_size(560.0, 360.0)
        .visible(false)
        .transparent(true)
        .theme(window_theme(&settings.theme));
    if let Some(dir) = crate::package::webview_data_dir() {
        builder = builder.data_directory(dir);
    }
    let window = builder.build()?;

    place_window(app, &window, cascade_from, settings.window);

    let dark = crate::server::effective_theme(&settings.theme) == "dark";
    #[cfg(windows)]
    let _ = window_vibrancy::apply_mica(&window, Some(dark));

    // Shown when the page reports ready (avoids a blank flash); fallback in case it never does.
    let fallback = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let _ = fallback.show();
    });

    let app_handle = app.clone();
    let l = label.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            // The page decides (it may need to ask about unsaved changes) and then calls close_window.
            api.prevent_close();
            let _ = app_handle.emit_to(l.as_str(), "close-requested", ());
        }
        WindowEvent::Focused(true) => {
            let mut r = app_handle.state::<WindowRegistry>().inner().0.lock().unwrap();
            r.order.retain(|x| x != &l);
            r.order.push(l.clone());
        }
        WindowEvent::Destroyed => {
            {
                let mut r = app_handle.state::<WindowRegistry>().inner().0.lock().unwrap();
                r.windows.remove(&l);
                r.order.retain(|x| x != &l);
            }
            app_handle.state::<DocWatcher>().set_window_docs(&l, &[]);
        }
        _ => {}
    });

    Ok(label)
}

fn place_window(app: &AppHandle, window: &WebviewWindow, cascade_from: Option<&str>, saved: Option<Placement>) {
    if let Some(from) = cascade_from.and_then(|l| app.get_webview_window(l)) {
        if !from.is_maximized().unwrap_or(false) {
            if let (Ok(pos), Ok(size)) = (from.outer_position(), from.outer_size()) {
                let scale = from.scale_factor().unwrap_or(1.0);
                let offset = (32.0 * scale) as i32;
                let _ = window.set_size(size);
                let mut next = PhysicalPosition::new(pos.x + offset, pos.y + offset);
                if let Ok(Some(monitor)) = from.current_monitor() {
                    let area = monitor.work_area();
                    if next.x + size.width as i32 > area.position.x + area.size.width as i32
                        || next.y + size.height as i32 > area.position.y + area.size.height as i32
                    {
                        next = PhysicalPosition::new(area.position.x + offset, area.position.y + offset);
                    }
                }
                let _ = window.set_position(next);
                return;
            }
        }
    }

    match saved {
        Some(p) if on_screen(window, &p) => {
            let _ = window.set_size(PhysicalSize::new(p.width.max(560), p.height.max(360)));
            let _ = window.set_position(PhysicalPosition::new(p.x, p.y));
            if p.maximized {
                let _ = window.maximize();
            }
        }
        _ => {
            let _ = window.center();
        }
    }
}

fn on_screen(window: &WebviewWindow, p: &Placement) -> bool {
    window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .any(|m| {
            let (pos, size) = (m.position(), m.size());
            let cx = p.x + 60;
            let cy = p.y + 20;
            cx >= pos.x && cy >= pos.y && cx < pos.x + size.width as i32 && cy < pos.y + size.height as i32
        })
}

/// Called by a window's page once it can receive events; returns what it should open.
pub fn window_ready(app: &AppHandle, label: &str) -> Vec<OpenRequest> {
    let pending = {
        let mut r = app.state::<WindowRegistry>().inner().0.lock().unwrap();
        match r.windows.get_mut(label) {
            Some(e) => {
                e.ready = true;
                std::mem::take(&mut e.pending)
            }
            None => Vec::new(),
        }
    };
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
    }
    pending
}

pub fn set_docs(app: &AppHandle, label: &str, docs: Vec<String>) {
    let paths: Vec<PathBuf> = docs.iter().map(PathBuf::from).collect();
    app.state::<DocWatcher>().set_window_docs(label, &paths);
    let mut r = app.state::<WindowRegistry>().inner().0.lock().unwrap();
    if let Some(e) = r.windows.get_mut(label) {
        e.docs = docs;
    }
}

fn deliver(app: &AppHandle, label: &str, request: OpenRequest) {
    let ready = {
        let mut r = app.state::<WindowRegistry>().inner().0.lock().unwrap();
        match r.windows.get_mut(label) {
            Some(e) if e.ready => true,
            Some(e) => {
                e.pending.push(request.clone());
                false
            }
            None => false,
        }
    };
    if ready {
        let _ = app.emit_to(label, "open-request", request);
    }
}

pub fn focus(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Window to open a new document in: the source (or most recent) window if tabs are on or it is empty,
/// otherwise a new window.
fn target_window(app: &AppHandle, source: Option<&str>) -> Option<String> {
    let use_tabs = app.state::<SettingsStore>().get().use_tabs;
    let r = app.state::<WindowRegistry>().inner().0.lock().unwrap();
    let candidate = source
        .filter(|l| r.windows.contains_key(*l))
        .map(str::to_string)
        .or_else(|| r.order.last().cloned())?;
    let entry = r.windows.get(&candidate)?;
    let empty = entry.docs.is_empty() && entry.pending.is_empty();
    (use_tabs || empty).then_some(candidate)
}

fn last_window(app: &AppHandle) -> Option<String> {
    app.state::<WindowRegistry>().inner().0.lock().unwrap().order.last().cloned()
}

pub fn open_path(app: &AppHandle, path: PathBuf, fragment: Option<String>, source: Option<&str>) -> Option<String> {
    // Absolute, with Windows separators, so the same file is always the same string.
    let path = std::path::absolute(&path).unwrap_or(path);
    let path_str = path.to_string_lossy().to_string();

    // Already open somewhere: bring that window forward.
    let owner = {
        let r = app.state::<WindowRegistry>().inner().0.lock().unwrap();
        r.windows
            .iter()
            .find(|(_, e)| e.docs.iter().any(|d| d.eq_ignore_ascii_case(&path_str)))
            .map(|(l, _)| l.clone())
    };
    if let Some(label) = owner {
        deliver(app, &label, OpenRequest::Open { path: path_str, fragment });
        focus(app, &label);
        return Some(label);
    }

    let request = OpenRequest::Open { path: path_str, fragment };
    match target_window(app, source) {
        Some(label) => {
            deliver(app, &label, request);
            focus(app, &label);
            Some(label)
        }
        None => {
            let cascade = source.map(str::to_string).or_else(|| last_window(app));
            create_window(app, cascade.as_deref(), vec![request]).ok()
        }
    }
}

pub fn new_document(app: &AppHandle, source: Option<&str>) {
    match target_window(app, source) {
        Some(label) => {
            deliver(app, &label, OpenRequest::New);
            focus(app, &label);
        }
        None => {
            let cascade = source.map(str::to_string).or_else(|| last_window(app));
            let _ = create_window(app, cascade.as_deref(), vec![OpenRequest::New]);
        }
    }
}

/// Files from the first launch or a later (redirected) launch.
pub fn open_launch_files(app: &AppHandle, files: Vec<PathBuf>, redirected: bool) {
    if files.is_empty() {
        let use_tabs = app.state::<SettingsStore>().get().use_tabs;
        match last_window(app) {
            Some(label) if use_tabs || !redirected => focus(app, &label),
            last => {
                let _ = create_window(app, last.as_deref(), Vec::new());
            }
        }
        return;
    }
    let mut source = last_window(app);
    for file in files {
        source = open_path(app, file, None, source.as_deref()).or(source);
    }
}
