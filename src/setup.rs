//! Settings window. Renders [`crate::settings::SettingsState`] and nothing else.

use crate::config::{user_config_file, Config};
use crate::models::{
    fetch, model_dir, recommend, url_file_name, validate_file, Catalogue, Hardware, HfHub, Kind,
    Model, ModelSearch,
};
use crate::settings::{emit, DevEvent, Event, SettingsState, SlotKind};
use anyhow::{anyhow, Result};
use eframe::egui::{self, Color32};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

enum Msg {
    Progress {
        kind: SlotKind,
        got: u64,
        total: u64,
    },
    Done(SlotKind),
    Failed(String),
    Hits(Vec<crate::models::RemoteFile>),
}

struct Setup {
    cfg: Config,
    save_to: PathBuf,
    state: SettingsState,
    stt_models: Vec<Model>,
    cleanup_models: Vec<Model>,
    stt_query: String,
    cleanup_query: String,
    lang_query: String,
    hf_query: String,
    hits: Vec<crate::models::RemoteFile>,
    devices: Vec<String>,
    rt: Handle,
    client: reqwest::Client,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    cancel: CancellationToken,
    busy: bool,
    worker: Option<std::thread::JoinHandle<()>>,
    pending_start: bool,
    started: bool,
    started_flag: std::sync::Arc<Mutex<bool>>,
    opened: bool,
    /// Quartile last printed for the in-flight download, so `--dev` is not one line per chunk.
    dev_marks: u8,
}

pub fn run(cfg: Config, save_to: PathBuf, has_file: bool, rt: Handle) -> Result<bool> {
    let (tx, rx) = mpsc::channel();
    let started_flag = std::sync::Arc::new(Mutex::new(false));
    let flag = started_flag.clone();
    let client = reqwest::Client::builder().build()?;
    let (state, stt_models, cleanup_models) = prefill(&cfg, has_file);
    let devices = crate::audio::device_names().unwrap_or_default();
    let viewport = egui::ViewportBuilder::default().with_inner_size([520.0, 640.0]);
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let app_rt = rt.clone();
    eframe::run_native(
        "live-subs",
        opts,
        Box::new(move |_cc| {
            Ok(Box::new(Setup {
                cfg,
                save_to,
                state,
                stt_models,
                cleanup_models,
                stt_query: String::new(),
                cleanup_query: String::new(),
                lang_query: String::new(),
                hf_query: String::new(),
                hits: Vec::new(),
                devices,
                rt: app_rt,
                client,
                tx,
                rx,
                cancel: CancellationToken::new(),
                busy: false,
                worker: None,
                pending_start: false,
                started: false,
                started_flag: flag,
                opened: false,
                dev_marks: 0,
            }))
        }),
    )
    .map_err(|e| anyhow!("settings: {e}"))?;
    let started = *started_flag.lock().unwrap_or_else(|e| e.into_inner());
    Ok(started)
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        *self.started_flag.lock().unwrap_or_else(|e| e.into_inner()) = self.started;
    }
}

/// `has_file` is whether a config file was on disk before this window opened.
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
    } else {
        if let Some(m) = stt_models.first() {
            select_model(&mut state, SlotKind::Stt, m);
        }
        if let Some(m) = cleanup_models.first() {
            select_model(&mut state, SlotKind::Cleanup, m);
        }
    }
    (state, stt_models, cleanup_models)
}

fn ordered(cat: &Catalogue, kind: Kind, hw: &Hardware) -> Vec<Model> {
    let mut out: Vec<Model> = recommend(hw, cat, kind)
        .into_iter()
        .map(|r| r.model.clone())
        .collect();
    for m in &cat.model {
        if m.kind == kind && out.iter().all(|e| e.id != m.id) {
            out.push(m.clone());
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

impl Setup {
    fn slot_models(&self, kind: SlotKind) -> &[Model] {
        match kind {
            SlotKind::Stt => &self.stt_models,
            SlotKind::Cleanup => &self.cleanup_models,
        }
    }

    fn query_mut(&mut self, kind: SlotKind) -> &mut String {
        match kind {
            SlotKind::Stt => &mut self.stt_query,
            SlotKind::Cleanup => &mut self.cleanup_query,
        }
    }

    fn filtered(&self, kind: SlotKind) -> Vec<&Model> {
        let q = match kind {
            SlotKind::Stt => self.stt_query.as_str(),
            SlotKind::Cleanup => self.cleanup_query.as_str(),
        };
        let q = q.trim().to_ascii_lowercase();
        self.slot_models(kind)
            .iter()
            .filter(|m| {
                q.is_empty()
                    || m.id.to_ascii_lowercase().contains(&q)
                    || m.name.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Progress { kind, got, total } => {
                    self.state.apply(Event::Progress { kind, got, total });
                    self.dev_progress(kind, got, total);
                }
                Msg::Done(kind) => {
                    self.state.apply(Event::Downloaded(kind));
                    self.busy = false;
                    if self.pending_start {
                        self.on_start(ctx);
                    }
                }
                Msg::Failed(err) => {
                    self.state.apply(Event::Cancel(SlotKind::Stt));
                    self.state.apply(Event::Cancel(SlotKind::Cleanup));
                    self.state.apply(Event::Error(err));
                    self.busy = false;
                    self.pending_start = false;
                }
                Msg::Hits(hits) => self.hits = hits,
            }
        }
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

    fn on_start(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        if self.needs_file() {
            self.pending_start = true;
            self.kick_next(ctx);
            return;
        }
        self.pending_start = false;
        self.launch(ctx);
    }

    fn needs_file(&self) -> bool {
        self.state.stt.as_ref().is_none_or(|s| !s.present)
            || self.state.cleanup.as_ref().is_none_or(|s| !s.present)
    }

    fn kick_next(&mut self, ctx: &egui::Context) {
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
            return;
        };
        let model = self
            .slot_models(kind)
            .iter()
            .find(|m| m.id == slot.id)
            .cloned();
        let Some(model) = model else {
            self.state.apply(Event::Start);
            self.pending_start = false;
            return;
        };
        self.spawn_download(ctx, kind, model.url, model.sha256, slot.path, model.id);
    }

    fn spawn_download(
        &mut self,
        ctx: &egui::Context,
        kind: SlotKind,
        url: String,
        sha: String,
        path: String,
        id: String,
    ) {
        self.cancel.cancel();
        self.cancel = CancellationToken::new();
        self.busy = true;
        self.dev_marks = 0;
        let cancel = self.cancel.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let client = self.client.clone();
        self.rt.spawn(async move {
            let tx2 = tx.clone();
            let ctx2 = ctx.clone();
            let res = fetch(
                &client,
                &url,
                Path::new(&path),
                &sha,
                move |got, total| {
                    let _ = tx2.send(Msg::Progress { kind, got, total });
                    ctx2.request_repaint();
                },
                &cancel,
            )
            .await;
            let _ = tx.send(match res {
                Ok(()) => Msg::Done(kind),
                Err(e) => Msg::Failed(format!("{id}: {e}")),
            });
            ctx.request_repaint();
        });
    }

    fn launch(&mut self, ctx: &egui::Context) {
        self.state.apply(Event::Start);
        if !self.state.running {
            return;
        }
        if let Some(s) = &self.state.stt {
            self.cfg.whisper_model = PathBuf::from(&s.path);
        }
        if let Some(s) = &self.state.cleanup {
            self.cfg.llama_model = PathBuf::from(&s.path);
            self.cfg.cleanup = true;
        }
        self.cfg.language = self.state.language.clone();
        self.cfg.prompt_carry = self.state.prompt_carry;
        self.cfg.device = self.state.device.clone();
        if let Err(e) = self.cfg.check_files() {
            self.state.apply(Event::Error(e.to_string()));
            return;
        }
        if let Err(e) = self.cfg.save(&self.save_to) {
            self.state.apply(Event::Error(e.to_string()));
            return;
        }
        let cfg = self.cfg.clone();
        let cancel = CancellationToken::new();
        self.cancel = cancel.clone();
        let (cues, _) = tokio::sync::broadcast::channel(cfg.channel_size);
        // `Capture` is not `Send`, so the session cannot run on the pool.
        let rt = self.rt.clone();
        self.worker = Some(std::thread::spawn(move || {
            if let Err(err) = rt.block_on(crate::session::run(cfg, cues, cancel)) {
                tracing::error!(?err, "session");
            }
        }));
        self.started = true;
        ctx.request_repaint();
    }

    fn pick(&mut self, kind: SlotKind, model: &Model) {
        select_model(&mut self.state, kind, model);
        emit(&DevEvent::ModelChosen {
            kind: kind.as_str(),
            id: &model.id,
        });
    }

    fn browse(&mut self, kind: SlotKind) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("model", &["bin", "gguf"])
            .pick_file()
        else {
            return;
        };
        match validate_file(&path) {
            Ok(()) => {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("model")
                    .to_string();
                self.state.apply(Event::Select {
                    kind,
                    id: id.clone(),
                    path: path.display().to_string(),
                    licence: "local".into(),
                    present: true,
                });
                emit(&DevEvent::ModelChosen {
                    kind: kind.as_str(),
                    id: &id,
                });
            }
            Err(e) => self.state.apply(Event::Error(e.to_string())),
        }
    }

    fn search_hf(&self, ctx: &egui::Context) {
        let q = self.hf_query.clone();
        let client = self.client.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.rt.spawn(async move {
            let hub = HfHub::new(client);
            let msg = match hub.search(&q).await {
                Ok(hits) => Msg::Hits(hits),
                Err(e) => Msg::Failed(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn model_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, kind: SlotKind) {
        let title = match kind {
            SlotKind::Stt => "Speech",
            SlotKind::Cleanup => "Cleanup",
        };
        ui.heading(title);
        let q = self.query_mut(kind);
        ui.text_edit_singleline(q);
        let rows: Vec<Model> = self.filtered(kind).into_iter().cloned().collect();
        let selected = match kind {
            SlotKind::Stt => self.state.stt.as_ref().map(|s| s.id.clone()),
            SlotKind::Cleanup => self.state.cleanup.as_ref().map(|s| s.id.clone()),
        };
        egui::ScrollArea::vertical()
            .id_salt(("models", kind.as_str()))
            .max_height(90.0)
            .show(ui, |ui| {
                for m in &rows {
                    let on = selected.as_deref() == Some(m.id.as_str());
                    if ui
                        .selectable_label(on, format!("{} — {}", m.name, m.id))
                        .clicked()
                    {
                        self.pick(kind, m);
                    }
                }
            });
        let slot = match kind {
            SlotKind::Stt => self.state.stt.clone(),
            SlotKind::Cleanup => self.state.cleanup.clone(),
        };
        if let Some(slot) = slot {
            ui.label(format!("Licence: {}", slot.licence));
            ui.horizontal(|ui| {
                if ui.button("Browse").clicked() {
                    self.browse(kind);
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Download"))
                    .clicked()
                {
                    if let Some(model) = self
                        .slot_models(kind)
                        .iter()
                        .find(|m| m.id == slot.id)
                        .cloned()
                    {
                        self.spawn_download(
                            ctx,
                            kind,
                            model.url,
                            model.sha256,
                            slot.path.clone(),
                            model.id,
                        );
                    }
                }
                if self.busy && ui.button("Cancel").clicked() {
                    self.cancel.cancel();
                    self.state.apply(Event::Cancel(kind));
                }
            });
            let progress = match kind {
                SlotKind::Stt => self.state.stt_progress,
                SlotKind::Cleanup => self.state.cleanup_progress,
            };
            if let Some(p) = progress {
                let frac = if p.total == 0 {
                    0.0
                } else {
                    p.got as f32 / p.total as f32
                };
                ui.add(egui::ProgressBar::new(frac).text(format!("{}/{}", p.got, p.total)));
            }
        } else if ui.button("Browse").clicked() {
            self.browse(kind);
        }
    }
}

impl eframe::App for Setup {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.opened {
            self.opened = true;
            emit(&DevEvent::WindowOpened);
        }
        self.poll(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.model_row(ui, ctx, SlotKind::Stt);
                ui.separator();
                self.model_row(ui, ctx, SlotKind::Cleanup);
                ui.separator();
                ui.heading("Hugging Face");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.hf_query);
                    if ui.button("Search").clicked() {
                        self.search_hf(ctx);
                    }
                });
                let hits = self.hits.clone();
                for hit in &hits {
                    ui.label(format!(
                        "{} ({} bytes, {})",
                        hit.filename, hit.size, hit.licence
                    ));
                    if ui.button(format!("Download {}", hit.filename)).clicked() && !self.busy {
                        let kind = if hit.filename.to_ascii_lowercase().ends_with(".gguf") {
                            SlotKind::Cleanup
                        } else {
                            SlotKind::Stt
                        };
                        let dir = model_dir().unwrap_or_else(|| PathBuf::from("models"));
                        let path = dir.join(&hit.filename);
                        self.state.apply(Event::Select {
                            kind,
                            id: hit.filename.clone(),
                            path: path.display().to_string(),
                            licence: hit.licence.clone(),
                            present: false,
                        });
                        self.spawn_download(
                            ctx,
                            kind,
                            hit.url.clone(),
                            hit.sha256.clone(),
                            path.display().to_string(),
                            hit.filename.clone(),
                        );
                    }
                }
                ui.separator();
                let label = self.state.device.as_deref().unwrap_or("Default");
                egui::ComboBox::from_label("Device")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.state.device.is_none(), "Default")
                            .clicked()
                        {
                            self.state.apply(Event::Device(None));
                        }
                        for n in self.devices.clone() {
                            if ui
                                .selectable_label(
                                    self.state.device.as_deref() == Some(n.as_str()),
                                    &n,
                                )
                                .clicked()
                            {
                                self.state.apply(Event::Device(Some(n)));
                            }
                        }
                    });
                ui.heading("Language");
                ui.text_edit_singleline(&mut self.lang_query);
                let langs = crate::settings::filter_languages(&self.lang_query);
                let current = self.state.language.clone();
                egui::ScrollArea::vertical()
                    .id_salt("langs")
                    .max_height(80.0)
                    .show(ui, |ui| {
                        for lang in langs {
                            let on = current == lang.code;
                            if ui
                                .selectable_label(on, format!("{} {}", lang.code, lang.name))
                                .clicked()
                            {
                                self.state.apply(Event::Language(lang.code.into()));
                            }
                        }
                    });
                let mut carry = self.state.prompt_carry;
                if ui
                    .checkbox(&mut carry, "Use the previous line as a hint")
                    .changed()
                {
                    self.state.apply(Event::PromptCarry(carry));
                }
                if let Some(err) = &self.state.error {
                    ui.colored_label(Color32::RED, err);
                }
                ui.horizontal(|ui| {
                    let label = if self.state.running {
                        "Running"
                    } else {
                        "Start"
                    };
                    if ui
                        .add_enabled(!self.state.running, egui::Button::new(label))
                        .clicked()
                    {
                        self.on_start(ctx);
                    }
                    if ui
                        .add_enabled(self.state.running, egui::Button::new("Stop"))
                        .clicked()
                    {
                        self.cancel.cancel();
                        self.state.apply(Event::Stop);
                    }
                });
            });
        });
    }
}

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
