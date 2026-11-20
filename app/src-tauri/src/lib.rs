//! Tauri shell: exposes fofoca-core to the UI as commands and forwards session events.

mod settings;

use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use fofoca_core::calendar::{CalendarEvent, CalendarSettings, CalendarSource, Calendars, Keyring};
use fofoca_core::config::config_path;
use fofoca_core::model::Meeting;
use fofoca_core::{Config, MeetingMeta, Session, SessionEvent, StartInfo, Store};
use serde::Serialize;
use tauri::image::Image;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;

struct AppState {
    /// Behind a lock so Settings can change the audio policy while the app runs (spec 011).
    cfg: RwLock<Config>,
    store: Store,
    session: Mutex<Option<Session>>,
    calendars: Calendars<Keyring>,
    /// Serves replay audio (spec 014 fix).
    media: fofoca_core::media::MediaServer,
    /// Imports run here, one at a time (spec 012).
    jobs: fofoca_core::jobs::Jobs,
    /// Upcoming events, refreshed at most every CALENDAR_TTL unless forced.
    upcoming: Mutex<Option<(Instant, Upcoming)>>,
    /// GPU probe and model downloads (spec 015).
    settings: settings::SettingsState,
}

impl AppState {
    fn cfg(&self) -> Config {
        self.cfg.read().unwrap().clone()
    }
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
        meetings_dir: state.cfg().meetings_dir().display().to_string(),
        whisper_model: state.cfg().whisper_name(),
        llm_model: state.cfg().llm_name(),
        config: state.cfg(),
    }
}

#[tauri::command]
fn list_meetings(state: State<AppState>, label: Option<String>) -> CmdResult<Vec<MeetingMeta>> {
    state.store.list(label.as_deref()).map_err(err)
}

#[derive(Serialize)]
struct LabelCount {
    label: String,
    count: usize,
}

/// Labels in use, most used first (for autocomplete and the Meetings filter).
#[tauri::command]
fn known_labels(state: State<AppState>) -> CmdResult<Vec<LabelCount>> {
    Ok(state.store.known_labels().map_err(err)?.into_iter().map(|(label, count)| LabelCount { label, count }).collect())
}

#[tauri::command]
fn set_meeting_labels(state: State<AppState>, id: String, labels: Vec<String>) -> CmdResult<MeetingMeta> {
    let meta = state.store.set_labels(&id, &labels).map_err(err)?;
    // Keep the live session's copy in sync so the recording screen shows the new labels.
    if let Some(s) = state.session.lock().unwrap().as_mut() {
        if s.meeting.id == id {
            s.meeting.labels = meta.labels.clone();
        }
    }
    Ok(meta)
}

/// Every meeting's action items (spec 016); the UI filters by the meetings' labels.
#[tauri::command]
fn list_tasks(state: State<AppState>) -> CmdResult<Vec<fofoca_core::store::TaskItem>> {
    state.store.tasks().map_err(err)
}

#[tauri::command]
fn get_meeting(state: State<AppState>, id: String) -> CmdResult<Meeting> {
    state.store.load(&id).map_err(err)
}

/// Name a voice on the call (spec 003); an empty name goes back to "Person N".
#[tauri::command]
fn rename_speaker(state: State<AppState>, id: String, speaker: String, name: String) -> CmdResult<Meeting> {
    state.store.rename_speaker(&id, &speaker, &name).map_err(err)?;
    state.store.load(&id).map_err(err)
}

/// (Re-)run speaker separation on a stored meeting. CPU-bound, so it runs on a blocking thread.
#[tauri::command]
async fn separate_speakers(app: AppHandle, id: String) -> CmdResult<Meeting> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if state.session.lock().unwrap().as_ref().is_some_and(|s| s.meeting.id == id) {
            return Err("this meeting is still being recorded".to_string());
        }
        match fofoca_core::pipeline::separate_speakers(&state.store, &state.cfg(), &id).map_err(err)? {
            fofoca_core::pipeline::StepResult::Done { .. } => state.store.load(&id).map_err(err),
            fofoca_core::pipeline::StepResult::Skipped(msg) => Err(msg),
        }
    })
    .await
    .map_err(err)?
}

/// (Re-)write grounded notes (spec 005). Loads the summary model, so not while recording.
#[tauri::command]
async fn write_notes(app: AppHandle, id: String) -> CmdResult<Meeting> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if state.session.lock().unwrap().is_some() {
            return Err("a recording is running — the GPU is busy with it".to_string());
        }
        let m = state.store.load(&id).map_err(err)?;
        let server = fofoca_core::servers::ModelServer::start(fofoca_core::servers::Kind::Llm, &state.cfg(), state.cfg().workflow.notes_device).map_err(err)?;
        let client = fofoca_core::llm::LlmClient::new(&server.base_url).map_err(err)?;
        let notes = fofoca_core::notes::extract(&client, &m.segments, &m.speakers, &m.meta, &state.cfg()).map_err(err)?;
        drop(server);
        state.store.save_notes(&id, &notes).map_err(err)?;
        let _ = fofoca_core::pipeline::mark_done(&state.store, &id, fofoca_core::pipeline::STEP_NOTES);
        state.store.load(&id).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Keep this meeting's audio regardless of the global setting (spec 011), or follow it again.
#[tauri::command]
fn set_keep_audio(state: State<AppState>, id: String, keep: bool) -> CmdResult<Meeting> {
    state.store.update_meta(&id, |m| m.keep_audio = keep.then_some(true)).map_err(err)?;
    state.store.load(&id).map_err(err)
}

#[tauri::command]
fn delete_audio(state: State<AppState>, id: String) -> CmdResult<Meeting> {
    if state.session.lock().unwrap().as_ref().is_some_and(|s| s.meeting.id == id) {
        return Err("this meeting is still being recorded".into());
    }
    fofoca_core::retention::delete_audio(&state.store, &id).map_err(err)?;
    state.store.load(&id).map_err(err)
}

#[derive(Serialize)]
struct TrackUrls {
    mic: Option<String>,
    call: Option<String>,
}

/// URLs of the meeting's audio files that exist, for replay (spec 014). They point at the
/// loopback media server: WebKitGTK's GStreamer can't stream from the asset protocol.
#[tauri::command]
fn audio_tracks(state: State<AppState>, id: String) -> CmdResult<TrackUrls> {
    let t = state.store.audio_tracks(&id).map_err(err)?;
    let url = |p: &Option<std::path::PathBuf>, track| p.as_ref().map(|_| state.media.url(&id, track));
    Ok(TrackUrls { mic: url(&t.mic, "mic"), call: url(&t.call, "call") })
}

#[derive(Serialize)]
struct StorageView {
    audio: fofoca_core::config::AudioConfig,
    /// Audio still on disk across all meetings, in bytes.
    audio_bytes: u64,
    meetings_with_audio: usize,
}

#[tauri::command]
fn storage(state: State<AppState>) -> CmdResult<StorageView> {
    let ids: Vec<String> = state.store.list(None).map_err(err)?.into_iter().map(|m| m.id).collect();
    let sizes: Vec<u64> = ids.iter().map(|id| fofoca_core::retention::audio_bytes(&state.store, id)).collect();
    Ok(StorageView { audio: state.cfg().audio, audio_bytes: sizes.iter().sum(), meetings_with_audio: sizes.iter().filter(|b| **b > 0).count() })
}

/// Change the audio policy: saved to config.toml and applied right away.
#[tauri::command]
fn set_audio_policy(state: State<AppState>, audio: fofoca_core::config::AudioConfig) -> CmdResult<StorageView> {
    Config::save_audio(&config_path(), &audio).map_err(err)?;
    state.cfg.write().unwrap().audio = audio;
    fofoca_core::retention::apply(&state.store, &audio, chrono::Local::now().fixed_offset(), None, false).map_err(err)?;
    storage(state)
}

/// Native file picker for an audio/video file to import (spec 012).
#[tauri::command]
async fn pick_recording(app: AppHandle) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Import a recording")
            .add_filter("Audio and video", &["mp3", "m4a", "aac", "ogg", "opus", "oga", "wav", "flac", "webm", "mp4", "mkv", "mov", "amr", "3gp"])
            .blocking_pick_file()
            .and_then(|f| f.into_path().ok())
            .map(|p| p.display().to_string())
    })
    .await
    .map_err(err)
}

/// Create a meeting for a recording made elsewhere and queue its processing (spec 012). Returns
/// right away; progress arrives as `job` events.
#[tauri::command]
fn import_recording(state: State<AppState>, path: String, title: Option<String>, labels: Option<Vec<String>>) -> CmdResult<MeetingMeta> {
    let src = std::path::PathBuf::from(&path);
    let info = fofoca_core::import::probe(&src).map_err(err)?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let meta = state
        .store
        .create(&StartInfo {
            title: title.filter(|t| !t.trim().is_empty()).unwrap_or(stem),
            labels: labels.unwrap_or_default(),
            started_at: info.created_at,
            imported_from: src.file_name().map(|n| n.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .map_err(err)?;
    state.jobs.import(meta.id.clone(), src);
    Ok(meta)
}

#[tauri::command]
fn active_session(state: State<AppState>) -> Option<MeetingMeta> {
    state.session.lock().unwrap().as_ref().map(|s| s.meeting.clone())
}

#[tauri::command]
fn start_session(
    app: AppHandle,
    state: State<AppState>,
    title: String,
    attendees: Option<Vec<String>>,
    labels: Option<Vec<String>>,
    calendar_uid: Option<String>,
) -> CmdResult<MeetingMeta> {
    let mut labels = labels.unwrap_or_default();
    // spec 009: a recurring meeting gets the labels its last recording had
    if let Some(uid) = calendar_uid.as_deref() {
        labels.extend(state.store.labels_for_series(uid).unwrap_or_default());
    }
    let info = StartInfo { title, attendees: attendees.unwrap_or_default(), labels, calendar_uid, ..Default::default() };
    let mut slot = state.session.lock().unwrap();
    if slot.is_some() {
        return Err("a recording is already running".into());
    }
    let handle = app.clone();
    let sink = Arc::new(move |e: SessionEvent| {
        if matches!(e, SessionEvent::Finished { .. } | SessionEvent::Discarded { .. }) {
            // The worker thread emits this as its last act; dropping the handle detaches it.
            let state = handle.state::<AppState>();
            state.session.lock().unwrap().take();
            state.jobs.set_recording(false);
        }
        let _ = handle.emit("session", &e);
    });
    // imports give the GPU back while we record (spec 012)
    state.jobs.set_recording(true);
    let session = match Session::start(state.cfg(), state.store.clone(), &info, sink) {
        Ok(s) => s,
        Err(e) => {
            state.jobs.set_recording(false);
            return Err(err(e));
        }
    };
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

/// Pause / resume keeping audio (spec 010). Takes effect immediately.
#[tauri::command]
fn pause_session(state: State<AppState>) -> CmdResult<()> {
    state.session.lock().unwrap().as_ref().map(|s| s.pause()).ok_or_else(|| "no recording is running".into())
}

#[tauri::command]
fn resume_session(state: State<AppState>) -> CmdResult<()> {
    state.session.lock().unwrap().as_ref().map(|s| s.resume()).ok_or_else(|| "no recording is running".into())
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
/// Runs on a blocking thread: the calendar client uses `reqwest::blocking`, which panics when
/// called from inside the async runtime (the command would then never answer the UI).
#[tauri::command]
async fn add_calendar(app: AppHandle, input: String) -> CmdResult<CalendarSource> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let src = state.calendars.add(&input).map_err(err)?;
        state.upcoming.lock().unwrap().take();
        Ok(src)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn remove_calendar(state: State<AppState>, id: String) -> CmdResult<()> {
    state.calendars.remove(&id).map_err(err)?;
    state.upcoming.lock().unwrap().take();
    Ok(())
}

#[tauri::command]
fn set_calendar_default_labels(state: State<AppState>, id: String, labels: Vec<String>) -> CmdResult<CalendarSettings> {
    let s = state.calendars.set_default_labels(&id, &labels).map_err(err)?;
    state.upcoming.lock().unwrap().take();
    Ok(s)
}

#[tauri::command]
fn set_calendar_options(
    state: State<AppState>,
    only_with_call_link: bool,
    notify_before: bool,
    auto_record: bool,
) -> CmdResult<CalendarSettings> {
    let s = state
        .calendars
        .update(|s| {
            s.only_with_call_link = only_with_call_link;
            s.notify_before = notify_before;
            s.auto_record = auto_record;
        })
        .map_err(err)?;
    state.upcoming.lock().unwrap().take();
    Ok(s)
}

/// Desktop notification (shown by the system even when the window is in the background).
#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) -> CmdResult<()> {
    app.notification().builder().title(title).body(body).show().map_err(err)
}

/// Events from now (including ones in progress) to 7 days ahead.
/// On a blocking thread for the same reason as `add_calendar`.
#[tauri::command]
async fn upcoming_events(app: AppHandle, force: bool) -> CmdResult<Upcoming> {
    tauri::async_runtime::spawn_blocking(move || fetch_upcoming(&app.state::<AppState>(), force))
        .await
        .map_err(err)
}

fn fetch_upcoming(state: &AppState, force: bool) -> Upcoming {
    let cached = if force {
        None
    } else {
        state.upcoming.lock().unwrap().as_ref().filter(|(at, _)| at.elapsed() < CALENDAR_TTL).map(|(_, u)| u.clone())
    };
    let u = cached.unwrap_or_else(|| {
        let now = chrono::Utc::now();
        let (events, errors) = state.calendars.upcoming(now, now + chrono::Duration::days(7));
        let u = Upcoming { events, errors };
        *state.upcoming.lock().unwrap() = Some((Instant::now(), u.clone()));
        u
    });
    with_series_labels(&state.store, u)
}

/// Add each event's series labels (spec 009) to its calendar defaults, so the UI shows exactly the
/// labels a recording would get. Done after the cache: labels edited a minute ago count.
fn with_series_labels(store: &Store, mut u: Upcoming) -> Upcoming {
    let Ok(meetings) = store.list(None) else { return u };
    for e in &mut u.events {
        if let Some(m) = meetings.iter().find(|m| m.calendar_uid.as_deref() == Some(e.uid.as_str())) {
            e.labels = fofoca_core::labels::normalize_all(&[e.labels.as_slice(), m.labels.as_slice()].concat());
        }
    }
    u
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cfg = Config::load_or_create().expect("loading fo-foca config");
    let store = Store::new(cfg.meetings_dir()).expect("opening meetings folder");
    {
        // spec 011: apply the audio policy to past meetings, off the startup path
        let (store, audio) = (store.clone(), cfg.audio);
        std::thread::spawn(move || {
            if let Err(e) = fofoca_core::retention::apply(&store, &audio, chrono::Local::now().fixed_offset(), None, false) {
                eprintln!("audio retention: {e:#}");
            }
        });
    }
    let calendars = Calendars::new(fofoca_core::config::calendars_path(), Keyring).expect("calendar client");
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            let cfg_handle = handle.clone();
            let jobs = fofoca_core::jobs::Jobs::start(
                store.clone(),
                Arc::new(move || cfg_handle.state::<AppState>().cfg()),
                Arc::new(move |e| {
                    let _ = handle.emit("job", &e);
                }),
            );
            // Replay (spec 014): meeting audio over loopback HTTP, token-guarded, meeting audio only.
            let media = fofoca_core::media::MediaServer::start(store.clone())?;
            app.manage(AppState { cfg: RwLock::new(cfg.clone()), store: store.clone(), session: Mutex::new(None), calendars, upcoming: Mutex::new(None), jobs, media, settings: Default::default() });
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
            known_labels,
            set_meeting_labels,
            get_meeting,
            list_tasks,
            rename_speaker,
            separate_speakers,
            write_notes,
            set_keep_audio,
            delete_audio,
            audio_tracks,
            settings::workflow_view,
            settings::probe_hardware,
            settings::set_workflow,
            settings::models_view,
            settings::set_model_path,
            settings::use_model,
            settings::pick_model_file,
            settings::download_model,
            settings::cancel_download,
            storage,
            set_audio_policy,
            pick_recording,
            import_recording,
            active_session,
            start_session,
            summarize_now,
            stop_session,
            discard_session,
            pause_session,
            resume_session,
            calendar_settings,
            add_calendar,
            remove_calendar,
            set_calendar_options,
            set_calendar_default_labels,
            notify,
            upcoming_events
        ])
        .run(tauri::generate_context!())
        .expect("error while running fo-foca");
}
