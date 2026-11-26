//! The states a page can be in, drawn the same way everywhere: loading (a spinner and what is
//! happening), empty (a call to action), error (a red card with the message and "Copy log") and
//! success (a green card).

use eframe::egui::{self, Color32, Frame, Stroke, Ui, vec2};

use crate::app::App;
use crate::app_launch::Banner;
use crate::widgets::{self, Status};
use crate::{theme, tokens};

/// A spinner with the work in progress beside it.
pub fn loading(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(18.0).color(theme::HOT_PINK));
        ui.add_space(tokens::XS);
        ui.label(tokens::body(text));
    });
}

/// A card for an outcome: the icon, a title, the message, and its actions below.
pub fn outcome_card(
    ui: &mut Ui,
    status: Status,
    title: &str,
    message: &str,
    actions: impl FnOnce(&mut Ui),
) {
    status_card(ui, status, title, message, Some(Box::new(actions)));
}

/// A card for an outcome with nothing to do about it.
pub fn notice_card(ui: &mut Ui, status: Status, title: &str, message: &str) {
    status_card(ui, status, title, message, None);
}

type Actions<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

fn status_card(ui: &mut Ui, status: Status, title: &str, message: &str, actions: Option<Actions>) {
    let (fill, edge) = match status {
        Status::Bad => (theme::ERROR_FILL, theme::BAD),
        Status::Good => (theme::SUCCESS_FILL, theme::GOOD),
        Status::Warn => (theme::CHIP_FILL, theme::WARN),
    };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, edge.gamma_multiply(0.8)))
        .corner_radius(tokens::radius(tokens::R_CARD))
        .inner_margin(egui::Margin::same(tokens::CARD_PAD - 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::hover());
                widgets::status_icon(ui.painter(), rect.center(), 11.0, status);
                ui.label(tokens::subtitle(title).color(edge));
            });
            ui.add_space(tokens::XS);
            ui.label(tokens::body(message));
            if let Some(actions) = actions {
                ui.add_space(tokens::S);
                ui.horizontal(actions);
            }
        });
}

/// The outcome of the last job, when there is one: an error card with "Copy log", or a success
/// card. Both can be dismissed.
pub fn banner(app: &mut App, ui: &mut Ui) {
    let Some(banner) = app.banner.clone() else {
        return;
    };
    let mut dismiss = false;
    match &banner {
        Banner::Error(message) => {
            let log = app.log_text();
            outcome_card(ui, Status::Bad, "The launch stopped", message, |ui| {
                if widgets::secondary_button(ui, !log.is_empty(), "Copy log").clicked() {
                    ui.ctx().copy_text(log);
                    app.notice = Some("Log copied to the clipboard.".to_string());
                }
                if widgets::secondary_button(ui, true, "Show the console").clicked() {
                    app.drawer_open = true;
                }
                dismiss = widgets::secondary_button(ui, true, "Dismiss").clicked();
            });
        }
        Banner::Success(message) => {
            outcome_card(ui, Status::Good, "Done", message, |ui| {
                dismiss = widgets::secondary_button(ui, true, "Dismiss").clicked();
            });
        }
    }
    ui.add_space(tokens::M);
    if dismiss {
        app.banner = None;
    }
}

/// The Game folder page before any folder is set: what to pick, and the button that picks it.
pub fn empty_folder(app: &mut App, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(tokens::L);
        folder_glyph(ui);
        ui.add_space(tokens::M);
        ui.label(tokens::title("Choose your game folder"));
        ui.add_space(tokens::XS);
        ui.label(tokens::muted(
            "Pick the folder that holds game_scarlett_master.exe, or drop it on this window.",
        ));
        ui.label(tokens::muted(
            "Only its own files are read, and nothing is copied.",
        ));
        ui.add_space(tokens::L);
        if widgets::glow_button(ui, true, "Browse for the folder", vec2(260.0, 48.0), false)
            .clicked()
        {
            app.pick_game_folder();
        }
        ui.add_space(tokens::L);
    });
}

/// A folder drawn with rounded shapes, in the accent colours.
fn folder_glyph(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(72.0, 56.0), egui::Sense::hover());
    let painter = ui.painter();
    let tab = egui::Rect::from_min_size(rect.min, vec2(30.0, 14.0));
    painter.rect_filled(tab, tokens::radius(4), theme::MAGENTA);
    let body = egui::Rect::from_min_max(rect.min + vec2(0.0, 8.0), rect.max);
    painter.rect_filled(body, tokens::radius(6), theme::BUTTON_PINK);
    let shine = egui::Rect::from_min_max(body.min, egui::pos2(body.max.x, body.min.y + 14.0));
    painter.rect_filled(
        shine.shrink2(vec2(6.0, 4.0)),
        tokens::radius(3),
        theme::GLOW,
    );
    painter.rect_stroke(
        body,
        tokens::radius(6),
        Stroke::new(1.0_f32, Color32::WHITE.gamma_multiply(0.4)),
        egui::StrokeKind::Inside,
    );
}
