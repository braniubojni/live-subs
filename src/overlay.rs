//! Transparent, always-on-top, click-through subtitle window. Thin on purpose:
//! the logic lives in `SubtitleState`, this only renders it.

use crate::config::OverlayConfig;
use crate::pipeline::Cue;
use crate::subtitles::SubtitleState;
use anyhow::{anyhow, Result};
use eframe::egui::{self, Align, Color32, Layout, Pos2, RichText, Vec2, ViewportCommand};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tokio_util::sync::CancellationToken;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Gap between the window and the bottom of the screen.
const BOTTOM_MARGIN: f32 = 60.0;

/// Bottom-centre of a monitor.
fn place(monitor: Vec2, size: Vec2) -> Pos2 {
    Pos2::new(
        (monitor.x - size.x) / 2.0,
        monitor.y - size.y - BOTTOM_MARGIN,
    )
}

struct Overlay {
    cfg: OverlayConfig,
    state: Arc<Mutex<SubtitleState>>,
    cancel: CancellationToken,
    placed: bool,
    tray: Option<Tray>,
}

/// Menu-bar / system-tray control. The icon must stay alive or it vanishes.
struct Tray {
    _icon: TrayIcon,
    pause: MenuItem,
    quit: MenuId,
    events: mpsc::Receiver<MenuId>,
}

impl Tray {
    /// Must run on the main thread after the event loop starts (macOS).
    fn new(ctx: egui::Context) -> Result<Self> {
        let menu = Menu::new();
        let pause = MenuItem::new("Pause", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[&pause, &quit])?;
        let (tx, events) = mpsc::channel();
        // The handler runs on the winit thread; wake the window to drain `events`.
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e.id);
            ctx.request_repaint();
        }));
        // ponytail: flat white square, a real icon comes with packaging (10).
        let icon = Icon::from_rgba(vec![255; 16 * 16 * 4], 16, 16)?;
        let _icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("live-subs")
            .build()?;
        Ok(Self {
            _icon,
            pause,
            quit: quit.id().clone(),
            events,
        })
    }
}

impl Overlay {
    fn size(&self, monitor_w: f32) -> Vec2 {
        let h = self.cfg.font_size * 1.6 * self.cfg.max_lines as f32 + 24.0;
        Vec2::new(monitor_w * self.cfg.width_frac, h)
    }
}

impl eframe::App for Overlay {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.cancel.is_cancelled() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if !self.placed {
            // Monitor size is unknown until the first frame.
            if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
                let size = self.size(m.x);
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(place(m, size)));
                self.placed = true;
            }
        }
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = &self.tray {
            while let Ok(id) = t.events.try_recv() {
                if id == t.quit {
                    self.cancel.cancel();
                } else if id == *t.pause.id() {
                    let paused = !state.is_paused();
                    state.set_paused(paused);
                    t.pause.set_text(if paused { "Resume" } else { "Pause" });
                }
            }
        }
        state.expire(now);
        if let Some(at) = state.next_expiry() {
            ctx.request_repaint_after(at.saturating_duration_since(now));
        }
        let size = self.cfg.font_size;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    // bottom_up puts the first item lowest: newest line first.
                    for line in state.lines().collect::<Vec<_>>().into_iter().rev() {
                        // Same size for partial and final: no layout jump on the snap.
                        let text = RichText::new(&line.text).size(size);
                        let text = if line.is_partial {
                            text.italics().color(Color32::from_gray(170))
                        } else {
                            text.color(Color32::WHITE)
                        };
                        egui::Frame::new()
                            .fill(Color32::from_black_alpha(190))
                            .corner_radius(6.0)
                            .inner_margin(egui::Margin::symmetric(12, 4))
                            .show(ui, |ui| ui.label(text));
                    }
                });
            });
    }
}

/// Run the window on the calling thread (macOS needs the main thread) until it
/// closes or `cancel` fires. Cancels `cancel` on the way out.
pub fn run(
    cfg: OverlayConfig,
    mut cues: Receiver<Cue>,
    rt: tokio::runtime::Handle,
    cancel: CancellationToken,
) -> Result<()> {
    let state = Arc::new(Mutex::new(SubtitleState::new(
        Duration::from_millis(cfg.ttl_ms),
        cfg.max_lines,
    )));
    let viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_mouse_passthrough(true)
        .with_taskbar(false)
        .with_inner_size([800.0, cfg.font_size * 1.6 * cfg.max_lines as f32 + 24.0]);
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let app_cancel = cancel.clone();
    let app_state = state.clone();
    let res = eframe::run_native(
        "live-subs",
        opts,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let st = app_state.clone();
            let c = app_cancel.clone();
            let ctx2 = ctx.clone();
            let ctx3 = ctx.clone();
            rt.spawn(async move {
                loop {
                    match cues.recv().await {
                        Ok(cue) => {
                            st.lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .apply(&cue, Instant::now());
                            ctx.request_repaint();
                        }
                        Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => break,
                    }
                }
            });
            // Wake the window so it sees the cancel and closes.
            rt.spawn(async move {
                c.cancelled().await;
                ctx2.request_repaint();
            });
            let tray = Tray::new(ctx3)
                .map_err(|e| tracing::warn!("tray unavailable: {e}"))
                .ok();
            Ok(Box::new(Overlay {
                tray,
                cfg,
                state: app_state,
                cancel: app_cancel,
                placed: false,
            }))
        }),
    );
    cancel.cancel();
    res.map_err(|e| anyhow!("overlay: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_bottom_centre() {
        let p = place(Vec2::new(1000.0, 800.0), Vec2::new(800.0, 100.0));
        assert_eq!(p, Pos2::new(100.0, 800.0 - 100.0 - BOTTOM_MARGIN));
    }
}
