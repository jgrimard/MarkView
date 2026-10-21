//! Panic hunt over everything that handles document text, link targets and file names. A panic in
//! the app is fatal (panic = "abort"), and the 1.2.0 crash came from slicing a link target inside a
//! multi-byte character, so inputs mix Markdown and HTML syntax, paths and URL escapes with 1-4 byte
//! characters. Run it with:
//!
//!     cargo test --release fuzz -- --ignored --nocapture
//!
//! FUZZ_ITERS sets the number of generated inputs (default 200000), FUZZ_SEED the starting seed.

use crate::{render, textfile, urlmap};
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

const PIECES: &[&str] = &[
    // Markdown
    "[", "]", "(", ")", "![", "](", "<", ">", "`", "```", "$", "$$", "|", "---", "*", "_", "~~", "#", "## ", "> ",
    "- [ ] ", "- [x] ", "1. ", "[^1]", "[^1]: ", ":::", "\\", "&", "&amp;", "&#", "\n", "\r\n", "\n\n", "\t", " ",
    "    ", "> [!NOTE]\n", ":smile:", "---\ntitle: x\n---\n",
    // HTML
    "<img src=\"", "<img src='", "\" srcset=\"", " 2x, ", "<a href=\"", "\">", "'>", "</a>", "<video poster=\"",
    "<source src=\"", "<!--", "-->", "<details><summary>",
    // paths and URLs
    "file:", "file://", "file:///", "FILE:", "C:", "c:\\", "C:/", "\\\\", "\\\\server\\share\\", "/unc/", "..\\", "../",
    "http://mdfile.localhost/", "http://", "https://", "mailto:", "%", "%2", "%20", "%5C", "%3A", "%E7%94%BB", "%FF",
    "%C3", "%E2%82", "#", "?", "?v=1", "&pdf=", "/__app/", "/__examples/", "/__untitled/", ".md", ".png", " ", "a",
    "abc", "0", "画像", "写真/a.png", "#概要", "fiché", "файл", "żółw", "Größe", "𠮷", "😀", "👨‍👩‍👧", "か\u{3099}",
    "葛\u{E0100}", "\u{FFFD}", "\u{FEFF}", "\u{0}", "\u{200B}", "\u{2028}", "é", "\u{301}", "ı", "İ", "ß", "ﬁ", "Ω",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn text(&mut self, max_pieces: usize) -> String {
        let n = self.below(max_pieces) + 1;
        let mut s = String::new();
        for _ in 0..n {
            if self.below(8) == 0 {
                // a random scalar value, any plane
                let c = char::from_u32(self.next() as u32 % 0x11_0000).unwrap_or('\u{FFFD}');
                s.push(c);
            } else {
                s.push_str(PIECES[self.below(PIECES.len())]);
            }
        }
        s
    }
    fn bytes(&mut self) -> Vec<u8> {
        let mut b: Vec<u8> = self.text(20).into_bytes();
        for _ in 0..self.below(6) {
            if !b.is_empty() {
                let i = self.below(b.len());
                b[i] = self.next() as u8; // break the UTF-8 here and there
            }
        }
        b
    }
}

/// (location, message) of the panic just caught.
static LAST_PANIC: Mutex<Option<(String, String)>> = Mutex::new(None);

#[test]
#[ignore]
fn fuzz() {
    let iters: usize = std::env::var("FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    let seed: u64 = std::env::var("FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5EED);
    let mut rng = Rng(seed | 1);

    let quiet = panic::take_hook();
    panic::set_hook(Box::new(|info| {
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let message = info
            .payload()
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        *LAST_PANIC.lock().unwrap() = Some((place, message));
    }));

    let encodings = ["UTF-8", "UTF-16LE", "UTF-16BE", "Shift_JIS", "EUC-JP", "GBK", "Big5", "EUC-KR", "windows-1252", "bogus"];
    // panic location -> (count, target, first message, sample input)
    let mut found: BTreeMap<String, (usize, &str, String, String)> = BTreeMap::new();

    for i in 0..iters {
        if i % (iters / 10).max(1) == 0 {
            println!("[{i}/{iters}] {} distinct panics so far", found.len());
        }
        let doc = rng.text(60);
        let short = rng.text(6);
        let bytes = rng.bytes();
        let encoding = encodings[rng.below(encodings.len())];
        let bom = rng.below(2) == 0;

        let targets: [(&str, &dyn Fn()); 10] = [
            ("render", &|| drop(render::render(&doc))),
            ("first_heading", &|| drop(render::first_heading(&doc))),
            ("rewrite_absolute", &|| drop(urlmap::rewrite_absolute(&short))),
            ("url_to_path", &|| drop(urlmap::url_to_path(&short))),
            ("url_path_to_fs", &|| drop(urlmap::url_path_to_fs(&short))),
            ("path_to_url", &|| drop(urlmap::url_to_path(&urlmap::path_to_url(Path::new(&short))))),
            ("decode", &|| drop(textfile::decode(&bytes))),
            ("encode", &|| drop(textfile::encode(&doc, encoding, bom))),
            ("link_for_file", &|| drop(crate::link_for_file(Some(format!(r"C:\docs\{short}\a.md")), format!(r"C:\{doc}")))),
            ("is_large", &|| {
                let _ = render::is_large(&doc);
            }),
        ];
        for (name, target) in targets {
            if panic::catch_unwind(AssertUnwindSafe(target)).is_err() {
                let (place, message) = LAST_PANIC.lock().unwrap().take().unwrap_or_default();
                let sample = match name {
                    "decode" => format!("{bytes:?}"),
                    "rewrite_absolute" | "url_to_path" | "url_path_to_fs" | "path_to_url" => short.clone(),
                    _ => doc.clone(),
                };
                found.entry(place).or_insert((0, name, message, sample)).0 += 1;
            }
        }
    }

    panic::set_hook(quiet);
    println!("[{iters}/{iters}] done: {} distinct panics", found.len());
    for (place, (count, name, message, sample)) in &found {
        let sample: String = format!("{sample:?}").chars().take(300).collect();
        println!("\n{place} ({name}, {count} inputs): {message}\n  sample: {sample}");
    }
    assert!(found.is_empty(), "{} distinct panics", found.len());
}
