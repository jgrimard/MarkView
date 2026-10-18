//! PDF export: loads the document (with any unsaved edits) in an off-screen window in light theme,
//! waits for diagrams and math to render, then uses WebView2's PrintToPdf.

use crate::{urlmap, PendingPrints};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub async fn export(app: AppHandle, doc_path: String, text: String, out: String) -> Result<(), String> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    let token = format!("{}-{n}", std::process::id());
    app.state::<PendingPrints>().0.lock().unwrap().insert(token.clone(), text);

    let url = format!("{}?pdf={token}", urlmap::path_to_url(Path::new(&doc_path)));
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let label = format!("pdf{n}");

    let out_path = out.clone();
    let tx_load = tx.clone();
    let mut builder = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url.parse().map_err(|e| format!("{e}"))?))
        .title("MarkView PDF export")
        .inner_size(1000.0, 1200.0)
        .position(-32000.0, -32000.0)
        .skip_taskbar(true)
        .focused(false)
        .decorations(false)
        .on_page_load(move |webview, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            let webview = webview.clone();
            let out = out_path.clone();
            let tx = tx_load.clone();
            std::thread::spawn(move || {
                // Mermaid and KaTeX render asynchronously after the load event.
                std::thread::sleep(Duration::from_millis(1500));
                let tx_err = tx.clone();
                if let Err(e) = webview.with_webview(move |platform| print_to_pdf(platform, &out, tx)) {
                    let _ = tx_err.send(Err(e.to_string()));
                }
            });
        });
    if let Some(dir) = crate::package::webview_data_dir() {
        builder = builder.data_directory(dir);
    }
    let window = builder.build().map_err(|e| e.to_string())?;
    drop(tx);

    let result = tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| Err("Timed out exporting PDF".into()))
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = window.destroy();
    app.state::<PendingPrints>().0.lock().unwrap().remove(&token);
    result
}

#[cfg(windows)]
fn print_to_pdf(platform: tauri::webview::PlatformWebview, out: &str, tx: mpsc::Sender<Result<(), String>>) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::PrintToPdfCompletedHandler;
    use windows_core::{Interface, HSTRING};

    let result = (|| -> windows_core::Result<()> {
        unsafe {
            let core = platform.controller().CoreWebView2()?;
            let core7: ICoreWebView2_7 = core.cast()?;
            let env: ICoreWebView2Environment6 = platform.environment().cast()?;
            let settings = env.CreatePrintSettings()?;
            settings.SetShouldPrintBackgrounds(true)?;
            settings.SetShouldPrintHeaderAndFooter(false)?;
            settings.SetMarginTop(0.6)?;
            settings.SetMarginBottom(0.6)?;
            settings.SetMarginLeft(0.6)?;
            settings.SetMarginRight(0.6)?;
            let done = tx.clone();
            let handler = PrintToPdfCompletedHandler::create(Box::new(move |status, ok| {
                let _ = done.send(match (status, ok) {
                    (Ok(()), true) => Ok(()),
                    (Err(e), _) => Err(e.message()),
                    _ => Err("PDF export failed".to_string()),
                });
                Ok(())
            }));
            core7.PrintToPdf(&HSTRING::from(out), &settings, &handler)
        }
    })();
    if let Err(e) = result {
        let _ = tx.send(Err(e.message()));
    }
}

#[cfg(not(windows))]
fn print_to_pdf(_: tauri::webview::PlatformWebview, _: &str, tx: mpsc::Sender<Result<(), String>>) {
    let _ = tx.send(Err("PDF export is only supported on Windows".into()));
}
