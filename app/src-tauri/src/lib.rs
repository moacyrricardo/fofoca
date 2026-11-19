//! Tauri shell: exposes fofoca-core to the UI as commands and forwards session events.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fofoca_core::calendar::{CalendarEvent, CalendarSettings, CalendarSource, Calendars, Keyring};
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
    calendars: Calendars<Keyring>,
    /// Upcoming events, refreshed at most every CALENDAR_TTL unless forced.
    upcoming: Mutex<Option<(Instant, Upcoming)>>,
}

const CALENDAR_TTL: Duration = Duration::from_secs(300);

#[derive(Serialize, Clone)]
struct Upcoming {
    events: Vec<CalendarEvent>,
    errors: Vec<String>,
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

#[tauri::command]
fn calendar_settings(state: State<AppState>) -> CalendarSettings {
    state.calendars.settings()
}

/// Validates the link (or .ics path) by fetching and parsing it before storing anything.
#[tauri::command(async)]
fn add_calendar(state: State<'_, AppState>, input: String) -> CmdResult<CalendarSource> {
    let src = state.calendars.add(&input).map_err(err)?;
    state.upcoming.lock().unwrap().take();
    Ok(src)
}

#[tauri::command]
fn remove_calendar(state: State<AppState>, id: String) -> CmdResult<()> {
    state.calendars.remove(&id).map_err(err)?;
    state.upcoming.lock().unwrap().take();
    Ok(())
}

#[tauri::command]
fn set_calendar_filter(state: State<AppState>, only_with_call_link: bool) -> CmdResult<CalendarSettings> {
    let s = state.calendars.set_only_with_call_link(only_with_call_link).map_err(err)?;
    state.upcoming.lock().unwrap().take();
    Ok(s)
}

/// Events from now (including ones in progress) to 7 days ahead.
#[tauri::command(async)]
fn upcoming_events(state: State<'_, AppState>, force: bool) -> Upcoming {
    if !force {
        if let Some((at, u)) = state.upcoming.lock().unwrap().as_ref() {
            if at.elapsed() < CALENDAR_TTL {
                return u.clone();
            }
        }
    }
    let now = chrono::Utc::now();
    let (events, errors) = state.calendars.upcoming(now, now + chrono::Duration::days(7));
    let u = Upcoming { events, errors };
    *state.upcoming.lock().unwrap() = Some((Instant::now(), u.clone()));
    u
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cfg = Config::load_or_create().expect("loading fo-foca config");
    let store = Store::new(cfg.meetings_dir()).expect("opening meetings folder");
    let calendars = Calendars::new(fofoca_core::config::calendars_path(), Keyring).expect("calendar client");
    tauri::Builder::default()
        .manage(AppState { cfg, store, session: Mutex::new(None), calendars, upcoming: Mutex::new(None) })
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
            discard_session,
            calendar_settings,
            add_calendar,
            remove_calendar,
            set_calendar_filter,
            upcoming_events
        ])
        .run(tauri::generate_context!())
        .expect("error while running fo-foca");
}
