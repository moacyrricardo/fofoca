//! Export a meeting to a folder (spec 023): the dialog's commands over `fofoca_core::export`.
//!
//! The export runs on its own thread with `export` progress events, like a model download — not in
//! the GPU job queue, so it works while another meeting is recording.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fofoca_core::config::{config_path, Config};
use fofoca_core::export::{self, ExportOptions, ExportPlan, ExportStep};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{err, AppState, CmdResult};

#[derive(Default)]
pub struct ExportState {
    /// The running export's cancel flag; one export at a time.
    running: Mutex<Option<Arc<AtomicBool>>>,
    /// Folders exported this session: the only ones "Open folder" opens.
    exported: Mutex<Vec<PathBuf>>,
}

#[derive(Serialize, Clone)]
struct ExportEvent {
    meeting_id: String,
    /// `running` | `done` | `failed` | `cancelled`
    state: &'static str,
    step: Option<ExportStep>,
    file: String,
    done: u64,
    total: u64,
    folder: Option<String>,
    zip: Option<String>,
    error: Option<String>,
}

/// Refuse the meeting being recorded (its WAVs are still growing) and one being processed.
pub(crate) fn check_export(id: &str, recording_id: Option<&str>, processing: bool) -> CmdResult<()> {
    if recording_id == Some(id) {
        return Err("This meeting is still recording".into());
    }
    if processing {
        return Err("This meeting is being processed — export it when that's done".into());
    }
    Ok(())
}

/// Native folder picker, starting at `start`.
#[tauri::command]
pub async fn pick_export_folder(app: AppHandle, start: Option<String>) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        let mut d = app.dialog().file().set_title("Export to");
        if let Some(dir) = start.map(PathBuf::from).filter(|p| p.is_dir()) {
            d = d.set_directory(dir);
        }
        d.blocking_pick_folder().and_then(|f| f.into_path().ok()).map(|p| p.display().to_string())
    })
    .await
    .map_err(err)
}

/// What an export would write: real sizes, the folder name and which parts the meeting has.
#[tauri::command]
pub async fn export_plan(app: AppHandle, id: String, options: ExportOptions) -> CmdResult<ExportPlan> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        export::plan(&state.store, &state.cfg(), &id, &options).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Start an export; progress arrives as `export` events. On success the destination becomes the
/// remembered folder.
#[tauri::command]
pub fn export_meeting(app: AppHandle, id: String, options: ExportOptions) -> CmdResult<()> {
    let state = app.state::<AppState>();
    {
        let session = state.session.lock().unwrap();
        check_export(&id, session.as_ref().map(|s| s.meeting.id.as_str()), state.jobs.is_pending(&id))?;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut running = state.export.running.lock().unwrap();
        if running.is_some() {
            return Err("An export is already running".into());
        }
        *running = Some(cancel.clone());
    }
    let handle = app.clone();
    std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            let state = handle.state::<AppState>();
            let ev = |state, step, file: String, done, total| ExportEvent { meeting_id: id.clone(), state, step, file, done, total, folder: None, zip: None, error: None };
            let progress = |p: export::ExportProgress| {
                let _ = handle.emit("export", ev("running", Some(p.step), p.file, p.done, p.total));
            };
            let result = export::run(&state.store, &state.cfg(), &id, &options, &progress, &cancel);
            state.export.running.lock().unwrap().take();
            let event = match result {
                Ok(done) => {
                    if let Err(e) = Config::save_export_dir(&config_path(), &options.dest) {
                        eprintln!("export: remembering the folder: {e:#}");
                    }
                    state.cfg.write().unwrap().export.dir = Some(options.dest.clone());
                    state.export.exported.lock().unwrap().push(done.folder.clone());
                    ExportEvent {
                        folder: Some(done.folder.display().to_string()),
                        zip: done.zip.map(|z| z.display().to_string()),
                        ..ev("done", None, String::new(), done.bytes, done.bytes)
                    }
                }
                Err(e) if export::is_cancelled(&e) => ev("cancelled", None, String::new(), 0, 0),
                Err(e) => ExportEvent { error: Some(err(e)), ..ev("failed", None, String::new(), 0, 0) },
            };
            let _ = handle.emit("export", event);
        })
        .map_err(|e| {
            state.export.running.lock().unwrap().take();
            err(e)
        })?;
    Ok(())
}

#[tauri::command]
pub fn cancel_export(state: State<AppState>) {
    if let Some(flag) = state.export.running.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Show an exported folder in the file manager. Only folders exported in this session, so the
/// web view can't open anything else.
#[tauri::command]
pub fn open_export_folder(app: AppHandle, path: String) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = PathBuf::from(path);
    let state = app.state::<AppState>();
    if !path.is_dir() || !state.export.exported.lock().unwrap().contains(&path) {
        return Err("That folder wasn't exported here".into());
    }
    app.opener().open_path(path.display().to_string(), None::<&str>).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_the_recording_meeting_and_one_being_processed() {
        assert!(check_export("a", Some("a"), false).unwrap_err().contains("recording"));
        assert!(check_export("a", None, true).unwrap_err().contains("processed"));
        assert!(check_export("a", Some("b"), false).is_ok(), "another meeting may be recording");
        assert!(check_export("a", None, false).is_ok());
    }
}
