//! The `mdfile` protocol (http://mdfile.localhost/...): serves the preview of every document from the
//! document's own folder URL, plus the files it references. This is what makes relative image paths
//! work; viewers that load HTML from a string have no base URL to resolve them against.

use crate::settings::SettingsStore;
use crate::{i18n, render, urlmap, PendingPrints, TrustedServers};
use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager};

// Markdown may contain raw HTML; only the preview's own scripts may run in it, never scripts that sit
// next to the document, and not the rest of the app's scripts either. Images and media may come from
// anywhere.
const PAGE_CSP: &str = "default-src 'none'; \
    script-src http://mdfile.localhost/__app/viewer.js http://mdfile.localhost/__app/keys.js \
        http://mdfile.localhost/__app/vendor/; \
    style-src http://mdfile.localhost 'unsafe-inline'; \
    img-src * data: blob:; media-src * data: blob:; \
    font-src http://mdfile.localhost/__app/ data:; \
    frame-src https:; object-src 'none'; base-uri 'none'; form-action 'none'; \
    frame-ancestors http://tauri.localhost";

// Everything else on this origin (files next to a document, the app's own assets) is only meant to be
// loaded by a preview page. Scripts on this origin can call the app's commands, so a file that ends up
// as the document itself - an HTML or SVG file a hostile document steers the preview to - is given no
// scripts and no origin. The header means nothing for images, styles, fonts and media.
const FILE_CSP: &str = "default-src 'none'; sandbox";

/// Most bytes sent for one range request; the player asks again for the rest. (Without ranges a video
/// would be read into memory whole, and could not be seeked.)
const MAX_RANGE: u64 = 2 * 1024 * 1024;

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
    let decoded = percent_encoding::percent_decode_str(raw_path).decode_utf8_lossy();
    // Same length as `decoded`, so an ASCII prefix found in it can be cut off `decoded`.
    let lower = decoded.to_ascii_lowercase();

    if let Some(asset) = decoded.strip_prefix(urlmap::APP_PREFIX) {
        return serve_asset(app, asset);
    }

    // Custom-protocol requests carry no Sec-Fetch-Dest header, so the app marks page loads itself:
    // the preview navigates to `doc.md?v=N`, PDF export to `doc.md?pdf=token`.
    let is_page_request = query
        .split('&')
        .any(|kv| kv.starts_with("v=") || kv.starts_with("pdf="));

    if lower.starts_with(urlmap::UNTITLED_PREFIX) {
        return page(app, "", "Untitled", query);
    }

    // Like an untitled document (the shell pushes its text), but relative images load from the app.
    if lower.starts_with(urlmap::EXAMPLES_PREFIX) {
        if is_page_request {
            return page(app, "", "Examples", query);
        }
        return serve_asset(app, &format!("examples/{}", &decoded[urlmap::EXAMPLES_PREFIX.len()..]));
    }

    let Some(path) = urlmap::url_path_to_fs(raw_path) else {
        return not_found();
    };
    // Before the path is touched in any way: see TrustedServers.
    if !app.state::<TrustedServers>().allows(&path) {
        return forbidden();
    }

    if is_page_request && is_markdown(&path) && path.is_file() {
        let pending = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("pdf="))
            .and_then(|token| app.state::<PendingPrints>().0.lock().unwrap().get(token).cloned());
        let text = match pending {
            Some(t) => t,
            None => match crate::textfile::read(&path, None) {
                Ok(f) => f.text,
                Err(e) => return error(&format!("Cannot read {}: {e}", path.display())),
            },
        };
        let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        return page(app, &text, &title, query);
    }

    serve_file(&path, request.headers().get(header::RANGE).and_then(|v| v.to_str().ok()))
}

fn serve_file(path: &Path, range: Option<&str>) -> Response<Cow<'static, [u8]>> {
    let opened = std::fs::File::open(path).and_then(|file| {
        let meta = file.metadata()?;
        Ok((file, meta))
    });
    let (mut file, len) = match opened {
        Ok((file, meta)) if meta.is_file() => (file, meta.len()),
        Ok(_) => return not_found(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return not_found(),
        Err(e) => return error(&e.to_string()),
    };
    let response = Response::builder()
        .header(header::CONTENT_TYPE, mime_for(path))
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ACCEPT_RANGES, "bytes")
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Security-Policy", FILE_CSP);

    let (response, start, count) = match range.map(|r| parse_range(r, len)) {
        None | Some(Range::Whole) => (response, 0, len),
        Some(Range::Part(start, end)) => (
            response
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}")),
            start,
            end - start + 1,
        ),
        Some(Range::Unsatisfiable) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Cow::Borrowed(&b""[..]))
                .unwrap();
        }
    };

    let mut bytes = Vec::new();
    let read = file.seek(SeekFrom::Start(start)).and_then(|_| file.take(count).read_to_end(&mut bytes));
    match read {
        Ok(_) => response.body(Cow::Owned(bytes)).unwrap(),
        Err(e) => error(&e.to_string()),
    }
}

#[derive(Debug, PartialEq)]
enum Range {
    /// No usable range: send the whole file.
    Whole,
    /// First and last byte to send.
    Part(u64, u64),
    Unsatisfiable,
}

/// One `bytes=first-last`, `bytes=first-` or `bytes=-suffix` range, capped at MAX_RANGE. Anything else
/// (several ranges, other units) is answered with the whole file, as HTTP allows.
fn parse_range(value: &str, len: u64) -> Range {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Range::Whole;
    };
    let Some((first, last)) = spec.split_once('-') else {
        return Range::Whole;
    };
    let (first, last) = (first.trim(), last.trim());
    let (start, end) = match (first.parse::<u64>(), last.parse::<u64>()) {
        (Ok(start), Ok(end)) if start <= end => (start, end),
        (Ok(start), Err(_)) if last.is_empty() => (start, u64::MAX),
        (Err(_), Ok(suffix)) if first.is_empty() && suffix > 0 => (len.saturating_sub(suffix), u64::MAX),
        _ => return Range::Whole,
    };
    if start >= len {
        return Range::Unsatisfiable;
    }
    Range::Part(start, end.min(len - 1).min(start.saturating_add(MAX_RANGE - 1)))
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
    let content = render::render(markdown);
    // Decided here rather than in viewer.js so the first layout already uses it.
    let article_class = if render::is_large(&content) { " large" } else { "" };
    let html = fill(
        &template,
        &[
            ("LANG", lang),
            ("TITLE", &escape_html(title)),
            ("THEME", &theme),
            ("LIGHT_MEDIA", if dark { "not all" } else { "all" }),
            ("DARK_MEDIA", if dark { "all" } else { "not all" }),
            ("CONFIG", &config.to_string().replace('<', "\\u003c")),
            ("ARTICLE_CLASS", article_class),
            ("CONTENT", &content),
        ],
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header("Content-Security-Policy", PAGE_CSP)
        .body(Cow::Owned(html.into_bytes()))
        .unwrap()
}

/// Fills the `{{NAME}}` placeholders in one pass, so text that was filled in (a file name, the document)
/// is never itself searched for placeholders.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + values.iter().map(|(_, v)| v.len()).sum::<usize>());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let value = after
            .find("}}")
            .and_then(|end| values.iter().find(|(name, _)| *name == &after[..end]).map(|(_, v)| (end, *v)));
        match value {
            Some((end, value)) => {
                out.push_str(value);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// One of the app's own files (`relative` is already percent-decoded).
fn serve_asset(app: &AppHandle, relative: &str) -> Response<Cow<'static, [u8]>> {
    if relative.split(['/', '\\']).any(|part| part == "..") {
        return not_found();
    }
    match app.asset_resolver().get(relative.to_string()) {
        Some(asset) => Response::builder()
            .header(header::CONTENT_TYPE, mime_for(Path::new(relative)))
            .header(header::CACHE_CONTROL, "max-age=3600")
            .header("X-Content-Type-Options", "nosniff")
            .header("Content-Security-Policy", FILE_CSP)
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

fn forbidden() -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Cow::Borrowed(&b"Forbidden"[..]))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_parsed() {
        assert_eq!(parse_range("bytes=0-", 100), Range::Part(0, 99));
        assert_eq!(parse_range("bytes=10-19", 100), Range::Part(10, 19));
        assert_eq!(parse_range("bytes=10-500", 100), Range::Part(10, 99));
        assert_eq!(parse_range("bytes=-30", 100), Range::Part(70, 99));
        assert_eq!(parse_range("bytes=-300", 100), Range::Part(0, 99));
        assert_eq!(parse_range("bytes=0-", MAX_RANGE * 3), Range::Part(0, MAX_RANGE - 1));
        assert_eq!(parse_range("bytes=5-", MAX_RANGE * 3), Range::Part(5, MAX_RANGE + 4));
        assert_eq!(parse_range("bytes=100-", 100), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-", 0), Range::Unsatisfiable);
        for whole in ["", "bytes=", "bytes=-", "bytes=5-2", "bytes=0-1, 5-9", "items=0-5", "bytes=a-b", "bytes=-0"] {
            assert_eq!(parse_range(whole, 100), Range::Whole, "{whole}");
        }
    }

    #[test]
    fn files_are_served_whole_or_in_part_and_inert() {
        let path = std::env::temp_dir().join(format!("markview-test-{}.svg", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        let header = |r: &Response<Cow<'static, [u8]>>, name: &str| {
            r.headers().get(name).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
        };

        let whole = serve_file(&path, None);
        assert_eq!((whole.status(), whole.body().as_ref()), (StatusCode::OK, &b"0123456789"[..]));
        assert_eq!(header(&whole, "content-type"), "image/svg+xml");
        // Opened as a page of its own, a file gets no scripts and no origin.
        assert!(header(&whole, "content-security-policy").contains("sandbox"));

        let part = serve_file(&path, Some("bytes=2-5"));
        assert_eq!((part.status(), part.body().as_ref()), (StatusCode::PARTIAL_CONTENT, &b"2345"[..]));
        assert_eq!(header(&part, "content-range"), "bytes 2-5/10");

        let tail = serve_file(&path, Some("bytes=7-"));
        assert_eq!((tail.body().as_ref(), header(&tail, "content-range").as_str()), (&b"789"[..], "bytes 7-9/10"));

        let beyond = serve_file(&path, Some("bytes=10-"));
        assert_eq!(beyond.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(header(&beyond, "content-range"), "bytes */10");

        std::fs::remove_file(&path).unwrap();
        assert_eq!(serve_file(&path, None).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve_file(&std::env::temp_dir(), None).status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn placeholders_are_filled_once() {
        let values = [("TITLE", "{{CONTENT}}.md"), ("CONTENT", "{{TITLE}} body")];
        assert_eq!(
            fill("<t>{{TITLE}}</t>{{CONTENT}}{{OTHER}}{{", &values),
            "<t>{{CONTENT}}.md</t>{{TITLE}} body{{OTHER}}{{"
        );
    }

    #[test]
    fn the_template_only_uses_known_placeholders() {
        let names = ["LANG", "TITLE", "THEME", "LIGHT_MEDIA", "DARK_MEDIA", "CONFIG", "ARTICLE_CLASS", "CONTENT"];
        let values: Vec<(&str, &str)> = names.iter().map(|n| (*n, "")).collect();
        let html = fill(include_str!("../../ui/viewer.html"), &values);
        assert!(!html.contains("{{"), "unfilled placeholder in viewer.html");
    }
}
