//! Tauri shell: exposes fofoca-core to the UI as commands and forwards session events.

use std::sync::{Arc, Mutex};

use fofoca_core::config::config_path;
use fofoca_core::model::Meeting;
use fofoca_core::{Config, MeetingMeta, Session, SessionEvent, Store};
use serde::Serialize;
use tauri::image::Image;
use tauri::{AppHandle, Emitter, Manager, State};

struct AppState {
    cfg: Config,
    store: Store,
    session: Mutex<Option<Session>>,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

#[derive(Serialize)]
struct ConfigView {
    path: String,
    meetings_dir: String,
    whisper_model: String,
    llm_model: String,
    config: Config,
}

#[tauri::command]
fn get_config(state: State<AppState>) -> ConfigView {
    ConfigView {
        path: config_path().display().to_string(),
        meetings_dir: state.cfg.meetings_dir().display().to_string(),
        whisper_model: state.cfg.whisper_name(),
        llm_model: state.cfg.llm_name(),
        config: state.cfg.clone(),
    }
}

#[tauri::command]
fn list_meetings(state: State<AppState>) -> CmdResult<Vec<MeetingMeta>> {
    state.store.list().map_err(err)
}

#[tauri::command]
fn get_meeting(state: State<AppState>, id: String) -> CmdResult<Meeting> {
    state.store.load(&id).map_err(err)
}

#[tauri::command]
fn active_session(state: State<AppState>) -> Option<MeetingMeta> {
    state.session.lock().unwrap().as_ref().map(|s| s.meeting.clone())
}

#[tauri::command]
fn start_session(app: AppHandle, state: State<AppState>, title: String) -> CmdResult<MeetingMeta> {
    let mut slot = state.session.lock().unwrap();
    if slot.is_some() {
        return Err("a recording is already running".into());
    }
    let handle = app.clone();
    let sink = Arc::new(move |e: SessionEvent| {
        if matches!(e, SessionEvent::Finished { .. } | SessionEvent::Discarded { .. }) {
            // The worker thread emits this as its last act; dropping the handle detaches it.
            handle.state::<AppState>().session.lock().unwrap().take();
        }
        let _ = handle.emit("session", &e);
    });
    let session = Session::start(state.cfg.clone(), state.store.clone(), &title, sink).map_err(err)?;
    let meta = session.meeting.clone();
    *slot = Some(session);
    Ok(meta)
}

#[tauri::command]
fn summarize_now(state: State<AppState>) -> CmdResult<()> {
    match state.session.lock().unwrap().as_ref() {
        Some(s) => {
            s.summarize();
            Ok(())
        }
        None => Err("no recording is running".into()),
    }
}

#[tauri::command]
fn stop_session(state: State<AppState>, summarize: bool) -> CmdResult<()> {
    match state.session.lock().unwrap().as_ref() {
        Some(s) => {
            s.stop(summarize);
            Ok(())
        }
        None => Err("no recording is running".into()),
    }
}

#[tauri::command]
fn discard_session(state: State<AppState>) -> CmdResult<()> {
    match state.session.lock().unwrap().as_ref() {
        Some(s) => {
            s.discard();
            Ok(())
        }
        None => Err("no recording is running".into()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cfg = Config::load_or_create().expect("loading fo-foca config");
    let store = Store::new(cfg.meetings_dir()).expect("opening meetings folder");
    tauri::Builder::default()
        .manage(AppState { cfg, store, session: Mutex::new(None) })
        .setup(|app| {
            // Embedded so the window has the seal icon even when no .desktop entry is installed
            // (X11/XWayland). GNOME on Wayland takes the icon from the desktop entry instead —
            // bundles ship one; for development see scripts/install-dev-desktop-entry.sh.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_icon(Image::from_bytes(include_bytes!("../icons/128x128@2x.png"))?);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            list_meetings,
            get_meeting,
            active_session,
            start_session,
            summarize_now,
            stop_session,
            discard_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running fo-foca");
}
