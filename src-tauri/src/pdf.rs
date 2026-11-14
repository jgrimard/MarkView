//! PDF export: loads the document (with any unsaved edits) in an off-screen window in light theme,
//! waits until the page says its diagrams and math are rendered, then uses WebView2's PrintToPdf.

use crate::{urlmap, PendingPrints};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// How long a page may take to render its diagrams and math before it is printed as it is.
const RENDER_TIMEOUT: Duration = Duration::from_secs(45);
/// For the whole export: loading, rendering and printing.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(120);

pub async fn export(app: AppHandle, doc_path: String, text: String, out: String) -> Result<(), String> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    let token = format!("{}-{n}", std::process::id());
    let url = format!("{}?pdf={token}", urlmap::path_to_url(Path::new(&doc_path)));
    let url = url.parse().map_err(|e| format!("{e}"))?;
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let label = format!("pdf{n}");

    app.state::<PendingPrints>().0.lock().unwrap().insert(token.clone(), text);
    let forget_text = || {
        app.state::<PendingPrints>().0.lock().unwrap().remove(&token);
    };

    let out_path = out.clone();
    let tx_load = tx.clone();
    let failed = crate::i18n::t(&app, "banner.pdfFailed");
    let printing = AtomicBool::new(false);
    let mut builder = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title("MarkView PDF export")
        .inner_size(1000.0, 1200.0)
        .position(-32000.0, -32000.0)
        .skip_taskbar(true)
        .focused(false)
        .decorations(false)
        .on_page_load(move |webview, payload| {
            // Once per export, should the page ever load twice.
            if payload.event() != PageLoadEvent::Finished || printing.swap(true, Ordering::SeqCst) {
                return;
            }
            let webview = webview.clone();
            let out = out_path.clone();
            let tx = tx_load.clone();
            let failed = failed.clone();
            std::thread::spawn(move || {
                wait_until_rendered(&webview);
                let tx_err = tx.clone();
                if let Err(e) = webview.with_webview(move |platform| print_to_pdf(platform, &out, failed, tx)) {
                    let _ = tx_err.send(Err(e.to_string()));
                }
            });
        });
    if let Some(dir) = crate::package::webview_data_dir() {
        builder = builder.data_directory(dir);
    }
    let window = match builder.build() {
        Ok(window) => window,
        Err(e) => {
            forget_text();
            return Err(e.to_string());
        }
    };
    drop(tx);

    let timeout = crate::i18n::t(&app, "error.pdfTimeout");
    let result = tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(EXPORT_TIMEOUT).unwrap_or(Err(timeout)))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));

    let _ = window.destroy();
    forget_text();
    result
}

/// Mermaid and KaTeX render after the load event, and a document full of diagrams takes a while;
/// viewer.js sets `data-ready` on the body once everything is in place.
fn wait_until_rendered(webview: &WebviewWindow) {
    let deadline = Instant::now() + RENDER_TIMEOUT;
    while Instant::now() < deadline {
        let (tx, rx) = mpsc::channel();
        let asked = webview.eval_with_callback("document.body.dataset.ready === '1'", move |result| {
            let _ = tx.send(result.trim() == "true");
        });
        if asked.is_err() {
            return; // the window is gone
        }
        if rx.recv_timeout(Duration::from_secs(5)) == Ok(true) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // One more beat, for the layout of what was just inserted.
    std::thread::sleep(Duration::from_millis(200));
}

#[cfg(windows)]
fn print_to_pdf(platform: tauri::webview::PlatformWebview, out: &str, failed: String, tx: mpsc::Sender<Result<(), String>>) {
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
                    _ => Err(failed.clone()),
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
fn print_to_pdf(_: tauri::webview::PlatformWebview, _: &str, _: String, tx: mpsc::Sender<Result<(), String>>) {
    let _ = tx.send(Err("PDF export is only supported on Windows".into()));
}
