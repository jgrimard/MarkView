//! UI languages. String tables live in ui/locales/<id>.json (en.json is the source of truth) and are
//! shared by the shell, the preview pages and the few strings Rust shows itself (dialog filters, errors).

use crate::settings::SettingsStore;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager};

/// Language id (also the HTML `lang` value) and its name in that language.
pub const SUPPORTED: &[(&str, &str)] = &[
    ("de", "Deutsch"),
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("pt-BR", "Português (Brasil)"),
    ("tr", "Türkçe"),
    ("ru", "Русский"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("zh-Hans", "中文(简体)"),
];

/// Picks the first supported language from BCP 47 tags in preference order, or English.
pub fn match_language<S: AsRef<str>>(tags: &[S]) -> &'static str {
    for tag in tags {
        let tag = tag.as_ref().to_ascii_lowercase();
        if let Some((id, _)) = SUPPORTED.iter().find(|(id, _)| id.to_ascii_lowercase() == tag) {
            return id;
        }
        let mut parts = tag.split('-');
        let base = parts.next().unwrap_or_default();
        if base == "zh" {
            // Only Simplified Chinese is translated; Traditional (TW, HK, MO, Hant) falls through.
            let rest: Vec<&str> = parts.collect();
            let traditional = rest.iter().any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo"));
            if !traditional {
                return "zh-Hans";
            }
            continue;
        }
        if base == "pt" {
            return "pt-BR";
        }
        if let Some((id, _)) = SUPPORTED.iter().find(|(id, _)| *id == base) {
            return id;
        }
    }
    "en"
}

/// The Windows display languages, most preferred first.
#[cfg(windows)]
fn preferred_ui_languages() -> Vec<String> {
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    use windows_core::PWSTR;
    let mut count = 0u32;
    let mut len = 0u32;
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len).is_err() || len == 0 {
            return Vec::new();
        }
        let mut buffer = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, Some(PWSTR(buffer.as_mut_ptr())), &mut len).is_err() {
            return Vec::new();
        }
        // A list of NUL-terminated names ending with an empty one.
        buffer
            .split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

#[cfg(not(windows))]
fn preferred_ui_languages() -> Vec<String> {
    Vec::new()
}

pub fn system_language() -> &'static str {
    match_language(&preferred_ui_languages())
}

/// The language the UI shows: the user's choice, or the Windows display language for "system".
pub fn effective(setting: &str) -> &'static str {
    SUPPORTED
        .iter()
        .find(|(id, _)| *id == setting)
        .map(|(id, _)| *id)
        .unwrap_or_else(system_language)
}

pub fn current(app: &AppHandle) -> &'static str {
    effective(&app.state::<SettingsStore>().get().language)
}

fn load_table(app: &AppHandle, lang: &str) -> Map<String, Value> {
    app.asset_resolver()
        .get(format!("locales/{lang}.json"))
        .and_then(|a| serde_json::from_slice::<Value>(&a.bytes).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// The strings for a language, with English filling any gaps.
pub fn strings(app: &AppHandle, lang: &str) -> Arc<Map<String, Value>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Map<String, Value>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(table) = cache.lock().unwrap().get(lang) {
        return table.clone();
    }
    let mut table = load_table(app, "en");
    if lang != "en" {
        table.extend(load_table(app, lang));
    }
    let table = Arc::new(table);
    cache.lock().unwrap().insert(lang.to_string(), table.clone());
    table
}

/// One string in the current UI language.
pub fn t(app: &AppHandle, key: &str) -> String {
    strings(app, current(app))
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn locales_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/locales")
    }

    fn read(lang: &str) -> Map<String, Value> {
        let text = std::fs::read_to_string(locales_dir().join(format!("{lang}.json")))
            .unwrap_or_else(|e| panic!("{lang}.json: {e}"));
        match serde_json::from_str(&text).unwrap_or_else(|e| panic!("{lang}.json: {e}")) {
            Value::Object(m) => m,
            _ => panic!("{lang}.json is not an object"),
        }
    }

    fn placeholders(s: &str) -> BTreeSet<String> {
        let re = regex::Regex::new(r"\{\w+\}|\$").unwrap();
        re.find_iter(s).map(|m| m.as_str().to_string()).collect()
    }

    #[test]
    fn every_locale_matches_english() {
        let en = read("en");
        for (lang, _) in SUPPORTED {
            let table = read(lang);
            let missing: Vec<_> = en.keys().filter(|k| !table.contains_key(*k)).collect();
            let extra: Vec<_> = table.keys().filter(|k| !en.contains_key(*k)).collect();
            assert!(missing.is_empty(), "{lang} is missing {missing:?}");
            assert!(extra.is_empty(), "{lang} has unknown keys {extra:?}");
            for (key, value) in &table {
                let text = value.as_str().unwrap_or_else(|| panic!("{lang}: {key} is not a string"));
                assert!(!text.trim().is_empty(), "{lang}: {key} is empty");
                let expected = placeholders(en[key].as_str().unwrap());
                assert_eq!(placeholders(text), expected, "{lang}: {key} placeholders");
            }
        }
    }

    #[test]
    fn no_stray_locale_files() {
        for entry in std::fs::read_dir(locales_dir()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            let id = name.trim_end_matches(".json");
            assert!(SUPPORTED.iter().any(|(s, _)| *s == id), "{name} is not in SUPPORTED");
        }
    }

    #[test]
    fn matches_windows_languages() {
        assert_eq!(match_language(&["fr-CA"]), "fr");
        assert_eq!(match_language(&["de-DE"]), "de");
        assert_eq!(match_language(&["pt-PT"]), "pt-BR");
        assert_eq!(match_language(&["pt-BR"]), "pt-BR");
        assert_eq!(match_language(&["zh-CN"]), "zh-Hans");
        assert_eq!(match_language(&["zh-Hans-SG"]), "zh-Hans");
        assert_eq!(match_language(&["zh-TW"]), "en");
        assert_eq!(match_language(&["zh-Hant-HK", "ja-JP"]), "ja");
        assert_eq!(match_language(&["xx-YY", "ko-KR"]), "ko");
        assert_eq!(match_language(&["xx"]), "en");
        assert_eq!(match_language::<&str>(&[]), "en");
    }
}
