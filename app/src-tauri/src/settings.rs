//! Settings → Processing and Models (spec 015): the global workflow, model files and downloads.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fofoca_core::config::{config_path, Config, Layout, ModelSetting, Workflow};
use fofoca_core::hardware::{self, Footprint, Gpu};
use fofoca_core::models::{self, Task};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{err, AppState, CmdResult};

/// Settings-screen state that lives as long as the app.
#[derive(Default)]
pub struct SettingsState {
    /// Last GPU probe; `None` = not probed yet.
    gpu: Mutex<Option<Option<Gpu>>>,
    /// Running downloads by catalog id, with their cancel flags.
    downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

#[derive(Serialize)]
pub struct WorkflowView {
    workflow: Workflow,
    gpu: Option<Gpu>,
    whisper: ModelInfo,
    llm: ModelInfo,
    /// What fo-foca suggests for this computer.
    suggested: Layout,
    /// A recording is running: changes apply to the next one.
    recording: bool,
}

#[derive(Serialize)]
pub struct ModelInfo {
    name: String,
    footprint: Option<Footprint>,
}

fn gpu(state: &AppState, force: bool) -> Option<Gpu> {
    let mut slot = state.settings.gpu.lock().unwrap();
    if force || slot.is_none() {
        *slot = Some(hardware::probe(&state.cfg()));
    }
    slot.clone().flatten()
}

fn workflow_view_of(state: &AppState, force_probe: bool) -> WorkflowView {
    let cfg = state.cfg();
    let gpu = gpu(state, force_probe);
    let (w, l) = (hardware::footprint(&cfg.whisper.model), hardware::footprint(&cfg.llm.model));
    WorkflowView {
        workflow: cfg.workflow,
        suggested: hardware::suggest(gpu.as_ref(), w, l),
        gpu,
        whisper: ModelInfo { name: cfg.whisper_name(), footprint: w },
        llm: ModelInfo { name: cfg.llm_name(), footprint: l },
        recording: state.session.lock().unwrap().is_some(),
    }
}

/// Async + a blocking thread: the first GPU probe can take a while, and a sync command would run
/// on the main thread and freeze the window.
#[tauri::command]
pub async fn workflow_view(app: AppHandle) -> CmdResult<WorkflowView> {
    tauri::async_runtime::spawn_blocking(move || workflow_view_of(&app.state::<AppState>(), false)).await.map_err(err)
}

/// Probe the GPU once in the background at startup, so Settings opens with it ready.
pub fn probe_in_background(app: AppHandle) {
    std::thread::spawn(move || {
        let _ = gpu(&app.state::<AppState>(), false);
    });
}

/// "Check this computer again": probe the GPU anew.
#[tauri::command]
pub async fn probe_hardware(app: AppHandle) -> CmdResult<WorkflowView> {
    tauri::async_runtime::spawn_blocking(move || workflow_view_of(&app.state::<AppState>(), true)).await.map_err(err)
}

#[tauri::command]
pub async fn set_workflow(app: AppHandle, workflow: Workflow) -> CmdResult<WorkflowView> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        Config::save_workflow(&config_path(), &workflow).map_err(err)?;
        state.cfg.write().unwrap().workflow = workflow;
        Ok(workflow_view_of(&state, false))
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
pub struct EntryView {
    id: &'static str,
    name: &'static str,
    note: &'static str,
    bytes: u64,
    present: bool,
    downloading: bool,
}

#[derive(Serialize)]
pub struct TaskRow {
    task: Task,
    /// The configured file(s) for this task.
    paths: Vec<String>,
    /// All of them exist.
    found: bool,
    /// Catalog entry the settings point at (`None` = the user's own file).
    current: Option<&'static str>,
    entries: Vec<EntryView>,
}

#[derive(Serialize)]
pub struct EngineRow {
    setting: ModelSetting,
    path: String,
    found: bool,
}

#[derive(Serialize)]
pub struct ModelsView {
    dir: String,
    tasks: Vec<TaskRow>,
    engines: Vec<EngineRow>,
}

fn settings_of(task: Task) -> &'static [ModelSetting] {
    match task {
        Task::Whisper => &[ModelSetting::Whisper],
        Task::Vad => &[ModelSetting::Vad],
        Task::Llm => &[ModelSetting::Llm],
        Task::Speakers => &[ModelSetting::Segmentation, ModelSetting::Embedding],
    }
}

fn models_view_of(state: &AppState) -> ModelsView {
    let cfg = state.cfg();
    let dir = models::models_dir();
    let catalog = models::catalog();
    let downloading = state.settings.downloads.lock().unwrap();
    let tasks = [Task::Whisper, Task::Llm, Task::Speakers, Task::Vad]
        .into_iter()
        .map(|task| {
            let paths: Vec<PathBuf> = settings_of(task).iter().filter_map(|s| cfg.path_of(*s).map(PathBuf::from)).collect();
            TaskRow {
                task,
                found: !paths.is_empty() && paths.iter().all(|p| p.is_file()),
                paths: paths.iter().map(|p| p.display().to_string()).collect(),
                current: models::current(&cfg, &catalog, task).map(|e| e.id),
                entries: catalog
                    .iter()
                    .filter(|e| e.task == task)
                    .map(|e| EntryView { id: e.id, name: e.name, note: e.note, bytes: e.bytes(), present: models::present(&cfg, e, &dir), downloading: downloading.contains_key(e.id) })
                    .collect(),
            }
        })
        .collect();
    let engines = [ModelSetting::WhisperServer, ModelSetting::LlmServer]
        .into_iter()
        .map(|s| {
            let p = cfg.path_of(s).map(PathBuf::from).unwrap_or_default();
            EngineRow { setting: s, found: p.is_file(), path: p.display().to_string() }
        })
        .collect();
    ModelsView { dir: dir.display().to_string(), tasks, engines }
}

#[tauri::command]
pub fn models_view(state: State<AppState>) -> ModelsView {
    models_view_of(&state)
}

fn save_paths(state: &AppState, paths: &[(ModelSetting, PathBuf)]) -> CmdResult<()> {
    for (setting, path) in paths {
        Config::save_path(&config_path(), *setting, path).map_err(err)?;
        state.cfg.write().unwrap().set_path(*setting, path.clone());
    }
    Ok(())
}

/// Point a setting at a file the user picked.
#[tauri::command]
pub fn set_model_path(state: State<AppState>, setting: ModelSetting, path: String) -> CmdResult<ModelsView> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("{path} isn't a file"));
    }
    save_paths(&state, &[(setting, p)])?;
    Ok(models_view_of(&state))
}

/// Switch a task to a suggested model that is already on this computer.
#[tauri::command]
pub fn use_model(state: State<AppState>, id: String) -> CmdResult<ModelsView> {
    let cfg = state.cfg();
    let dir = models::models_dir();
    let entry = models::catalog().into_iter().find(|e| e.id == id).ok_or("unknown model")?;
    let paths: Option<Vec<_>> = entry.files.iter().map(|f| models::locate(&cfg, f, &dir).map(|p| (f.setting, p))).collect();
    save_paths(&state, &paths.ok_or("that model isn't downloaded yet")?)?;
    Ok(models_view_of(&state))
}

/// Native file picker for a model or engine file.
#[tauri::command]
pub async fn pick_model_file(app: AppHandle, setting: ModelSetting) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        let mut d = app.dialog().file().set_title("Choose a file");
        d = match setting {
            ModelSetting::Whisper | ModelSetting::Vad => d.add_filter("whisper.cpp model", &["bin"]),
            ModelSetting::Llm => d.add_filter("GGUF model", &["gguf"]),
            ModelSetting::Segmentation | ModelSetting::Embedding => d.add_filter("ONNX model", &["onnx"]),
            ModelSetting::WhisperServer | ModelSetting::LlmServer => d,
        };
        d.blocking_pick_file().and_then(|f| f.into_path().ok()).map(|p| p.display().to_string())
    })
    .await
    .map_err(err)
}

#[derive(Serialize, Clone)]
struct DownloadEvent {
    id: String,
    done: u64,
    total: u64,
    /// `running` | `done` | `failed` | `cancelled`
    state: &'static str,
    error: Option<String>,
}

/// Download a suggested model in the background; progress arrives as `download` events, and
/// the task switches to it when done.
#[tauri::command]
pub fn download_model(app: AppHandle, id: String) -> CmdResult<()> {
    let entry = models::catalog().into_iter().find(|e| e.id == id).ok_or("unknown model")?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let state = app.state::<AppState>();
        let mut running = state.settings.downloads.lock().unwrap();
        if running.contains_key(&id) {
            return Ok(());
        }
        running.insert(id.clone(), cancel.clone());
    }
    std::thread::spawn(move || {
        let total = entry.bytes();
        let emit = |done, state, error| {
            let _ = app.emit("download", DownloadEvent { id: id.clone(), done, total, state, error });
        };
        let result = models::download(&entry, &models::models_dir(), &|done, _| emit(done, "running", None), &cancel);
        let state = app.state::<AppState>();
        state.settings.downloads.lock().unwrap().remove(&id);
        match result.map_err(err).and_then(|paths| save_paths(&state, &paths)) {
            Ok(()) => emit(total, "done", None),
            Err(_) if cancel.load(Ordering::Relaxed) => emit(0, "cancelled", None),
            Err(e) => emit(0, "failed", Some(e)),
        }
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<AppState>, id: String) {
    if let Some(flag) = state.settings.downloads.lock().unwrap().get(&id) {
        flag.store(true, Ordering::Relaxed);
    }
}
