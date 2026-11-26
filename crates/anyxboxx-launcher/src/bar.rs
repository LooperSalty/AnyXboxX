//! The progress bar: a pink-to-orange fill with a light band sweeping across it, or a band
//! sliding along the track when the length is unknown.

use eframe::egui::{self, Color32, CornerRadius, Rect, Stroke, Ui, pos2, vec2};

use crate::{theme, tokens};

/// Height of the bars on the pages.
pub const BAR_HEIGHT: f32 = 26.0;

/// A progress bar filled with a pink-to-orange gradient, with its text on top. A light band
/// sweeps across the filled part; with no fraction (length unknown) a band slides along the track.
pub fn gradient_bar(ui: &mut Ui, fraction: Option<f32>, text: &str) {
    sized_bar(ui, fraction, text, BAR_HEIGHT);
}

/// The bar at a chosen height (the launch screen draws a taller one).
pub fn sized_bar(ui: &mut Ui, fraction: Option<f32>, text: &str, height: f32) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    let radius = CornerRadius::same((height / 2.0).min(255.0) as u8);
    let time = ui.input(|i| i.time);
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let painter = ui.painter().clone();
    painter.rect_filled(rect, radius, theme::CHIP_FILL);
    let fill = match fraction {
        Some(f) if f > 0.0 => {
            let width = (rect.width() * f.clamp(0.0, 1.0))
                .max(height)
                .min(rect.width());
            Some(Rect::from_min_size(rect.min, vec2(width, height)))
        }
        Some(_) => None,
        None => {
            let (a, b) = crate::anim::moving_band(time, 1.6);
            let x0 = rect.left() + rect.width() * a;
            let x1 = rect.left() + rect.width() * b;
            (x1 - x0 > 1.0)
                .then(|| Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())))
        }
    };
    if let Some(fill) = fill {
        painter.add(egui::Shape::mesh(theme::horizontal_mesh(
            fill,
            theme::HOT_PINK,
            theme::GLOW,
        )));
        if fraction.is_some() {
            let (a, b) = crate::anim::moving_band(time, 1.8);
            let band = Rect::from_min_max(
                pos2(fill.left() + fill.width() * a, fill.top()),
                pos2(fill.left() + fill.width() * b, fill.bottom()),
            );
            let clear = Color32::from_rgba_unmultiplied(255, 255, 255, 0);
            let bright = Color32::from_rgba_unmultiplied(255, 255, 255, 70);
            if band.width() > 1.0 {
                let mid = band.center().x;
                painter.add(egui::Shape::mesh(theme::horizontal_mesh(
                    Rect::from_min_max(band.min, pos2(mid, band.bottom())),
                    clear,
                    bright,
                )));
                painter.add(egui::Shape::mesh(theme::horizontal_mesh(
                    Rect::from_min_max(pos2(mid, band.top()), band.max),
                    bright,
                    clear,
                )));
            }
        }
    }
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0_f32, theme::HOT_PINK.gamma_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    if text.is_empty() {
        return;
    }
    let font = egui::FontId::proportional(tokens::CAPTION + 1.0);
    // A dark halo under the label keeps it readable over the bright end of the fill.
    for offset in [vec2(1.0, 1.0), vec2(-1.0, 1.0), vec2(0.0, -1.0)] {
        painter.text(
            rect.center() + offset,
            egui::Align2::CENTER_CENTER,
            text,
            font.clone(),
            Color32::from_rgba_unmultiplied(12, 4, 24, 200),
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        Color32::WHITE,
    );
}
