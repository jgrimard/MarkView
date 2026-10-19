//! Per-user (HKCU) registration so MarkView appears under "Open with" for Markdown files and in
//! Settings > Default apps. Windows does not let an app make itself the default; the user picks it.

#[cfg(windows)]
mod imp {
    use winreg::enums::*;
    use winreg::{RegKey, RegValue};

    const PROG_ID: &str = "MarkView.md";
    const APP_NAME: &str = "MarkView";
    const CAPABILITIES: &str = r"Software\MarkView\Capabilities";
    const EXTENSIONS: &[&str] = &[".md", ".markdown", ".mdown", ".mkd", ".mkdn"];

    fn exe() -> String {
        std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
    }

    /// The program the current registration points at, if MarkView is registered at all.
    pub fn registered_exe() -> Option<String> {
        let command: String = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(format!(r"Software\Classes\{PROG_ID}\shell\open\command"))
            .and_then(|k| k.get_value(""))
            .ok()?;
        super::exe_from_command(&command)
    }

    /// Registered, and pointing at this exe (not a copy that was moved or deleted).
    pub fn is_registered() -> bool {
        registered_exe().is_some_and(|r| r.eq_ignore_ascii_case(&exe()))
    }

    /// `file_type` and `description` are shown by Explorer and Settings, in the UI language.
    pub fn register(file_type: &str, description: &str) -> std::io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let exe = exe();
        let command = format!("\"{exe}\" \"%1\"");
        let icon = format!("\"{exe}\",0");

        let (prog, _) = hkcu.create_subkey(format!(r"Software\Classes\{PROG_ID}"))?;
        prog.set_value("", &file_type)?;
        prog.set_value("FriendlyTypeName", &file_type)?;
        prog.create_subkey("DefaultIcon")?.0.set_value("", &icon)?;
        prog.create_subkey(r"shell\open\command")?.0.set_value("", &command)?;

        let (app, _) = hkcu.create_subkey(r"Software\Classes\Applications\markview.exe")?;
        app.set_value("FriendlyAppName", &APP_NAME)?;
        app.create_subkey(r"shell\open\command")?.0.set_value("", &command)?;
        let (types, _) = app.create_subkey("SupportedTypes")?;
        for ext in EXTENSIONS {
            types.set_value(ext, &"")?;
        }

        let (caps, _) = hkcu.create_subkey(CAPABILITIES)?;
        caps.set_value("ApplicationName", &APP_NAME)?;
        caps.set_value("ApplicationDescription", &description)?;
        caps.set_value("ApplicationIcon", &icon)?;
        let (assoc, _) = caps.create_subkey("FileAssociations")?;
        for ext in EXTENSIONS {
            assoc.set_value(ext, &PROG_ID)?;
        }
        hkcu.create_subkey(r"Software\RegisteredApplications")?
            .0
            .set_value(APP_NAME, &CAPABILITIES)?;

        for ext in EXTENSIONS {
            let (open_with, _) = hkcu.create_subkey(format!(r"Software\Classes\{ext}\OpenWithProgids"))?;
            open_with.set_raw_value(PROG_ID, &RegValue { vtype: REG_NONE, bytes: Vec::new().into() })?;
        }
        notify_shell();
        Ok(())
    }

    pub fn unregister() -> std::io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let _ = hkcu.delete_subkey_all(format!(r"Software\Classes\{PROG_ID}"));
        let _ = hkcu.delete_subkey_all(r"Software\Classes\Applications\markview.exe");
        let _ = hkcu.delete_subkey_all(r"Software\MarkView");
        if let Ok(k) = hkcu.open_subkey_with_flags(r"Software\RegisteredApplications", KEY_SET_VALUE) {
            let _ = k.delete_value(APP_NAME);
        }
        for ext in EXTENSIONS {
            if let Ok(k) = hkcu.open_subkey_with_flags(format!(r"Software\Classes\{ext}\OpenWithProgids"), KEY_SET_VALUE) {
                let _ = k.delete_value(PROG_ID);
            }
        }
        notify_shell();
        Ok(())
    }

    fn notify_shell() {
        use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
}

#[cfg(windows)]
pub use imp::*;

/// The program in a shell open command such as `"C:\path\markview.exe" "%1"`.
fn exe_from_command(command: &str) -> Option<String> {
    let command = command.trim_start();
    let exe = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command.split(' ').next()?,
    };
    (!exe.is_empty()).then(|| exe.to_string())
}

#[cfg(test)]
mod tests {
    use super::exe_from_command;

    #[test]
    fn parses_open_commands() {
        assert_eq!(
            exe_from_command(r#""C:\Program Files\MarkView\markview.exe" "%1""#).as_deref(),
            Some(r"C:\Program Files\MarkView\markview.exe")
        );
        assert_eq!(exe_from_command(r"C:\Tools\markview.exe %1").as_deref(), Some(r"C:\Tools\markview.exe"));
        assert_eq!(exe_from_command(""), None);
    }
}

#[cfg(not(windows))]
pub fn is_registered() -> bool {
    false
}
#[cfg(not(windows))]
pub fn registered_exe() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn register(_file_type: &str, _description: &str) -> std::io::Result<()> {
    Ok(())
}
#[cfg(not(windows))]
pub fn unregister() -> std::io::Result<()> {
    Ok(())
}
