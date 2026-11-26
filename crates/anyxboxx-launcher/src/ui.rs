//! The window: the navigation, the page on screen, the console drawer, the toasts and the splash.
//! The pages are in `ui_home`, `ui_pages` and `ui_options`; every decision lives in the other
//! modules.

use std::time::Duration;

use eframe::egui;

use crate::app::{App, Tab};
use crate::{anim, home, theme, toast, tokens, ui_chrome, ui_home, ui_launch, ui_pages};

/// How often the window redraws while a job or a folder check runs.
const BUSY_REPAINT: Duration = Duration::from_millis(200);
/// Distance a page slides up while it fades in.
const PAGE_SLIDE: f32 = 12.0;
/// Seconds between two looks at the shader cache folder.
const CACHE_CHECK_EVERY: f64 = 2.0;

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if std::mem::take(&mut self.close_requested) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.is_busy() {
            ctx.request_repaint_after(BUSY_REPAINT);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.handle_drops(&ctx);
        self.refresh_chips(now);
        if let Some(notice) = self.notice.take() {
            let kind = toast::classify(&notice);
            self.toasts.push(notice, kind, now);
        }
        theme::paint_background(&ctx);
        let open_frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 16));
        egui::Panel::left("nav")
            .frame(open_frame)
            .exact_size(ui_chrome::SIDEBAR_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .show_inside(ui, |ui| ui_chrome::sidebar(self, ui));
        egui::Panel::bottom("console_head")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 8)))
            .show_inside(ui, |ui| ui_chrome::console_header(self, ui));
        let drawer = egui::Panel::bottom("console_body")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 0)))
            .exact_size(ui_chrome::console_height())
            .resizable(false);
        let open = self.drawer_open;
        drawer.show_animated_inside(ui, open, |ui| ui_chrome::console_body(self, ui));
        egui::CentralPanel::default()
            .frame(open_frame)
            .show_inside(ui, |ui| self.show_page(ui, now));
        ui_pages::confirm_clean(self, &ctx);
        ui_chrome::toasts(self, &ctx);
        ui_launch::show(self, &ctx);
        self.save_settings_if_changed();
    }
}

impl App {
    /// The page, fading and sliding in when it was just switched to.
    fn show_page(&mut self, ui: &mut egui::Ui, now: f64) {
        if self.tab != self.shown_tab {
            self.shown_tab = self.tab;
            self.tab_changed_at = now;
        }
        let progress = anim::eased_progress(now, self.tab_changed_at, tokens::PAGE_TIME);
        if progress < 1.0 {
            ui.ctx().request_repaint();
            ui.set_opacity(progress);
            ui.add_space((1.0 - progress) * PAGE_SLIDE);
        }
        egui::ScrollArea::vertical()
            .id_salt(("page", self.tab as u8))
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                Tab::Home => ui_home::home(self, ui),
                Tab::Setup => ui_pages::setup(self, ui),
                Tab::Play => ui_pages::play(self, ui),
                Tab::Files => ui_pages::files(self, ui),
                Tab::Shaders => ui_pages::shaders(self, ui),
            });
    }

    /// Looks at the shader cache folder now and then, for the Home page's chip.
    fn refresh_chips(&mut self, now: f64) {
        if now - self.cache_checked_at >= CACHE_CHECK_EVERY || self.cache_checked_at == 0.0 {
            self.cache_checked_at = now.max(f64::MIN_POSITIVE);
            self.cache_ready = home::cache_filled(self.cache_dir.as_deref());
        }
    }
}
