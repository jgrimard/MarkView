//! Document text in the encoding it was saved with. UTF-8 (with or without BOM) and UTF-16 (with BOM)
//! are recognized exactly; anything else is a legacy code page (Shift_JIS, GBK, windows-1252, ...) guessed
//! by chardetng, the detector Firefox uses. Saving writes the same encoding back.

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use std::path::Path;

pub struct TextFile {
    pub text: String,
    /// encoding_rs name, e.g. "UTF-8", "UTF-16LE", "Shift_JIS".
    pub encoding: &'static str,
    pub bom: bool,
}

/// `expected`: the encoding this file had when the document was last read, if it is being read again.
pub fn read(path: &Path, expected: Option<&str>) -> std::io::Result<TextFile> {
    Ok(decode_as(&std::fs::read(path)?, expected))
}

#[cfg(test)]
pub fn decode(bytes: &[u8]) -> TextFile {
    decode_as(bytes, None)
}

/// A legacy-encoded file that is read again (after a save, or a change on disk) keeps its encoding as
/// long as its bytes are valid in it. Guessing afresh can give a different answer once the text has
/// changed, and the document would come back garbled.
pub fn decode_as(bytes: &[u8], expected: Option<&str>) -> TextFile {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return TextFile { text: text.into_owned(), encoding: encoding.name(), bom: true };
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return TextFile { text: text.to_string(), encoding: UTF_8.name(), bom: false };
    }
    let expected = expected
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .filter(|e| ![UTF_8, UTF_16LE, UTF_16BE].contains(e));
    if let Some(encoding) = expected {
        let (text, malformed) = encoding.decode_without_bom_handling(bytes);
        if !malformed {
            return TextFile { text: text.into_owned(), encoding: encoding.name(), bom: false };
        }
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(system_tld(), false);
    let (text, _) = encoding.decode_without_bom_handling(bytes);
    TextFile { text: text.into_owned(), encoding: encoding.name(), bom: false }
}

/// Text can't be saved in its encoding without losing characters.
#[derive(Debug, PartialEq)]
pub struct Unmappable;

pub fn encode(text: &str, encoding: &str, bom: bool) -> Result<Vec<u8>, Unmappable> {
    let encoding = Encoding::for_label(encoding.as_bytes()).unwrap_or(UTF_8);
    if encoding == UTF_16LE || encoding == UTF_16BE {
        let mut bytes = Vec::with_capacity(2 + text.len() * 2);
        for unit in std::iter::once(0xFEFF).chain(text.encode_utf16()) {
            bytes.extend_from_slice(&if encoding == UTF_16LE { unit.to_le_bytes() } else { unit.to_be_bytes() });
        }
        return Ok(bytes);
    }
    if encoding == UTF_8 {
        let mut bytes = Vec::with_capacity(text.len() + 3);
        if bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        bytes.extend_from_slice(text.as_bytes());
        return Ok(bytes);
    }
    // encoding_rs writes characters the encoding lacks as &#NNNN; - refuse instead.
    let (bytes, _, unmappable) = encoding.encode(text);
    if unmappable { Err(Unmappable) } else { Ok(bytes.into_owned()) }
}

/// Country hint for the detector from the Windows ANSI code page, so a short Japanese file on a
/// Japanese PC is read as Shift_JIS rather than, say, windows-1252.
#[cfg(windows)]
fn system_tld() -> Option<&'static [u8]> {
    let tld: &[u8] = match unsafe { windows::Win32::Globalization::GetACP() } {
        932 => b"jp",
        936 => b"cn",
        949 => b"kr",
        950 => b"tw",
        874 => b"th",
        1250 => b"pl",
        1251 => b"ru",
        1253 => b"gr",
        1254 => b"tr",
        1255 => b"il",
        1256 => b"sa",
        1257 => b"lt",
        1258 => b"vn",
        _ => return None,
    };
    Some(tld)
}

#[cfg(not(windows))]
fn system_tld() -> Option<&'static [u8]> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str, encoding: &'static Encoding) {
        let (bytes, _, unmappable) = encoding.encode(text);
        assert!(!unmappable);
        let file = decode(&bytes);
        assert_eq!(file.encoding, encoding.name(), "{text}");
        assert_eq!(file.text, text);
        assert_eq!(encode(&file.text, file.encoding, file.bom).unwrap(), bytes.as_ref());
    }

    #[test]
    fn legacy_encodings_are_detected_and_saved_back() {
        round_trip("# 日本語の文書\n\nこれはShift-JISで保存されたファイルです。漢字とカタカナ。\n", encoding_rs::SHIFT_JIS);
        round_trip("# 中文文档\n\n这是一个用 GBK 编码保存的 Markdown 文件，包含中文字符。\n", encoding_rs::GBK);
        round_trip("# 한국어 문서\n\n이 파일은 EUC-KR 인코딩으로 저장된 마크다운 문서입니다.\n", encoding_rs::EUC_KR);
        round_trip("# Заметки\n\nЭтот файл сохранён в кодировке Windows-1251 со словами на русском.\n", encoding_rs::WINDOWS_1251);
        round_trip("# Café notes\n\nRésumé, naïve, über, señor: accented words in an ANSI file.\n", encoding_rs::WINDOWS_1252);
    }

    #[test]
    fn unicode_files_keep_their_form() {
        let utf8 = "# 見出し\r\n本文\r\n";
        let file = decode(utf8.as_bytes());
        assert_eq!((file.encoding, file.bom, file.text.as_str()), ("UTF-8", false, utf8));

        let with_bom = [&[0xEF, 0xBB, 0xBF][..], utf8.as_bytes()].concat();
        let file = decode(&with_bom);
        assert_eq!((file.encoding, file.bom, file.text.as_str()), ("UTF-8", true, utf8));
        assert_eq!(encode(&file.text, file.encoding, file.bom).unwrap(), with_bom);

        let utf16: Vec<u8> = std::iter::once(0xFEFFu16).chain("𠮷 notes".encode_utf16()).flat_map(u16::to_le_bytes).collect();
        let file = decode(&utf16);
        assert_eq!((file.encoding, file.text.as_str()), ("UTF-16LE", "𠮷 notes"));
        assert_eq!(encode(&file.text, file.encoding, file.bom).unwrap(), utf16);
    }

    #[test]
    fn a_reread_file_keeps_its_encoding() {
        // Too short to guess: on its own this is not read as windows-1251.
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("Да");
        let file = decode_as(&bytes, Some("windows-1251"));
        assert_eq!((file.encoding, file.text.as_str()), ("windows-1251", "Да"));
        // Converted to UTF-8 by another program in the meantime: that wins.
        let file = decode_as("Да".as_bytes(), Some("windows-1251"));
        assert_eq!((file.encoding, file.text.as_str()), ("UTF-8", "Да"));
        // Bytes the expected encoding can't hold: guessed as usual.
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("これはShift-JISで保存されたファイルです。");
        assert_eq!(decode_as(&sjis, Some("EUC-KR")).encoding, decode(&sjis).encoding);
        assert_eq!(decode_as(&sjis, Some("no such encoding")).encoding, "Shift_JIS");
    }

    #[test]
    fn characters_the_encoding_lacks_are_refused() {
        assert_eq!(encode("日本語 😀", "Shift_JIS", false), Err(Unmappable));
        assert!(encode("日本語", "Shift_JIS", false).is_ok());
    }
}
