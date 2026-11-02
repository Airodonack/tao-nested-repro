//! The app's backend: a thread that emits a tick to the page every 100 ms,
//! a command that answers with the time, one that asks the window its size
//! first, and the dialogs that start a nested
//! GLib main loop on the main thread.
//!
//! On Linux, tao reads the events sent to it from other threads (emits,
//! window calls, `run_on_main_thread`) only between its own GTK
//! iterations. While a dialog's nested loop runs, they wait, and arrive all
//! at once when it ends. The "sleep" command is the control: it blocks a
//! thread, not the main loop, and nothing waits.
//!
//! For unattended runs, `REPRO_AUTO=<trigger>` makes the page press that
//! trigger 2 s after it loads, `REPRO_AUTO_CLOSE_MS` closes any open dialog
//! after that many milliseconds, and `REPRO_LOG_FILE` is where the page's
//! log is written before the app quits.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

/// The round trip: the page measures how long this answer takes.
#[tauri::command]
async fn ask_backend() -> f64 {
    now_ms()
}

/// A window query from a thread that isn't the main one: tao answers it from
/// the same event queue the ticks go through.
#[tauri::command]
async fn ask_window(window: WebviewWindow) -> Result<f64, String> {
    window.inner_size().map_err(|e| e.to_string())?;
    Ok(now_ms())
}

/// The control: blocks a worker thread for 5 s, not the main loop.
#[tauri::command]
async fn sleep_on_thread() {
    let _ =
        tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_secs(5))).await;
}

#[tauri::command]
async fn plugin_message(app: AppHandle) -> String {
    arm_auto_close(&app);
    let ok = app
        .dialog()
        .message("While this is open, watch the backend ticks and the round trip.")
        .title("Dialog plugin message")
        .blocking_show();
    format!("answered {ok}")
}

#[tauri::command]
async fn plugin_file(app: AppHandle) -> String {
    arm_auto_close(&app);
    match app.dialog().file().blocking_pick_file() {
        Some(path) => format!("picked {path}"),
        None => "cancelled".into(),
    }
}

#[tauri::command]
async fn raw_message(window: WebviewWindow) -> Result<String, String> {
    run_raw(window, RawKind::Message).await
}

#[tauri::command]
async fn raw_file(window: WebviewWindow) -> Result<String, String> {
    run_raw(window, RawKind::File).await
}

#[tauri::command]
fn auto_mode() -> Option<String> {
    std::env::var("REPRO_AUTO").ok().filter(|v| !v.is_empty())
}

/// Ends an unattended run: writes the page's log and quits.
#[tauri::command]
fn finish(app: AppHandle, log: String) {
    if let Ok(path) = std::env::var("REPRO_LOG_FILE") {
        let _ = std::fs::write(path, log);
    }
    app.exit(0);
}

enum RawKind {
    Message,
    File,
}

#[cfg(target_os = "linux")]
async fn run_raw(window: WebviewWindow, kind: RawKind) -> Result<String, String> {
    use gtk::prelude::*;

    let (tx, rx) = std::sync::mpsc::channel();
    let parent = window.clone();
    window
        .run_on_main_thread(move || {
            let parent = parent.gtk_window().ok();
            let dialog: gtk::Dialog = match kind {
                RawKind::Message => gtk::MessageDialog::new(
                    parent.as_ref(),
                    gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
                    gtk::MessageType::Info,
                    gtk::ButtonsType::Ok,
                    "gtk_dialog_run: while this is open, watch the backend ticks and the round trip.",
                )
                .upcast(),
                RawKind::File => gtk::FileChooserDialog::with_buttons(
                    Some("gtk_dialog_run on a file chooser"),
                    parent.as_ref(),
                    gtk::FileChooserAction::Open,
                    &[
                        ("Cancel", gtk::ResponseType::Cancel),
                        ("Open", gtk::ResponseType::Accept),
                    ],
                )
                .upcast(),
            };
            schedule_auto_close();
            // The nested main loop: it runs until the dialog answers.
            let response = dialog.run();
            dialog.close();
            let _ = tx.send(format!("answered {response:?}"));
        })
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
async fn run_raw(_window: WebviewWindow, _kind: RawKind) -> Result<String, String> {
    Err("the raw GTK dialogs exist on Linux only".into())
}

fn auto_close_ms() -> Option<u64> {
    std::env::var("REPRO_AUTO_CLOSE_MS").ok()?.parse().ok()
}

/// Arms the auto close from a thread that isn't the main one.
fn arm_auto_close(app: &AppHandle) {
    #[cfg(target_os = "linux")]
    if auto_close_ms().is_some() {
        let _ = app.run_on_main_thread(schedule_auto_close);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
}

/// On the main thread: a GLib timeout, which runs inside the nested loop too,
/// answers every open dialog with "closed" once the time is up.
#[cfg(target_os = "linux")]
fn schedule_auto_close() {
    use gtk::prelude::*;

    let Some(ms) = auto_close_ms() else { return };
    gtk::glib::timeout_add_local_once(Duration::from_millis(ms), || {
        for window in gtk::Window::list_toplevels() {
            if let Ok(dialog) = window.downcast::<gtk::Dialog>() {
                if dialog.is_visible() {
                    dialog.response(gtk::ResponseType::DeleteEvent);
                }
            }
        }
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut n: u64 = 0;
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                    n += 1;
                    let _ = handle.emit("tick", n);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ask_backend,
            ask_window,
            sleep_on_thread,
            plugin_message,
            plugin_file,
            raw_message,
            raw_file,
            auto_mode,
            finish
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
