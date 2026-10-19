//! Maps file-system paths to URLs on the local document origin and back.
//!
//!   C:\Docs\My notes\a.md   <->  http://mdfile.localhost/C:/Docs/My%20notes/a.md
//!   \\server\share\b.png     <->  http://mdfile.localhost/unc/server/share/b.png
//!
//! Every document is served from its own folder's URL, so relative references such as
//! `img/a.png` or `../assets/b.svg` are resolved by the browser itself.

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use std::path::{Path, PathBuf};

pub const ORIGIN: &str = "http://mdfile.localhost";
pub const APP_PREFIX: &str = "/__app/";
pub const UNTITLED_PREFIX: &str = "/__untitled/";
/// The built-in Markdown examples; its images are served from the app's `examples/` folder.
pub const EXAMPLES_PREFIX: &str = "/__examples/";

/// Everything except RFC 3986 unreserved characters is escaped (like .NET's EscapeDataString).
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

pub fn path_to_url(path: &Path) -> String {
    let mut s = path.to_string_lossy().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        s = format!(r"\\{rest}");
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }

    let (prefix, rest) = match s.strip_prefix(r"\\") {
        Some(rest) => ("/unc", rest.to_string()),
        None => ("", s),
    };

    let segments: Vec<String> = rest
        .split(['\\', '/'])
        .enumerate()
        .map(|(i, seg)| {
            if i == 0 && is_drive(seg) {
                seg.to_string()
            } else {
                utf8_percent_encode(seg, SEGMENT).to_string()
            }
        })
        .collect();
    format!("{ORIGIN}{prefix}/{}", segments.join("/"))
}

pub fn untitled_url(id: u32) -> String {
    format!("{ORIGIN}{UNTITLED_PREFIX}{id}/Untitled.md")
}

pub fn examples_url() -> String {
    format!("{ORIGIN}{EXAMPLES_PREFIX}Examples.md")
}

fn is_drive(seg: &str) -> bool {
    let b = seg.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Path part of a URL on the document origin (still percent-encoded), or None for other URLs.
pub fn local_path_part(url: &str) -> Option<&str> {
    let rest = url
        .get(..ORIGIN.len())
        .filter(|p| p.eq_ignore_ascii_case(ORIGIN))
        .map(|_| &url[ORIGIN.len()..])?;
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Converts the (percent-encoded) path of a document-origin URL to a file-system path.
pub fn url_path_to_fs(path: &str) -> Option<PathBuf> {
    let decoded = percent_decode_str(path).decode_utf8_lossy().to_string();
    let lower = decoded.to_ascii_lowercase();
    if lower.starts_with(APP_PREFIX) || lower.starts_with(UNTITLED_PREFIX) || lower.starts_with(EXAMPLES_PREFIX) {
        return None;
    }
    if let Some(rest) = decoded.strip_prefix("/unc/") {
        if rest.is_empty() {
            return None;
        }
        return Some(PathBuf::from(format!(r"\\{}", rest.replace('/', "\\"))));
    }
    let rest = decoded.strip_prefix('/')?;
    let drive = rest.get(..2).filter(|d| is_drive(d))?;
    let tail = rest[2..].trim_start_matches('/').replace('/', "\\");
    Some(PathBuf::from(format!("{}:\\{}", drive[..1].to_ascii_uppercase(), tail)))
}

/// Resolves a document-origin URL or a file:// URL to a path. Returns the path and the fragment.
pub fn url_to_path(url: &str) -> Option<(PathBuf, Option<String>)> {
    let fragment = url
        .find('#')
        .map(|i| percent_decode_str(&url[i + 1..]).decode_utf8_lossy().to_string())
        .filter(|f| !f.is_empty());
    if let Some(path) = local_path_part(url) {
        return url_path_to_fs(path).map(|p| (p, fragment));
    }
    if url.len() > 5 && url[..5].eq_ignore_ascii_case("file:") {
        return file_url_to_path(url).map(|p| (p, fragment));
    }
    None
}

fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let mut rest = &url[5..];
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    rest = &rest[..end];
    let decoded = percent_decode_str(rest).decode_utf8_lossy().to_string();
    let trimmed = decoded.trim_start_matches('/');
    if trimmed.len() >= 2 && is_drive(&trimmed[..2]) {
        return Some(PathBuf::from(trimmed.replace('/', "\\")));
    }
    // file://server/share/x  ->  \\server\share\x
    let host_path = decoded.strip_prefix("//")?;
    if host_path.is_empty() || host_path.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(format!(r"\\{}", host_path.replace('/', "\\"))))
}

/// Rewrites an absolute Windows path, UNC path or file:// URL into a document-origin URL.
/// Returns None for anything else (relative paths, http, data:, ...), which needs no rewriting.
pub fn rewrite_absolute(value: &str) -> Option<String> {
    let v = value.trim();
    let decoded = percent_decode_str(v).decode_utf8_lossy().to_string();

    if decoded.len() > 5 && decoded[..5].eq_ignore_ascii_case("file:") {
        let path = file_url_to_path(v)?;
        let fragment = v.find('#').map(|i| &v[i..]).unwrap_or("");
        return Some(path_to_url(&path) + fragment);
    }

    let b = decoded.as_bytes();
    let windows_abs = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if !windows_abs && !decoded.starts_with(r"\\") {
        return None;
    }
    let (path, fragment) = match decoded.find('#') {
        Some(i) if i > 0 => (&decoded[..i], &decoded[i..]),
        _ => (decoded.as_str(), ""),
    };
    Some(path_to_url(Path::new(path)) + fragment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths() {
        let p = Path::new(r"C:\Users\me\My Docs\pic é.png");
        let url = path_to_url(p);
        assert_eq!(url, "http://mdfile.localhost/C:/Users/me/My%20Docs/pic%20%C3%A9.png");
        assert_eq!(url_to_path(&url).unwrap().0, p);

        let unc = Path::new(r"\\server\share\a b.md");
        let url = path_to_url(unc);
        assert_eq!(url, "http://mdfile.localhost/unc/server/share/a%20b.md");
        assert_eq!(url_to_path(&url).unwrap().0, unc);
    }

    #[test]
    fn rewrites_absolute_references() {
        assert_eq!(
            rewrite_absolute(r"C:\x\a.png").unwrap(),
            "http://mdfile.localhost/C:/x/a.png"
        );
        assert_eq!(
            rewrite_absolute("file:///C:/My%20Pics/a.png").unwrap(),
            "http://mdfile.localhost/C:/My%20Pics/a.png"
        );
        assert_eq!(
            rewrite_absolute("C:%5Cx%5Ca.png").unwrap(),
            "http://mdfile.localhost/C:/x/a.png"
        );
        assert!(rewrite_absolute("img/a.png").is_none());
        assert!(rewrite_absolute("https://example.com/a.png").is_none());
    }

    #[test]
    fn fragments_are_kept() {
        let (p, f) = url_to_path("http://mdfile.localhost/C:/d/other.md#images").unwrap();
        assert_eq!(p, Path::new(r"C:\d\other.md"));
        assert_eq!(f.as_deref(), Some("images"));
    }

    #[test]
    fn app_urls_are_not_files() {
        assert!(url_to_path(&examples_url()).is_none());
        assert!(url_to_path(&untitled_url(3)).is_none());
    }
}
