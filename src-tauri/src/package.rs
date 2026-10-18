//! Whether MarkView runs from an MSIX package (the Microsoft Store build), and that package's identity.
//! A packaged MarkView gets its .md association from the package manifest, not from the registry.

#[cfg(windows)]
mod imp {
    use ::windows::core::PWSTR;
    use ::windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, WIN32_ERROR};
    use ::windows::Win32::Storage::Packaging::Appx::{GetCurrentApplicationUserModelId, GetCurrentPackageFamilyName};
    use std::sync::OnceLock;

    type Query = unsafe fn(*mut u32, Option<PWSTR>) -> WIN32_ERROR;

    fn query(get: Query) -> Option<String> {
        let mut len = 0u32;
        // Unpackaged processes get APPMODEL_ERROR_NO_PACKAGE; packaged ones learn the buffer size.
        if unsafe { get(&mut len, None) } != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if unsafe { get(&mut len, Some(PWSTR(buf.as_mut_ptr()))) } != ERROR_SUCCESS {
            return None;
        }
        // `len` counts the terminating NUL.
        Some(String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]))
    }

    /// e.g. `jgrimard.388874D7F8808_wgz7aqx4kv1ge`, or None when not running from a package.
    pub fn family_name() -> Option<&'static str> {
        static FAMILY: OnceLock<Option<String>> = OnceLock::new();
        FAMILY.get_or_init(|| query(GetCurrentPackageFamilyName)).as_deref()
    }

    /// `<family name>!<application id>`, which Settings > Default apps uses to find a packaged app.
    pub fn app_user_model_id() -> Option<String> {
        query(GetCurrentApplicationUserModelId)
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn family_name() -> Option<&'static str> {
        None
    }

    pub fn app_user_model_id() -> Option<String> {
        None
    }
}

pub use imp::*;

pub fn is_packaged() -> bool {
    family_name().is_some()
}

/// The Store build's WebView2 profile, inside its package data so uninstalling removes it and it is never
/// shared with a standalone MarkView.exe. None for the standalone exe, which keeps Tauri's default.
pub fn webview_data_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(std::path::PathBuf::from(base).join("Packages").join(family_name()?).join("LocalCache").join("WebView2"))
}
