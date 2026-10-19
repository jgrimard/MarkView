//! The `mdfile` protocol (http://mdfile.localhost/...): serves the preview of every document from the
//! document's own folder URL, plus the files it references. This is what makes relative image paths
//! work; viewers that load HTML from a string have no base URL to resolve them against.

use crate::settings::SettingsStore;
use crate::{i18n, render, urlmap, PendingPrints};
use std::borrow::Cow;
use std::path::Path;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager};

// Markdown may contain raw HTML; only the app's own scripts may run in the preview, never scripts that
// sit next to the document. Images and media may come from anywhere.
const PAGE_CSP: &str = "default-src 'none'; \
    script-src http://mdfile.localhost/__app/; \
    style-src http://mdfile.localhost 'unsafe-inline'; \
    img-src * data: blob:; media-src * data: blob:; \
    font-src http://mdfile.localhost/__app/ data:; \
    frame-src https:; object-src 'none'; base-uri 'none'; form-action 'none'; \
    frame-ancestors http://tauri.localhost";

const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "mdtxt", "mdtext"];

pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MARKDOWN_EXTENSIONS.iter().any(|m| m.eq_ignore_ascii_case(e)))
}

pub fn handle(app: &AppHandle, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let uri = request.uri();
    let raw_path = uri.path();
    let query = uri.query().unwrap_or("");
    let decoded_lower = percent_encoding::percent_decode_str(raw_path).decode_utf8_lossy().to_ascii_lowercase();

    if let Some(asset) = raw_path.strip_prefix(urlmap::APP_PREFIX) {
        return serve_asset(app, asset);
    }

    // Custom-protocol requests carry no Sec-Fetch-Dest header, so the app marks page loads itself:
    // the preview navigates to `doc.md?v=N`, PDF export to `doc.md?pdf=token`.
    let is_page_request = query
        .split('&')
        .any(|kv| kv.starts_with("v=") || kv.starts_with("pdf="));

    if decoded_lower.starts_with(urlmap::UNTITLED_PREFIX) {
        return page(app, "", "Untitled", query);
    }

    let Some(path) = urlmap::url_path_to_fs(raw_path) else {
        return not_found();
    };

    if is_page_request && is_markdown(&path) && path.is_file() {
        let pending = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("pdf="))
            .and_then(|token| app.state::<PendingPrints>().0.lock().unwrap().get(token).cloned());
        let text = match pending {
            Some(t) => t,
            None => match read_text(&path) {
                Ok((t, _)) => t,
                Err(e) => return error(&format!("Cannot read {}: {e}", path.display())),
            },
        };
        let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        return page(app, &text, &title, query);
    }

    if !path.is_file() {
        return not_found();
    }

    match std::fs::read(&path) {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, mime_for(&path))
            .header(header::CACHE_CONTROL, "no-cache")
            .header("X-Content-Type-Options", "nosniff")
            .body(Cow::Owned(bytes))
            .unwrap(),
        Err(e) => error(&e.to_string()),
    }
}

/// The strings a preview page shows itself (the `viewer.` keys, without the prefix).
fn viewer_strings(all: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    all.iter()
        .filter_map(|(k, v)| k.strip_prefix("viewer.").map(|k| (k.to_string(), v.clone())))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

fn page(app: &AppHandle, markdown: &str, title: &str, query: &str) -> Response<Cow<'static, [u8]>> {
    let settings = app.state::<SettingsStore>().get();
    let standalone = query.split('&').any(|kv| kv.starts_with("pdf="));
    let theme = if standalone { "light".to_string() } else { effective_theme(&settings.theme) };
    let template = app
        .asset_resolver()
        .get("viewer.html".to_string())
        .map(|a| String::from_utf8_lossy(&a.bytes).to_string())
        .unwrap_or_default();
    let lang = i18n::effective(&settings.language);
    let config = serde_json::json!({
        "theme": theme,
        "toc": settings.toc_visible && !standalone,
        "zoom": if standalone { 1.0 } else { settings.zoom },
        "standalone": standalone,
        "strings": viewer_strings(&i18n::strings(app, lang)),
    });
    let dark = theme == "dark";
    let html = template
        .replace("{{LANG}}", lang)
        .replace("{{TITLE}}", &escape_html(title))
        .replace("{{THEME}}", &theme)
        .replace("{{LIGHT_MEDIA}}", if dark { "not all" } else { "all" })
        .replace("{{DARK_MEDIA}}", if dark { "all" } else { "not all" })
        .replace("{{CONFIG}}", &config.to_string().replace('<', "\\u003c"))
        .replace("{{CONTENT}}", &render::render(markdown)); // last: document text is never a placeholder
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header("Content-Security-Policy", PAGE_CSP)
        .body(Cow::Owned(html.into_bytes()))
        .unwrap()
}

fn serve_asset(app: &AppHandle, relative: &str) -> Response<Cow<'static, [u8]>> {
    let relative = percent_encoding::percent_decode_str(relative).decode_utf8_lossy().to_string();
    match app.asset_resolver().get(relative.clone()) {
        Some(asset) => Response::builder()
            .header(header::CONTENT_TYPE, mime_for(Path::new(&relative)))
            .header(header::CACHE_CONTROL, "max-age=3600")
            .body(Cow::Owned(asset.bytes))
            .unwrap(),
        None => not_found(),
    }
}

pub fn effective_theme(setting: &str) -> String {
    match setting {
        "light" | "dark" => setting.to_string(),
        _ => {
            if system_prefers_dark() {
                "dark".into()
            } else {
                "light".into()
            }
        }
    }
}

#[cfg(windows)]
fn system_prefers_dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|v| v == 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn system_prefers_dark() -> bool {
    false
}

/// Reads a text file; returns the text and whether it started with a UTF-8 byte order mark.
pub fn read_text(path: &Path) -> std::io::Result<(String, bool)> {
    let bytes = std::fs::read(path)?;
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let body = if bom { &bytes[3..] } else { &bytes[..] };
    Ok((String::from_utf8_lossy(body).into_owned(), bom))
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn not_found() -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Cow::Borrowed(&b"Not found"[..]))
        .unwrap()
}

fn error(message: &str) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Cow::Owned(message.as_bytes().to_vec()))
        .unwrap()
}

pub fn mime_for(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "txt" | "md" | "markdown" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "png" => "image/png",
        "apng" => "image/apng",
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}
