//! Settings window. Renders [`crate::settings::SettingsState`] and nothing else.
//!
//! The webview is behind feature `settings-ui`. Without it there is no window.

use crate::config::{user_config_file, Config};
use std::path::{Path, PathBuf};

/// True when no config file is on disk, or a chosen model file is missing.
pub fn should_open(cli_has_file: bool, cfg: &Config) -> bool {
    !cli_has_file || cfg.missing_model().is_some()
}

pub fn save_target(explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(user_config_file)
        .unwrap_or_else(|| PathBuf::from("live-subs.toml"))
}

#[cfg(feature = "settings-ui")]
mod window {
    use crate::config::Config;
    use crate::models::{
        fetch, model_dir, remove_all_in, remove_in, url_file_name, validate_file, Catalogue,
        Hardware, HfHub, Kind, Model, ModelSearch,
    };
    use crate::settings::{emit, DevEvent, Event, SettingsState, SlotKind, Snapshot};
    use anyhow::{anyhow, Result};
    use serde::{Deserialize, Serialize};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tauri::{AppHandle, Emitter, Manager, State};
    use tauri_plugin_dialog::DialogExt;
    use tokio::runtime::Handle;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    struct FetchSpec {
        url: String,
        sha256: String,
    }

    struct Host {
        cfg: Config,
        save_to: PathBuf,
        state: SettingsState,
        stt_models: Vec<Model>,
        cleanup_models: Vec<Model>,
        devices: Vec<String>,
        rt: Handle,
        client: reqwest::Client,
        download_cancel: CancellationToken,
        session_cancel: CancellationToken,
        busy: bool,
        worker: Option<std::thread::JoinHandle<()>>,
        pending_start: bool,
        started: Arc<AtomicBool>,
        dev_marks: u8,
        extra_stt: Option<FetchSpec>,
        extra_cleanup: Option<FetchSpec>,
    }

    #[derive(Debug, Clone, Serialize)]
    struct ModelChoice {
        id: String,
        name: String,
        licence: String,
        path: String,
        present: bool,
    }

    #[derive(Debug, Clone, Serialize)]
    struct LangRow {
        code: String,
        name: String,
    }

    #[derive(Debug, Clone, Serialize)]
    struct DeviceRow {
        name: Option<String>,
        label: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct HfHit {
        repo: String,
        filename: String,
        size: u64,
        sha256: String,
        licence: String,
        url: String,
    }

    pub fn run(cfg: Config, save_to: PathBuf, has_file: bool, rt: Handle) -> Result<bool> {
        let started = Arc::new(AtomicBool::new(false));
        let host = Host::open(cfg, save_to, has_file, rt, started.clone())?;
        tauri::Builder::default()
            .plugin(tauri_plugin_dialog::init())
            .manage(Mutex::new(host))
            .setup(|app| {
                emit(&DevEvent::WindowOpened);
                if let Some(host) = app.try_state::<Mutex<Host>>() {
                    let snap = lock_ref(&host)?.snapshot();
                    app.emit("snapshot", snap)
                        .map_err(|e| anyhow!("settings: {e}"))?;
                }
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                snapshot,
                event,
                models,
                languages,
                devices,
                search_hf,
                pick_hf,
                browse,
                download,
                cancel_download,
                start,
                stop,
                remove_one,
                remove_all
            ])
            .build(tauri::generate_context!())
            .map_err(|e| anyhow!("settings: {e}"))?
            .run(|app, event| {
                if let tauri::RunEvent::Exit = event {
                    if let Some(host) = app.try_state::<Mutex<Host>>() {
                        if let Ok(mut host) = lock_ref(&host) {
                            host.shutdown();
                        }
                    }
                }
            });
        Ok(started.load(Ordering::Relaxed))
    }

    fn lock_ref(host: &Mutex<Host>) -> Result<std::sync::MutexGuard<'_, Host>> {
        Ok(host.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn lock<'a>(
        host: &'a State<'a, Mutex<Host>>,
    ) -> Result<std::sync::MutexGuard<'a, Host>, String> {
        Ok(host.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn push(app: &AppHandle, host: &Host) -> Result<(), String> {
        app.emit("snapshot", host.snapshot())
            .map_err(|e| e.to_string())
    }

    impl Host {
        fn open(
            cfg: Config,
            save_to: PathBuf,
            has_file: bool,
            rt: Handle,
            started: Arc<AtomicBool>,
        ) -> Result<Self> {
            let client = reqwest::Client::builder().build()?;
            let (state, stt_models, cleanup_models) = prefill(&cfg, has_file);
            let devices = crate::audio::device_names().unwrap_or_default();
            let mut host = Self {
                cfg,
                save_to,
                state,
                stt_models,
                cleanup_models,
                devices,
                rt,
                client,
                download_cancel: CancellationToken::new(),
                session_cancel: CancellationToken::new(),
                busy: false,
                worker: None,
                pending_start: false,
                started,
                dev_marks: 0,
                extra_stt: None,
                extra_cleanup: None,
            };
            if let Some(id) = host.state.stt.as_ref().map(|s| s.id.clone()) {
                host.note_catalogue(SlotKind::Stt, &id);
            }
            if let Some(id) = host.state.cleanup.as_ref().map(|s| s.id.clone()) {
                host.note_catalogue(SlotKind::Cleanup, &id);
            }
            Ok(host)
        }

        fn snapshot(&self) -> Snapshot {
            Snapshot {
                state: self.state.clone(),
            }
        }

        fn shutdown(&mut self) {
            self.download_cancel.cancel();
            self.session_cancel.cancel();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }

        fn models(&self, kind: SlotKind) -> &[Model] {
            match kind {
                SlotKind::Stt => &self.stt_models,
                SlotKind::Cleanup => &self.cleanup_models,
            }
        }

        fn extra(&mut self, kind: SlotKind) -> &mut Option<FetchSpec> {
            match kind {
                SlotKind::Stt => &mut self.extra_stt,
                SlotKind::Cleanup => &mut self.extra_cleanup,
            }
        }

        fn spec(&self, kind: SlotKind) -> Option<FetchSpec> {
            match kind {
                SlotKind::Stt => self.extra_stt.clone(),
                SlotKind::Cleanup => self.extra_cleanup.clone(),
            }
        }

        fn note_catalogue(&mut self, kind: SlotKind, id: &str) {
            let found = self
                .models(kind)
                .iter()
                .find(|m| m.id == id)
                .map(|m| FetchSpec {
                    url: m.url.clone(),
                    sha256: m.sha256.clone(),
                });
            if let Some(spec) = found {
                *self.extra(kind) = Some(spec);
            }
        }

        fn choices(&self, kind: SlotKind, query: &str) -> Vec<ModelChoice> {
            let q = query.trim().to_ascii_lowercase();
            self.models(kind)
                .iter()
                .filter(|m| {
                    q.is_empty()
                        || m.id.to_ascii_lowercase().contains(&q)
                        || m.name.to_ascii_lowercase().contains(&q)
                })
                .map(|m| {
                    let path = dest_for(m);
                    ModelChoice {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        licence: m.licence.clone(),
                        path: path.display().to_string(),
                        present: path.is_file(),
                    }
                })
                .collect()
        }

        fn device_rows(&self, query: &str) -> Vec<DeviceRow> {
            let q = query.trim().to_ascii_lowercase();
            let mut rows = Vec::new();
            if q.is_empty() || "default".starts_with(&q) {
                rows.push(DeviceRow {
                    name: None,
                    label: "Default".into(),
                });
            }
            for name in &self.devices {
                if q.is_empty() || name.to_ascii_lowercase().contains(&q) {
                    rows.push(DeviceRow {
                        name: Some(name.clone()),
                        label: name.clone(),
                    });
                }
            }
            rows
        }

        fn dev_progress(&mut self, kind: SlotKind, got: u64, total: u64) {
            let id = match kind {
                SlotKind::Stt => self.state.stt.as_ref().map(|s| s.id.clone()),
                SlotKind::Cleanup => self.state.cleanup.as_ref().map(|s| s.id.clone()),
            };
            let Some(id) = id else { return };
            let mark = got.saturating_mul(4).checked_div(total).unwrap_or(0) as u8;
            if mark != self.dev_marks || got == total {
                self.dev_marks = mark;
                emit(&DevEvent::Download {
                    id: &id,
                    got,
                    total,
                });
            }
        }

        fn fail(&mut self, message: String) {
            self.busy = false;
            self.pending_start = false;
            self.state.apply(Event::Cancel {
                kind: SlotKind::Stt,
            });
            self.state.apply(Event::Cancel {
                kind: SlotKind::Cleanup,
            });
            self.state.apply(Event::Error { message });
        }

        fn needs_file(&self) -> bool {
            self.state.stt.as_ref().is_none_or(|s| !s.present)
                || self.state.cleanup.as_ref().is_none_or(|s| !s.present)
        }

        fn start(&mut self, app: &AppHandle) -> Result<()> {
            if self.busy {
                return Ok(());
            }
            if self.needs_file() {
                self.pending_start = true;
                self.kick_next(app)?;
                return Ok(());
            }
            self.pending_start = false;
            self.launch()
        }

        fn kick_next(&mut self, app: &AppHandle) -> Result<()> {
            let kind = if self.state.stt.as_ref().is_none_or(|s| !s.present) {
                SlotKind::Stt
            } else {
                SlotKind::Cleanup
            };
            let Some(slot) = (match kind {
                SlotKind::Stt => self.state.stt.clone(),
                SlotKind::Cleanup => self.state.cleanup.clone(),
            }) else {
                self.state.apply(Event::Start);
                self.pending_start = false;
                return Ok(());
            };
            if self.spec(kind).is_none() {
                self.state.apply(Event::Start);
                self.pending_start = false;
                return Ok(());
            }
            self.spawn_download(app, kind, slot.path, slot.id)
        }

        fn spawn_download(
            &mut self,
            app: &AppHandle,
            kind: SlotKind,
            path: String,
            id: String,
        ) -> Result<()> {
            let Some(spec) = self.spec(kind) else {
                self.fail(format!("{id}: no download"));
                return Ok(());
            };
            self.download_cancel.cancel();
            self.download_cancel = CancellationToken::new();
            self.busy = true;
            self.dev_marks = 0;
            self.state.apply(Event::Progress {
                kind,
                got: 0,
                total: 0,
            });
            let cancel = self.download_cancel.clone();
            let client = self.client.clone();
            let app = app.clone();
            self.rt.spawn(async move {
                let app_progress = app.clone();
                let res = fetch(
                    &client,
                    &spec.url,
                    Path::new(&path),
                    &spec.sha256,
                    move |got, total| {
                        let host = app_progress.state::<Mutex<Host>>();
                        let mut host = host.lock().unwrap_or_else(|e| e.into_inner());
                        host.state.apply(Event::Progress { kind, got, total });
                        host.dev_progress(kind, got, total);
                        let snap = host.snapshot();
                        drop(host);
                        let _ = app_progress.emit("snapshot", snap);
                    },
                    &cancel,
                )
                .await;
                let host = app.state::<Mutex<Host>>();
                let mut host = host.lock().unwrap_or_else(|e| e.into_inner());
                match res {
                    Ok(()) => {
                        host.busy = false;
                        host.state.apply(Event::Downloaded { kind });
                        if host.pending_start {
                            if let Err(err) = host.start(&app) {
                                host.fail(err.to_string());
                            }
                        }
                    }
                    Err(err) => host.fail(format!("{id}: {err}")),
                }
                let snap = host.snapshot();
                drop(host);
                let _ = app.emit("snapshot", snap);
            });
            Ok(())
        }

        fn launch(&mut self) -> Result<()> {
            self.state.apply(Event::Start);
            if !self.state.running {
                return Ok(());
            }
            if let Some(slot) = &self.state.stt {
                self.cfg.whisper_model = PathBuf::from(&slot.path);
            }
            if let Some(slot) = &self.state.cleanup {
                self.cfg.llama_model = PathBuf::from(&slot.path);
                self.cfg.cleanup = true;
            }
            self.cfg.language = self.state.language.clone();
            self.cfg.prompt_carry = self.state.prompt_carry;
            self.cfg.device = self.state.device.clone();
            if let Err(err) = self.cfg.check_files() {
                self.state.apply(Event::Error {
                    message: err.to_string(),
                });
                return Ok(());
            }
            if let Err(err) = self.cfg.save(&self.save_to) {
                self.state.apply(Event::Error {
                    message: err.to_string(),
                });
                return Ok(());
            }
            let cfg = self.cfg.clone();
            self.session_cancel = CancellationToken::new();
            let cancel = self.session_cancel.clone();
            let (cues, _) = tokio::sync::broadcast::channel(cfg.channel_size);
            let rt = self.rt.clone();
            self.worker = Some(std::thread::spawn(move || {
                if let Err(err) = rt.block_on(crate::session::run(cfg, cues, cancel)) {
                    tracing::error!(?err, "session");
                }
            }));
            self.started.store(true, Ordering::Relaxed);
            Ok(())
        }

        fn stop_capture(&mut self) {
            self.session_cancel.cancel();
            self.session_cancel = CancellationToken::new();
            self.state.apply(Event::Stop);
        }

        fn mark_removed_if_gone(&mut self, dir: &Path) {
            for kind in [SlotKind::Stt, SlotKind::Cleanup] {
                let Some(path) = (match kind {
                    SlotKind::Stt => self.state.stt.as_ref().map(|s| s.path.clone()),
                    SlotKind::Cleanup => self.state.cleanup.as_ref().map(|s| s.path.clone()),
                }) else {
                    continue;
                };
                let path = PathBuf::from(path);
                if path.starts_with(dir) && !path.is_file() {
                    self.state.apply(Event::Removed { kind });
                }
            }
        }
    }

    fn prefill(cfg: &Config, has_file: bool) -> (SettingsState, Vec<Model>, Vec<Model>) {
        let cat = crate::models::bundled();
        let hw = Hardware::detect();
        let stt_models = ordered(&cat, Kind::Stt, &hw);
        let cleanup_models = ordered(&cat, Kind::Cleanup, &hw);
        let mut state = SettingsState {
            language: cfg.language.clone(),
            prompt_carry: cfg.prompt_carry,
            device: cfg.device.clone(),
            ..SettingsState::default()
        };
        if has_file {
            select_path(&mut state, SlotKind::Stt, &stt_models, &cfg.whisper_model);
            select_path(
                &mut state,
                SlotKind::Cleanup,
                &cleanup_models,
                &cfg.llama_model,
            );
        } else if let Some(model) = stt_models.first() {
            select_model(&mut state, SlotKind::Stt, model);
            if let Some(model) = cleanup_models.first() {
                select_model(&mut state, SlotKind::Cleanup, model);
            }
        } else if let Some(model) = cleanup_models.first() {
            select_model(&mut state, SlotKind::Cleanup, model);
        }
        (state, stt_models, cleanup_models)
    }

    fn ordered(cat: &Catalogue, kind: Kind, hw: &Hardware) -> Vec<Model> {
        let mut out: Vec<Model> = crate::models::recommend(hw, cat, kind)
            .into_iter()
            .map(|r| r.model.clone())
            .collect();
        for model in &cat.model {
            if model.kind == kind && out.iter().all(|e| e.id != model.id) {
                out.push(model.clone());
            }
        }
        out
    }

    fn select_model(state: &mut SettingsState, kind: SlotKind, model: &Model) {
        let path = dest_for(model);
        state.apply(Event::Select {
            kind,
            id: model.id.clone(),
            path: path.display().to_string(),
            licence: model.licence.clone(),
            present: path.is_file(),
        });
    }

    fn select_path(state: &mut SettingsState, kind: SlotKind, models: &[Model], path: &Path) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let found = models.iter().find(|m| url_file_name(&m.url) == name);
        state.apply(Event::Select {
            kind,
            id: found.map(|m| m.id.clone()).unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("model")
                    .into()
            }),
            licence: found
                .map(|m| m.licence.clone())
                .unwrap_or_else(|| "local".into()),
            path: path.display().to_string(),
            present: path.is_file(),
        });
    }

    fn dest_for(model: &Model) -> PathBuf {
        let name = url_file_name(&model.url);
        let dir = model_dir().unwrap_or_else(|| PathBuf::from("models"));
        let in_data = dir.join(name);
        if in_data.is_file() {
            return in_data;
        }
        let in_cwd = PathBuf::from("models").join(name);
        if in_cwd.is_file() {
            return in_cwd;
        }
        in_data
    }

    #[tauri::command]
    fn snapshot(host: State<Mutex<Host>>) -> Result<Snapshot, String> {
        Ok(lock(&host)?.snapshot())
    }

    #[tauri::command]
    fn event(app: AppHandle, host: State<Mutex<Host>>, event: Event) -> Result<(), String> {
        let mut host = lock(&host)?;
        match event {
            Event::Select {
                kind,
                id,
                path,
                licence,
                present,
            } => {
                host.state.apply(Event::Select {
                    kind,
                    id: id.clone(),
                    path,
                    licence,
                    present,
                });
                host.note_catalogue(kind, &id);
                emit(&DevEvent::ModelChosen {
                    kind: kind.as_str(),
                    id: &id,
                });
            }
            Event::Language { .. } | Event::PromptCarry { .. } | Event::Device { .. } => {
                host.state.apply(event);
            }
            _ => return Err("not a settings event".into()),
        }
        push(&app, &host)
    }

    #[tauri::command]
    fn models(
        host: State<Mutex<Host>>,
        kind: SlotKind,
        query: String,
    ) -> Result<Vec<ModelChoice>, String> {
        Ok(lock(&host)?.choices(kind, &query))
    }

    #[tauri::command]
    fn languages(query: String) -> Vec<LangRow> {
        crate::settings::filter_languages(&query)
            .into_iter()
            .map(|lang| LangRow {
                code: lang.code.into(),
                name: lang.name.into(),
            })
            .collect()
    }

    #[tauri::command]
    fn devices(host: State<Mutex<Host>>, query: String) -> Result<Vec<DeviceRow>, String> {
        Ok(lock(&host)?.device_rows(&query))
    }

    #[tauri::command]
    async fn search_hf(host: State<'_, Mutex<Host>>, query: String) -> Result<Vec<HfHit>, String> {
        let client = lock(&host)?.client.clone();
        let hub = HfHub::new(client);
        let hits = hub.search(&query).await.map_err(|e| e.to_string())?;
        Ok(hits
            .into_iter()
            .map(|hit| HfHit {
                repo: hit.repo,
                filename: hit.filename,
                size: hit.size,
                sha256: hit.sha256,
                licence: hit.licence,
                url: hit.url,
            })
            .collect())
    }

    #[tauri::command]
    fn pick_hf(app: AppHandle, host: State<Mutex<Host>>, hit: HfHit) -> Result<(), String> {
        let kind = if hit.filename.to_ascii_lowercase().ends_with(".gguf") {
            SlotKind::Cleanup
        } else {
            SlotKind::Stt
        };
        let dir = model_dir().unwrap_or_else(|| PathBuf::from("models"));
        let path = dir.join(&hit.filename);
        let mut host = lock(&host)?;
        host.state.apply(Event::Select {
            kind,
            id: hit.filename.clone(),
            path: path.display().to_string(),
            licence: hit.licence,
            present: path.is_file(),
        });
        *host.extra(kind) = Some(FetchSpec {
            url: hit.url,
            sha256: hit.sha256,
        });
        emit(&DevEvent::ModelChosen {
            kind: kind.as_str(),
            id: &hit.filename,
        });
        push(&app, &host)
    }

    #[tauri::command]
    fn browse(app: AppHandle, kind: SlotKind) {
        let app2 = app.clone();
        app.dialog()
            .file()
            .add_filter("model", &["bin", "gguf"])
            .pick_file(move |picked| {
                let Some(picked) = picked else { return };
                let Ok(path) = picked.into_path() else { return };
                let host = app2.state::<Mutex<Host>>();
                let mut host = host.lock().unwrap_or_else(|e| e.into_inner());
                match validate_file(&path) {
                    Ok(()) => {
                        let id = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("model")
                            .to_string();
                        host.state.apply(Event::Select {
                            kind,
                            id: id.clone(),
                            path: path.display().to_string(),
                            licence: "local".into(),
                            present: true,
                        });
                        *host.extra(kind) = None;
                        emit(&DevEvent::ModelChosen {
                            kind: kind.as_str(),
                            id: &id,
                        });
                    }
                    Err(err) => host.state.apply(Event::Error {
                        message: err.to_string(),
                    }),
                }
                let snap = host.snapshot();
                drop(host);
                let _ = app2.emit("snapshot", snap);
            });
    }

    #[tauri::command]
    fn download(app: AppHandle, host: State<Mutex<Host>>, kind: SlotKind) -> Result<(), String> {
        let mut host = lock(&host)?;
        let Some(slot) = (match kind {
            SlotKind::Stt => host.state.stt.clone(),
            SlotKind::Cleanup => host.state.cleanup.clone(),
        }) else {
            host.state.apply(Event::Error {
                message: "no model".into(),
            });
            return push(&app, &host);
        };
        host.pending_start = false;
        host.spawn_download(&app, kind, slot.path, slot.id)
            .map_err(|e| e.to_string())?;
        push(&app, &host)
    }

    #[tauri::command]
    fn cancel_download(
        app: AppHandle,
        host: State<Mutex<Host>>,
        kind: SlotKind,
    ) -> Result<(), String> {
        let mut host = lock(&host)?;
        host.download_cancel.cancel();
        host.download_cancel = CancellationToken::new();
        host.busy = false;
        host.pending_start = false;
        host.state.apply(Event::Cancel { kind });
        push(&app, &host)
    }

    #[tauri::command]
    fn start(app: AppHandle, host: State<Mutex<Host>>) -> Result<(), String> {
        let mut host = lock(&host)?;
        host.start(&app).map_err(|e| e.to_string())?;
        push(&app, &host)
    }

    #[tauri::command]
    fn stop(app: AppHandle, host: State<Mutex<Host>>) -> Result<(), String> {
        let mut host = lock(&host)?;
        host.download_cancel.cancel();
        host.busy = false;
        host.pending_start = false;
        host.stop_capture();
        push(&app, &host)
    }

    #[tauri::command]
    fn remove_one(app: AppHandle, host: State<Mutex<Host>>, kind: SlotKind) -> Result<(), String> {
        let mut host = lock(&host)?;
        let Some(path) = (match kind {
            SlotKind::Stt => host.state.stt.as_ref().map(|s| s.path.clone()),
            SlotKind::Cleanup => host.state.cleanup.as_ref().map(|s| s.path.clone()),
        }) else {
            host.state.apply(Event::Error {
                message: "no model".into(),
            });
            return push(&app, &host);
        };
        let Some(dir) = model_dir() else {
            host.state.apply(Event::Error {
                message: "no models dir".into(),
            });
            return push(&app, &host);
        };
        if let Err(err) = remove_in(&dir, Path::new(&path)) {
            host.state.apply(Event::Error {
                message: err.to_string(),
            });
            return push(&app, &host);
        }
        host.download_cancel.cancel();
        host.busy = false;
        host.pending_start = false;
        host.stop_capture();
        host.state.apply(Event::Removed { kind });
        push(&app, &host)
    }

    #[tauri::command]
    fn remove_all(app: AppHandle, host: State<Mutex<Host>>) -> Result<(), String> {
        let mut host = lock(&host)?;
        let Some(dir) = model_dir() else {
            host.state.apply(Event::Error {
                message: "no models dir".into(),
            });
            return push(&app, &host);
        };
        if let Err(err) = remove_all_in(&dir) {
            host.state.apply(Event::Error {
                message: err.to_string(),
            });
            return push(&app, &host);
        }
        host.download_cancel.cancel();
        host.busy = false;
        host.pending_start = false;
        host.stop_capture();
        host.mark_removed_if_gone(&dir);
        push(&app, &host)
    }
}

#[cfg(feature = "settings-ui")]
pub use window::run;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_target_uses_the_explicit_path() {
        let path = save_target(Some(Path::new("custom.toml")));
        assert_eq!(path, PathBuf::from("custom.toml"));
    }
}
