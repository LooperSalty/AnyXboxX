//! The launch card of the Play page: the profile, the XG library, the launch button, the advanced
//! panel and the folders the game uses.

use eframe::egui::{self, vec2};

use crate::app::{App, Verify};
use crate::{theme, tokens, ui_pages, widgets};

pub fn controls(app: &mut App, ui: &mut egui::Ui) {
    ui.label(tokens::subtitle("Launch"));
    ui.add_space(tokens::S);
    let profile = app.profile().clone();
    ui.label(tokens::body(format!("Profile: {}", profile.name)).strong());
    ui.label(tokens::muted(format!(
        "Status: {:?}. {}",
        profile.status, profile.note
    )));
    if !profile.pending.is_empty() {
        ui.label(
            tokens::body(format!(
                "Pending the maintainer's confirmation (set to 0 in this profile): {}",
                profile.pending.join(", ")
            ))
            .color(theme::WARN),
        );
    }
    ui.add_space(tokens::M);
    ui_pages::xg_row(app, ui);
    ui.add_space(tokens::M);
    let ready = matches!(&app.verify, Verify::Done(report) if report.is_ok());
    if !ready {
        ui.label(tokens::body("Check the game folder first (page 2).").color(theme::WARN));
        ui.add_space(tokens::S);
    }
    let can_launch = ready && !app.is_busy();
    if widgets::glow_button(
        ui,
        can_launch,
        "Launch game",
        vec2(200.0, 44.0),
        app.is_busy(),
    )
    .clicked()
    {
        app.start(false);
    }
    ui.add_space(tokens::M);
    advanced(app, ui);
    ui.add_space(tokens::M);
    folders(app, ui);
}

/// Where saves and shaders live, and the cache clean.
fn folders(app: &mut App, ui: &mut egui::Ui) {
    ui.label(tokens::caption(format!(
        "Saves and data: {}",
        app.data_dir_label()
    )));
    ui.label(tokens::caption(format!(
        "Shader cache: {}",
        app.cache_dir
            .as_ref()
            .map_or("unknown".to_string(), |d| d.display().to_string())
    )));
    ui.add_space(tokens::S);
    if widgets::secondary_button(ui, !app.is_busy(), "Clean shader cache").clicked() {
        app.confirm_clean = true;
    }
}

/// The advanced panel: overrides, the guard, and the environment the launch will use.
fn advanced(app: &mut App, ui: &mut egui::Ui) {
    ui.collapsing("Advanced", |ui| {
        ui.label(tokens::muted(
            "Overrides, one KEY=VALUE per line (VICTORY_ or VKD3D_ names). They win over the profile.",
        ));
        ui.add(
            egui::TextEdit::multiline(&mut app.settings.overrides)
                .desired_rows(5)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace),
        );
        ui.checkbox(
            &mut app.settings.skip_process_check,
            "Skip the running-game check (only if you are sure no game runs)",
        );
        let profile = app.profile().clone();
        match crate::env::parse_overrides(&app.settings.overrides) {
            Ok(pairs) => {
                let changed = crate::env::overridden_keys(&profile, &pairs);
                if !changed.is_empty() {
                    ui.label(tokens::caption(format!(
                        "Overrides profile values: {}",
                        changed.join(", ")
                    )));
                }
            }
            Err(error) => {
                ui.label(tokens::body(error.to_string()).color(theme::BAD));
            }
        }
        if let Some(lines) = ui_pages::plan_lines(app) {
            ui.collapsing("Environment of the launch", |ui| {
                for line in lines {
                    ui.monospace(line);
                }
            });
        }
    });
}
