//! The optional file manifest on the Game folder page: a button, then a progress bar with speed
//! and time left, a Cancel button, and the result.

use eframe::egui::{self, RichText, Ui, vec2};

use crate::app::App;
use crate::app_check::ManifestState;
use crate::manifest::manifest_text;
use crate::theme;

pub fn section(app: &mut App, ui: &mut Ui) {
    ui.add_space(theme::SP_M);
    ui.label(
        RichText::new("File manifest (optional)")
            .size(18.0)
            .strong(),
    );
    ui.label(
        RichText::new(
            "SHA-256 of the required files and the .rpf archives. It reads every byte (minutes for \
             130 GB, then remembered) and is never needed to play.",
        )
        .color(theme::TEXT_MUTED),
    );
    ui.add_space(theme::SP_S);
    match app.manifest.clone() {
        ManifestState::Running(progress) => {
            theme::gradient_bar(ui, Some(progress.fraction()), &progress.describe());
            if theme::glow_button(ui, true, "Cancel", vec2(0.0, 36.0), false).clicked() {
                app.cancel_manifest();
            }
        }
        ManifestState::Done(entries) => {
            let text = manifest_text(&entries);
            ui.collapsing("Show the manifest", |ui| {
                let mut shown = text.clone();
                ui.add(
                    egui::TextEdit::multiline(&mut shown)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
                if ui.button("Copy manifest").clicked() {
                    ui.ctx().copy_text(text.clone());
                }
            });
            start_button(app, ui, "Compute again");
        }
        ManifestState::Failed(error) => {
            ui.colored_label(theme::BAD, format!("The manifest failed: {error}"));
            start_button(app, ui, "Try again");
        }
        ManifestState::Cancelled => {
            ui.label("Cancelled. Files already hashed are remembered.");
            start_button(app, ui, "Compute file manifest");
        }
        ManifestState::NotRun => start_button(app, ui, "Compute file manifest"),
    }
}

fn start_button(app: &mut App, ui: &mut Ui, label: &str) {
    if theme::glow_button(ui, true, label, vec2(0.0, 36.0), false).clicked() {
        app.start_manifest();
    }
}
