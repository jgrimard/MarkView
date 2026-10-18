#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assoc;
mod package;
mod pdf;
mod render;
mod server;
mod settings;
mod urlmap;
mod watcher;
mod windows;

use base64::Engine;
use serde::Serialize;
use settings::{Placement, Settings, SettingsStore};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use watcher::DocWatcher;
use windows::{OpenRequest, WindowRegistry};

/// Unsaved document text handed to the PDF export window, keyed by a one-time token.
#[derive(Default)]
pub struct PendingPrints(pub Mutex<HashMap<String, String>>);

/// Linked files with these extensions are revealed in Explorer instead of being run.
const UNSAFE_TO_LAUNCH: &[&str] = &[
    "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "scr",
    "pif", "lnk", "url", "hta", "cpl", "reg", "jar", "appref-ms", "application", "msc",
];

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif", "apng", "jfif"];

fn has_ext(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

// ---------------------------------------------------------------- documents

#[derive(Serialize)]
struct FileText {
    text: String,
    bom: bool,
}

#[tauri::command]
fn read_file(path: String) -> Result<FileText, String> {
    server::read_text(Path::new(&path))
        .map(|(text, bom)| FileText { text, bom })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn write_file(path: String, text: String, bom: bool) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    bytes.extend_from_slice(text.as_bytes());
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

#[tauri::command]
fn render(text: String) -> String {
    render::render(&text)
}

#[tauri::command]
fn doc_url(path: Option<String>, untitled: u32) -> String {
    match path {
        Some(p) => urlmap::path_to_url(Path::new(&p)),
        None => urlmap::untitled_url(untitled),
    }
}

#[tauri::command]
fn add_recent(app: AppHandle, path: String) {
    let s = app.state::<SettingsStore>().add_recent(&path);
    let _ = app.emit("settings-changed", s);
}

/// Watches the local files a preview shows (reported by the page), so replacing one refreshes it.
#[tauri::command]
async fn watch_dependencies(app: AppHandle, doc: String, urls: Vec<String>) {
    let watcher = app.state::<DocWatcher>();
    for url in urls {
        if let Some((path, _)) = urlmap::url_to_path(&url) {
            watcher.add_dependency(Path::new(&doc), &path);
        }
    }
}

#[tauri::command]
fn path_exists(path: String) -> bool {
    Path::new(&path).is_file()
}

/// Saves a pasted image next to the document (images/<doc>-<timestamp>.<ext>) and returns its relative path.
#[tauri::command]
fn save_pasted_image(doc_path: String, data: String, ext: String) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
    let doc = Path::new(&doc_path);
    let dir = doc.parent().ok_or("Document has no folder")?.join("images");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem: String = doc
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".into())
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let ext = ext.trim_start_matches('.');
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut name = format!("{stem}-{stamp}.{ext}");
    let mut i = 2;
    while dir.join(&name).exists() {
        name = format!("{stem}-{stamp}-{i}.{ext}");
        i += 1;
    }
    std::fs::write(dir.join(&name), bytes).map_err(|e| e.to_string())?;
    Ok(format!("images/{name}"))
}

/// Markdown for a dropped file: a link relative to the document (an image link for images).
#[tauri::command]
fn link_for_file(doc_path: Option<String>, file: String) -> String {
    let file_path = Path::new(&file);
    let target = doc_path
        .as_deref()
        .and_then(|d| Path::new(d).parent())
        .and_then(|dir| relative_path(dir, file_path))
        .unwrap_or_else(|| file.replace('\\', "/"));
    let label = file_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dest = if target.contains([' ', '(', ')']) { format!("<{target}>") } else { target };
    if has_ext(file_path, IMAGE_EXTENSIONS) {
        format!("![{label}]({dest})")
    } else {
        format!("[{label}]({dest})")
    }
}

fn relative_path(from_dir: &Path, to: &Path) -> Option<String> {
    let from: Vec<_> = from_dir.components().collect();
    let to_c: Vec<_> = to.components().collect();
    // Different drives cannot be expressed relatively.
    if from.first().map(|c| c.as_os_str().to_ascii_lowercase()) != to_c.first().map(|c| c.as_os_str().to_ascii_lowercase()) {
        return None;
    }
    let common = from
        .iter()
        .zip(&to_c)
        .take_while(|(a, b)| a.as_os_str().eq_ignore_ascii_case(b.as_os_str()))
        .count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), from.len() - common).collect();
    parts.extend(to_c[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    Some(parts.join("/"))
}

#[tauri::command]
fn suggest_name(text: String) -> String {
    let base = render::first_heading(&text).unwrap_or_else(|| "Untitled".into());
    let cleaned: String = base
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c })
        .take(60)
        .collect();
    cleaned.trim().to_string()
}

// ---------------------------------------------------------------- links

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkResult {
    /// "markdown" (the page should navigate), "handled" (opened elsewhere) or "missing"
    kind: &'static str,
    path: Option<String>,
    fragment: Option<String>,
}

/// Routes a clicked link: web links to the browser, Markdown back to the page, other files to the shell.
#[tauri::command]
fn open_link(url: String) -> LinkResult {
    let handled = LinkResult { kind: "handled", path: None, fragment: None };
    let lower = url.to_ascii_lowercase();
    let is_local = lower.starts_with(urlmap::ORIGIN) || lower.starts_with("file:");
    if !is_local {
        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") || lower.starts_with("tel:") {
            let _ = open::that_detached(&url);
        }
        return handled;
    }
    let Some((path, fragment)) = urlmap::url_to_path(&url) else {
        return handled;
    };
    let path_str = path.to_string_lossy().to_string();
    if path.is_dir() {
        let _ = std::process::Command::new("explorer.exe").arg(&path).spawn();
        handled
    } else if !path.is_file() {
        LinkResult { kind: "missing", path: Some(path_str), fragment: None }
    } else if server::is_markdown(&path) {
        LinkResult { kind: "markdown", path: Some(path_str), fragment }
    } else if has_ext(&path, UNSAFE_TO_LAUNCH) {
        reveal(path_str);
        handled
    } else {
        let _ = open::that_detached(&path);
        handled
    }
}

#[tauri::command]
fn reveal(path: String) {
    use std::os::windows::process::CommandExt;
    // Explorer only understands `/select,"path"`; `.arg()` would quote the whole switch when the path has spaces.
    let path = path.replace('/', "\\");
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn();
}

// ---------------------------------------------------------------- windows

#[tauri::command]
fn window_ready(app: AppHandle, window: WebviewWindow) -> Vec<OpenRequest> {
    windows::window_ready(&app, window.label())
}

#[tauri::command]
async fn report_docs(app: AppHandle, window: WebviewWindow, paths: Vec<String>) {
    windows::set_docs(&app, window.label(), paths);
}

// These may create windows, so they must be async: synchronous commands run on the main thread, and
// creating a window from there deadlocks on Windows.
#[tauri::command]
async fn open_paths(app: AppHandle, window: WebviewWindow, paths: Vec<String>, fragment: Option<String>) {
    for p in paths {
        windows::open_path(&app, PathBuf::from(p), fragment.clone(), Some(window.label()));
    }
}

#[tauri::command]
async fn new_document(app: AppHandle, window: WebviewWindow) {
    windows::new_document(&app, Some(window.label()));
}

#[tauri::command]
fn set_title(window: WebviewWindow, title: String) {
    let _ = window.set_title(&title);
}

/// Saves the window placement and closes it (after the page has dealt with unsaved changes).
#[tauri::command]
fn close_window(app: AppHandle, window: WebviewWindow) {
    if let (Ok(pos), Ok(size), Ok(max)) = (window.outer_position(), window.inner_size(), window.is_maximized()) {
        let minimized = window.is_minimized().unwrap_or(false);
        if !minimized {
            app.state::<SettingsStore>().update(|s| {
                let previous = s.window;
                s.window = Some(if max {
                    Placement { maximized: true, ..previous.unwrap_or(Placement { x: pos.x, y: pos.y, width: 1280, height: 860, maximized: true }) }
                } else {
                    Placement { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: false }
                });
            });
        }
    }
    let _ = window.destroy();
}

// ---------------------------------------------------------------- dialogs

#[tauri::command]
async fn pick_open(window: WebviewWindow) -> Vec<String> {
    let dialog = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .add_filter("Markdown", &["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "txt"])
        .add_filter("All files", &["*"]);
    dialog
        .pick_files()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.path().to_string_lossy().to_string())
        .collect()
}

#[tauri::command]
async fn pick_save(window: WebviewWindow, suggested: String, kind: String, directory: Option<String>) -> Option<String> {
    let mut dialog = rfd::AsyncFileDialog::new().set_parent(&window).set_file_name(suggested);
    dialog = if kind == "pdf" {
        dialog.add_filter("PDF document", &["pdf"])
    } else {
        dialog.add_filter("Markdown", &["md"]).add_filter("All files", &["*"])
    };
    if let Some(dir) = directory {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file().await.map(|f| f.path().to_string_lossy().to_string())
}

#[tauri::command]
async fn export_pdf(app: AppHandle, doc_path: String, text: String, out: String) -> Result<(), String> {
    pdf::export(app, doc_path, text, out).await
}

#[tauri::command]
fn open_path_external(path: String) {
    let _ = open::that_detached(path);
}

// ---------------------------------------------------------------- settings & association

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    app.state::<SettingsStore>().get()
}

#[tauri::command]
fn update_settings(app: AppHandle, patch: serde_json::Value) -> Settings {
    let store = app.state::<SettingsStore>();
    let updated = store.update(|s| {
        let mut value = serde_json::to_value(&*s).unwrap_or_default();
        if let (Some(obj), Some(p)) = (value.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                obj.insert(k.clone(), v.clone());
            }
        }
        if let Ok(merged) = serde_json::from_value(value) {
            *s = merged;
        }
    });
    let theme = windows::window_theme(&updated.theme);
    let dark = server::effective_theme(&updated.theme) == "dark";
    for (_, w) in app.webview_windows() {
        let _ = w.set_theme(theme);
        #[cfg(windows)]
        let _ = window_vibrancy::apply_mica(&w, Some(dark));
    }
    let _ = app.emit("settings-changed", updated.clone());
    updated
}

#[tauri::command]
fn system_theme() -> String {
    server::effective_theme("system")
}

/// The Windows accent colour as WinUI uses it: AccentDark1 on light backgrounds, AccentLight2 on dark.
#[tauri::command]
fn accent_colors() -> Option<serde_json::Value> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        let palette = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent")
            .and_then(|k| k.get_raw_value("AccentPalette"))
            .ok()?;
        let b = palette.bytes;
        if b.len() < 32 {
            return None;
        }
        let hex = |i: usize| format!("#{:02x}{:02x}{:02x}", b[i * 4], b[i * 4 + 1], b[i * 4 + 2]);
        Some(serde_json::json!({ "light": hex(4), "dark": hex(1) }))
    }
    #[cfg(not(windows))]
    None
}

/// { registered: points at this exe, registeredExe: whatever the registration points at (may be a moved copy) }
#[tauri::command]
fn association_status() -> serde_json::Value {
    if package::is_packaged() {
        return serde_json::json!({ "packaged": true });
    }
    serde_json::json!({
        "registered": assoc::is_registered(),
        "registeredExe": assoc::registered_exe(),
    })
}

#[tauri::command]
fn register_association() -> Result<String, String> {
    assoc::register().map_err(|e| e.to_string())?;
    Ok(std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default())
}

/// Offered once per launch: "use MarkView for .md files?" Returns where the exe is, or None when there
/// is nothing to ask (already registered from this location, or the user said don't ask again).
#[tauri::command]
fn association_prompt(app: AppHandle) -> Option<serde_json::Value> {
    static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // The Store build is associated with .md files by its package manifest.
    if package::is_packaged() || app.state::<SettingsStore>().get().assoc_prompt == "never" || assoc::is_registered() {
        return None;
    }
    if ASKED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return None; // only the first window of this launch asks
    }
    let mut info = exe_location();
    info["previous"] = serde_json::json!(assoc::registered_exe());
    Some(info)
}

/// Where this exe runs from, whether that looks temporary (Downloads/Temp), and a suggested permanent home.
fn exe_location() -> serde_json::Value {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let lower = exe.to_lowercase();
    let temp = std::env::temp_dir().to_string_lossy().to_lowercase();
    let temporary = lower.contains(r"\downloads\") || lower.starts_with(temp.trim_end_matches('\\'));
    serde_json::json!({
        "exe": exe,
        "temporary": temporary,
        "suggested": settings::local_programs_dir().join("MarkView").to_string_lossy(),
    })
}

#[tauri::command]
fn unregister_association() -> Result<(), String> {
    assoc::unregister().map_err(|e| e.to_string())
}

#[tauri::command]
fn open_default_apps() {
    let page = match package::app_user_model_id() {
        Some(aumid) => format!("ms-settings:defaultapps?registeredAUMID={aumid}"),
        None => "ms-settings:defaultapps?registeredAppUser=MarkView".into(),
    };
    let _ = open::that_detached(page);
}

#[tauri::command]
fn app_info() -> serde_json::Value {
    let mut info = exe_location();
    info["version"] = serde_json::json!(env!("CARGO_PKG_VERSION"));
    info["repository"] = serde_json::json!(env!("CARGO_PKG_REPOSITORY"));
    info["dataDir"] = serde_json::json!(settings::data_dir().to_string_lossy());
    info["packaged"] = serde_json::json!(package::is_packaged());
    info
}

// ---------------------------------------------------------------- startup

fn files_from_args(args: &[String], cwd: &Path) -> Vec<PathBuf> {
    args.iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(|a| {
            let p = PathBuf::from(a);
            if p.is_absolute() { p } else { cwd.join(p) }
        })
        .filter(|p| p.is_file())
        .collect()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let files = files_from_args(&args, Path::new(&cwd));
            windows::open_launch_files(app, files, true);
        }))
        .manage(SettingsStore::load())
        .manage(WindowRegistry::default())
        .manage(PendingPrints::default())
        .register_asynchronous_uri_scheme_protocol("mdfile", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || responder.respond(server::handle(&app, &request)));
        })
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(DocWatcher::new(handle.clone()));
            let args: Vec<String> = std::env::args().collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            let files = files_from_args(&args, &cwd);
            if files.is_empty() {
                windows::create_window(&handle, None, Vec::new())?;
            } else {
                windows::open_launch_files(&handle, files, false);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            read_file,
            write_file,
            render,
            doc_url,
            add_recent,
            path_exists,
            watch_dependencies,
            save_pasted_image,
            link_for_file,
            suggest_name,
            open_link,
            reveal,
            window_ready,
            report_docs,
            open_paths,
            new_document,
            set_title,
            close_window,
            pick_open,
            pick_save,
            export_pdf,
            open_path_external,
            get_settings,
            update_settings,
            system_theme,
            accent_colors,
            association_status,
            association_prompt,
            register_association,
            unregister_association,
            open_default_apps,
            app_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MarkView");
}
