//! User settings, stored as JSON in %LOCALAPPDATA%\MarkView\markview.json.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_RECENT: usize = 15;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "system", "light" or "dark".
    pub theme: String,
    pub zoom: f64,
    pub toc_visible: bool,
    /// Off: every document opens in its own window. On: documents open as tabs.
    pub use_tabs: bool,
    pub split_ratio: f64,
    pub recent: Vec<String>,
    pub window: Option<Placement>,
    /// "ask" or "never": whether to offer the .md file association at startup.
    pub assoc_prompt: String,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            zoom: 1.0,
            toc_visible: false,
            use_tabs: false,
            split_ratio: 0.5,
            recent: Vec::new(),
            window: None,
            assoc_prompt: "ask".into(),
        }
    }
}

pub struct SettingsStore(pub Mutex<Settings>);

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("MarkView");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// %LOCALAPPDATA%\Programs, the usual home for per-user apps.
pub fn local_programs_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Programs")
}

fn settings_path() -> PathBuf {
    data_dir().join("markview.json")
}

impl SettingsStore {
    pub fn load() -> Self {
        let settings = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self(Mutex::new(settings))
    }

    pub fn get(&self) -> Settings {
        self.0.lock().unwrap().clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = self.0.lock().unwrap();
        f(&mut s);
        if let Ok(json) = serde_json::to_string_pretty(&*s) {
            let _ = std::fs::write(settings_path(), json);
        }
        s.clone()
    }

    pub fn add_recent(&self, path: &str) -> Settings {
        self.update(|s| {
            s.recent.retain(|p| !p.eq_ignore_ascii_case(path));
            s.recent.insert(0, path.to_string());
            s.recent.truncate(MAX_RECENT);
        })
    }
}
