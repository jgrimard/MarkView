//! Markdown -> HTML with comrak (GitHub-flavoured: tables, task lists, footnotes, alerts, math, emoji).

use crate::urlmap;
use comrak::nodes::NodeValue;
use comrak::{format_html, parse_document, Arena, Options};
use regex::{Captures, Regex};
use std::sync::LazyLock;

fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    o.extension.description_lists = true;
    o.extension.front_matter_delimiter = Some("---".to_string());
    o.extension.header_id_prefix = Some(String::new());
    o.extension.alerts = true;
    o.extension.math_dollars = true;
    o.extension.math_code = true;
    o.extension.shortcodes = true;
    o.parse.smart = false;
    o.render.r#unsafe = true; // documents may contain raw HTML (e.g. <img width=...>)
    o.render.sourcepos = true; // data-sourcepos drives scroll sync and task toggling
    o.render.tasklist_classes = true;
    o
}

/// `![alt](C:\My Pictures\a.png)`: CommonMark ends a destination at the first space, so such links only
/// work as `![alt](<C:\My Pictures\a.png>)`. Windows paths often contain spaces, so add the brackets.
static UNBRACKETED_PATH_WITH_SPACES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(!?\[[^\]\r\n]*\]\()([^()<>\r\n"']*?\s[^()<>\r\n"']*?\.(?:png|jpe?g|gif|webp|svg|bmp|ico|avif|apng|jfif|md|markdown|pdf))(\))"#,
    )
    .unwrap()
});

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s{0,3}(```|~~~)").unwrap());

/// URL-bearing attributes in raw HTML.
static URL_ATTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\b(src|href|poster|srcset)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());

fn bracket_paths_with_spaces(markdown: &str) -> String {
    if !markdown.contains("](") {
        return markdown.to_string();
    }
    let mut fence: Option<String> = None;
    let mut out = String::with_capacity(markdown.len() + 16);
    for (i, line) in markdown.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if let Some(m) = FENCE.captures(line) {
            let marker = m[1].to_string();
            match &fence {
                None => fence = Some(marker),
                Some(f) if *f == marker => fence = None,
                _ => {}
            }
            out.push_str(line);
            continue;
        }
        if fence.is_none() && line.contains("](") {
            out.push_str(&UNBRACKETED_PATH_WITH_SPACES.replace_all(line, "$1<$2>$3"));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Above this many source-mapped elements the preview switches to its large-document mode: off-screen
/// blocks skip layout, and math and code highlighting render as they scroll into view. viewer.js
/// applies the same limit after live edits.
pub const LARGE_DOC_ELEMENTS: usize = 5000;

/// Whether rendered HTML is big enough for the preview's large-document mode.
pub fn is_large(html: &str) -> bool {
    html.matches("data-sourcepos=\"").nth(LARGE_DOC_ELEMENTS).is_some()
}

/// Renders Markdown to the HTML that goes inside the viewer's article element.
pub fn render(markdown: &str) -> String {
    let source = bracket_paths_with_spaces(markdown);
    let options = options();
    let arena = Arena::new();
    let root = parse_document(&arena, &source, &options);

    // Absolute Windows paths and file:// URLs cannot load in a web page; point them at the local origin.
    for node in root.descendants() {
        let mut data = node.data.borrow_mut();
        match &mut data.value {
            NodeValue::Link(link) | NodeValue::Image(link) => {
                if let Some(url) = urlmap::rewrite_absolute(&link.url) {
                    link.url = url;
                }
            }
            _ => {}
        }
    }

    let mut html = String::new();
    if format_html(root, &options, &mut html).is_err() {
        return String::from("<p>Render error</p>");
    }
    rewrite_raw_html_paths(&html)
}

fn rewrite_raw_html_paths(html: &str) -> String {
    if !(html.contains("file:") || html.contains(":\\") || html.contains("\\\\") || html.contains("%5C")) {
        return html.to_string();
    }
    URL_ATTR
        .replace_all(html, |c: &Captures| {
            let attr = &c[1];
            let (value, quote) = match (c.get(2), c.get(3)) {
                (Some(v), _) => (v.as_str(), '"'),
                (_, Some(v)) => (v.as_str(), '\''),
                _ => return c[0].to_string(),
            };
            let unescaped = value.replace("&amp;", "&");
            let rewritten = if attr.eq_ignore_ascii_case("srcset") {
                rewrite_srcset(&unescaped)
            } else {
                urlmap::rewrite_absolute(&unescaped)
            };
            match rewritten {
                Some(v) => format!("{attr}={quote}{}{quote}", v.replace('&', "&amp;")),
                None => c[0].to_string(),
            }
        })
        .into_owned()
}

fn rewrite_srcset(srcset: &str) -> Option<String> {
    let mut changed = false;
    let parts: Vec<String> = srcset
        .split(',')
        .map(|candidate| {
            let t = candidate.trim();
            let (url, descriptor) = match t.find([' ', '\t']) {
                Some(i) => (&t[..i], &t[i..]),
                None => (t, ""),
            };
            match urlmap::rewrite_absolute(url) {
                Some(u) => {
                    changed = true;
                    format!("{u}{descriptor}")
                }
                None => t.to_string(),
            }
        })
        .collect();
    changed.then(|| parts.join(", "))
}

/// Plain text of the first heading, used to suggest a file name for untitled documents.
pub fn first_heading(markdown: &str) -> Option<String> {
    markdown
        .lines()
        .map(str::trim_start)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_absolute_image_paths() {
        let html = render(r"![a](<C:\My Pics\a.png>)");
        assert!(html.contains(r#"src="http://mdfile.localhost/C:/My%20Pics/a.png""#), "{html}");
        let html = render(r"![a](C:\My Pics\a.png)");
        assert!(html.contains(r#"src="http://mdfile.localhost/C:/My%20Pics/a.png""#), "{html}");
        let html = render(r#"<img src="C:\pics\b.png" width="20">"#);
        assert!(html.contains(r#"src="http://mdfile.localhost/C:/pics/b.png""#), "{html}");
    }

    #[test]
    fn keeps_relative_paths_and_code() {
        let html = render("![a](img/a.png)\n\n```\n![x](a b.png)\n```\n");
        assert!(html.contains(r#"src="img/a.png""#), "{html}");
        assert!(html.contains("![x](a b.png)"), "{html}");
    }

    #[test]
    fn has_source_positions_and_features() {
        let html = render("# Title\n\n- [ ] task\n\n> [!NOTE]\n> hi\n\n:rocket: $x^2$\n");
        assert!(html.contains("data-sourcepos"), "{html}");
        assert!(html.contains("markdown-alert"), "{html}");
        assert!(html.contains("🚀"), "{html}");
        assert!(html.contains("data-math-style"), "{html}");
        assert!(html.contains("type=\"checkbox\""), "{html}");
    }

    #[test]
    fn large_documents_are_detected() {
        assert!(!is_large(&render(include_str!("../../ui/examples/examples.md"))));
        assert!(!is_large(&render(&"Paragraph.\n\n".repeat(LARGE_DOC_ELEMENTS))));
        assert!(is_large(&render(&"Paragraph.\n\n".repeat(LARGE_DOC_ELEMENTS + 1))));
    }

    #[test]
    fn examples_document_renders() {
        let html = render(include_str!("../../ui/examples/examples.md"));
        for expected in [
            "markdown-alert-caution",
            "data-math-style=\"display\"",
            "language-mermaid",
            "footnote",
            "<table",
            "<dl",
            "id=\"font-size-and-alignment\"",
            "id=\"heading-6\"",
            "id=\"underlined-heading-1\"",
            "id=\"underlined-heading-2\"",
            "start=\"7\"",
            "examples-images/dusk.jpg",
        ] {
            assert!(html.contains(expected), "missing {expected}");
        }
        assert!(html.trim_start().starts_with("<h1"), "front matter is not hidden");
        // Every in-page link points at a heading that exists.
        for anchor in Regex::new(r##"href="#([a-z0-9-]+)""##).unwrap().captures_iter(&html) {
            let id = format!("id=\"{}\"", &anchor[1]);
            assert!(html.contains(&id) || anchor[1].starts_with("fn"), "no heading for #{}", &anchor[1]);
        }
    }
}
