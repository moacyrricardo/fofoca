//! Desktop tray. Native menu updates run on the UI thread, including updates
//! originating in the recording worker and its status ticker.
use super::{err, fetch_upcoming, start_recording, AppState, CmdResult};
use chrono::{DateTime, Duration, Utc};
use fofoca_core::{calendar::CalendarEvent, SessionEvent, StartInfo};
use std::sync::{atomic::Ordering, Mutex};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager, State, Wry,
};

const IDLE_ICON: &[u8] = include_bytes!("../icons/tray-idle.png");
const RECORDING_ICON: &[u8] = include_bytes!("../icons/tray-recording.png");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Recording { paused: bool, elapsed: u64 },
    Processing,
}

struct MenuState {
    record: &'static str,
    pause: Option<&'static str>,
    recording_icon: bool,
    title: String,
}

fn menu_state(mode: Mode) -> MenuState {
    let (record, pause, recording_icon, status) = match mode {
        Mode::Idle => ("● Start recording", None, false, "ready".into()),
        Mode::Recording {
            paused: false,
            elapsed,
        } => (
            "■ Stop recording",
            Some("❚❚ Pause"),
            true,
            format!("recording {:02}:{:02}", elapsed / 60, elapsed % 60),
        ),
        Mode::Recording { paused: true, .. } => {
            ("■ Stop recording", Some("▶ Resume"), false, "paused".into())
        }
        Mode::Processing => ("Saving recording…", None, false, "processing".into()),
    };
    MenuState {
        record,
        pause,
        recording_icon,
        title: format!("fo-foca — {status}"),
    }
}

struct Tray {
    icon: TrayIcon,
    menu: Menu<Wry>,
    record: MenuItem<Wry>,
    pause: MenuItem<Wry>,
    window: MenuItem<Wry>,
    mode: Mutex<Mode>,
}

impl Tray {
    fn update(&self, next: Mode) -> tauri::Result<()> {
        let mut mode = self.mode.lock().unwrap();
        if *mode == next {
            return Ok(());
        }
        let old = menu_state(*mode);
        let new = menu_state(next);
        *mode = next;
        drop(mode);
        if old.record != new.record {
            self.record.set_text(new.record)?;
            self.record.set_enabled(next != Mode::Processing)?;
        }
        if old.pause != new.pause {
            if let Some(label) = new.pause {
                self.pause.set_text(label)?;
                if old.pause.is_none() {
                    self.menu.insert(&self.pause, 1)?;
                }
            } else {
                self.menu.remove(&self.pause)?;
            }
        }
        if old.recording_icon != new.recording_icon {
            let bytes = if new.recording_icon {
                RECORDING_ICON
            } else {
                IDLE_ICON
            };
            self.icon.set_icon(Some(Image::from_bytes(bytes)?))?;
        }
        if old.title != new.title {
            self.icon.set_tooltip(Some(&new.title))?;
            #[cfg(target_os = "linux")]
            self.icon.set_title(Some(&new.title))?; // AppIndicator exposes its label, not tooltips.
        }
        Ok(())
    }
}

pub(super) fn setup(app: &AppHandle) -> tauri::Result<()> {
    let idle = menu_state(Mode::Idle);
    let item = |id, label| MenuItem::with_id(app, id, label, true, None::<&str>);
    let record = item("record", idle.record)?;
    let pause = item("pause", "❚❚ Pause")?;
    let window = item("window", "Hide window")?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = item("quit", "Quit fo-foca")?;
    let menu = Menu::with_items(app, &[&record, &window, &separator, &quit])?;
    let builder = TrayIconBuilder::new()
        .icon(Image::from_bytes(IDLE_ICON)?)
        .menu(&menu)
        .tooltip(&idle.title)
        .on_menu_event(|app, event| {
            if let Err(error) = action(app, event.id().as_ref()) {
                report_error(app, error);
            }
        })
        .on_tray_icon_event(|icon, _| refresh_window(icon.app_handle()));
    #[cfg(target_os = "linux")]
    let builder = builder.title(&idle.title);
    let icon = builder.build(app)?;
    app.manage(Tray {
        icon,
        menu,
        record,
        pause,
        window,
        mode: Mutex::new(Mode::Idle),
    });
    if let Some(main) = app.get_webview_window("main") {
        let handle = app.clone();
        main.on_window_event(move |event| {
            if matches!(
                event,
                tauri::WindowEvent::Focused(_) | tauri::WindowEvent::Resized(_)
            ) {
                refresh_window(&handle);
            }
        });
        refresh_window(app);
    }
    Ok(())
}

fn refresh_window(app: &AppHandle) {
    if let (Some(tray), Some(main)) = (app.try_state::<Tray>(), app.get_webview_window("main")) {
        if let Ok(visible) = main.is_visible() {
            let _ = tray.window.set_text(if visible {
                "Hide window"
            } else {
                "Show window"
            });
        }
    }
}

pub(super) fn session_event(app: &AppHandle, event: &SessionEvent) {
    let next = match event {
        SessionEvent::Started { .. } => Mode::Recording {
            paused: false,
            elapsed: 0,
        },
        SessionEvent::Status {
            paused, elapsed_s, ..
        } => Mode::Recording {
            paused: *paused,
            elapsed: *elapsed_s as u64,
        },
        SessionEvent::Paused { at_s } => Mode::Recording {
            paused: true,
            elapsed: *at_s as u64,
        },
        SessionEvent::Resumed { at_s } => Mode::Recording {
            paused: false,
            elapsed: *at_s as u64,
        },
        SessionEvent::Stopping => Mode::Processing,
        SessionEvent::Discarded { .. } | SessionEvent::Finished { .. } => Mode::Idle,
        _ => return,
    };
    let tick = matches!(
        event,
        SessionEvent::Status { .. } | SessionEvent::Paused { .. } | SessionEvent::Resumed { .. }
    );
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.try_state::<Tray>() {
            // The ticker can outlive capture while the worker is saving.
            if tick && !matches!(*tray.mode.lock().unwrap(), Mode::Recording { .. }) {
                return;
            }
            if let Err(error) = tray.update(next) {
                eprintln!("updating tray: {error}");
            }
        }
    });
}

fn start_info(events: Vec<CalendarEvent>, now: DateTime<Utc>) -> StartInfo {
    events
        .into_iter()
        .find(|event| {
            !event.all_day
                && DateTime::parse_from_rfc3339(&event.start).is_ok_and(|start| {
                    (now - Duration::minutes(2)..=now + Duration::minutes(1))
                        .contains(&start.to_utc())
                })
                && DateTime::parse_from_rfc3339(&event.end).is_ok_and(|end| end > now)
        })
        .map(|e| StartInfo {
            title: e.title,
            attendees: e.attendees,
            labels: e.labels,
            calendar_uid: Some(e.uid),
            ..Default::default()
        })
        .unwrap_or_default()
}

fn report_error(app: &AppHandle, message: String) {
    let _ = app.emit(
        "session",
        SessionEvent::Error {
            message: message.clone(),
        },
    );
    if app
        .get_webview_window("main")
        .is_some_and(|w| !w.is_visible().unwrap_or(true))
    {
        let _ = super::notify(app.clone(), "fo-foca".into(), message);
    }
}

fn main_window(app: &AppHandle) -> CmdResult<tauri::WebviewWindow> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window unavailable".into())
}

fn show_window(main: &tauri::WebviewWindow) -> CmdResult<()> {
    main.show().map_err(err)?;
    main.unminimize().map_err(err)?;
    main.set_focus().map_err(err)
}

fn action(app: &AppHandle, id: &str) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let tray = app.state::<Tray>();
    let mode = *tray.mode.lock().unwrap();
    match id {
        "record" if mode == Mode::Idle => {
            let handle = app.clone();
            // Calendar I/O must not block the desktop event loop.
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(error) = start_recording(&handle, |state| {
                    let upcoming = fetch_upcoming(state, false);
                    start_info(upcoming.events, Utc::now())
                }) {
                    report_error(&handle, error);
                }
            });
        }
        "record" if matches!(mode, Mode::Recording { .. }) => {
            let notes = state.cfg().workflow.notes_after;
            super::stop_session(state, notes)?;
        }
        "pause" => {
            if let Some(session) = state.session.lock().unwrap().as_ref() {
                if session.is_paused() {
                    session.resume();
                } else {
                    session.pause();
                }
            }
        }
        "window" => {
            let main = main_window(app)?;
            if main.is_visible().map_err(err)? {
                main.hide().map_err(err)?;
            } else {
                show_window(&main)?;
            }
            refresh_window(app);
        }
        "quit" => {
            let slot = state.session.lock().unwrap();
            let Some(session) = slot.as_ref() else {
                drop(slot);
                app.exit(0);
                return Ok(());
            };
            // A previous Stop has already committed to saving this meeting.
            if mode == Mode::Processing {
                state.quit_after_save.store(true, Ordering::Relaxed);
                return Ok(());
            }
            let id = session.meeting.id.clone();
            drop(slot);
            let main = main_window(app)?;
            show_window(&main)?;
            refresh_window(app);
            main.emit("quit-requested", id).map_err(err)?;
        }
        _ => {}
    }
    Ok(())
}

#[tauri::command]
pub(super) fn stop_and_quit(state: State<AppState>, meeting_id: String) -> CmdResult<()> {
    let slot = state.session.lock().unwrap();
    let session = slot
        .as_ref()
        .filter(|s| s.meeting.id == meeting_id)
        .ok_or("recording already ended")?;
    state.quit_after_save.store(true, Ordering::Relaxed);
    session.stop(state.cfg().workflow.notes_after);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_and_processing_have_no_pause_or_badge() {
        let idle = menu_state(Mode::Idle);
        assert_eq!(idle.record, "● Start recording");
        assert_eq!(idle.title, "fo-foca — ready");
        let saving = menu_state(Mode::Processing);
        assert_eq!(saving.record, "Saving recording…");
        for state in [idle, saving] {
            assert!(state.pause.is_none());
            assert!(!state.recording_icon);
        }
    }

    #[test]
    fn recording_and_paused_share_stop_but_change_badge_title_and_action() {
        let recording = menu_state(Mode::Recording {
            paused: false,
            elapsed: 754,
        });
        assert_eq!(recording.record, "■ Stop recording");
        assert_eq!(recording.pause, Some("❚❚ Pause"));
        assert_eq!(recording.title, "fo-foca — recording 12:34");
        assert!(recording.recording_icon);
        let paused = menu_state(Mode::Recording {
            paused: true,
            elapsed: 754,
        });
        assert_eq!(paused.record, recording.record);
        assert_eq!(paused.pause, Some("▶ Resume"));
        assert_eq!(paused.title, "fo-foca — paused");
        assert!(!paused.recording_icon);
    }

    #[test]
    fn calendar_start_uses_banner_order_metadata_and_time_boundaries() {
        let now: DateTime<Utc> = "2026-10-01T12:00:00Z".parse().unwrap();
        let event = |offset| CalendarEvent {
            start: (now + Duration::seconds(offset)).to_rfc3339(),
            end: (now + Duration::hours(1)).to_rfc3339(),
            all_day: false,
            uid: "series".into(),
            title: "Team call".into(),
            labels: vec!["work".into()],
            attendees: vec!["Sam".into()],
            calendar: "Work".into(),
            call_link: None,
        };
        for offset in [-120, 0, 60] {
            let info = start_info(
                vec![
                    event(-121),
                    event(offset),
                    CalendarEvent {
                        title: "Next".into(),
                        ..event(60)
                    },
                ],
                now,
            );
            assert_eq!(info.title, "Team call");
            assert_eq!(info.labels, ["work"]);
            assert_eq!(info.attendees, ["Sam"]);
            assert_eq!(info.calendar_uid.as_deref(), Some("series"));
        }
        let ignored = vec![
            event(-121),
            event(61),
            CalendarEvent {
                all_day: true,
                ..event(0)
            },
            CalendarEvent {
                end: now.to_rfc3339(),
                ..event(-120)
            },
        ];
        let fallback = start_info(ignored, now);
        assert!(fallback.title.is_empty()); // Store::create supplies the local date and time.
        assert!(fallback.calendar_uid.is_none());
    }
}
