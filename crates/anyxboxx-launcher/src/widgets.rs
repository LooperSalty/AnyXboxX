//! The launcher's controls, painted by hand: the glowing primary button, the quiet secondary
//! button, pills, the switch and the status chips. Each answers the pointer (hover glow, press
//! scale-down) and shows a focus ring when reached with the keyboard.

use eframe::egui::{
    self, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::theme::{self, lerp_color};
use crate::tokens;

/// Scale of a pressed control.
const PRESSED_SCALE: f32 = 0.96;
/// Gap between a control and its focus ring.
const FOCUS_GAP: f32 = 3.0;

/// The keyboard focus ring around `rect`, when `response` has the focus.
pub fn focus_ring(ui: &Ui, response: &Response, rect: Rect, radius: CornerRadius) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(FOCUS_GAP),
            radius,
            Stroke::new(2.0_f32, theme::FOCUS),
            StrokeKind::Outside,
        );
    }
}

/// Hover (0..1) and press (0..1) of a control, eased.
fn hover_press(ui: &Ui, response: &Response, enabled: bool) -> (f32, f32) {
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(
        response.id,
        enabled && (response.hovered() || response.has_focus()),
        tokens::HOVER_TIME,
    );
    let press = ctx.animate_bool_with_time(
        response.id.with("press"),
        enabled && response.is_pointer_button_down_on(),
        0.08,
    );
    (hover, press)
}

fn pill_radius(rect: Rect) -> CornerRadius {
    CornerRadius::same((rect.height() / 2.0).min(255.0) as u8)
}

/// The main button: a pink pill that glows under the pointer, sinks a little when pressed and
/// pulses while `pulsing` (a launch is under way). A disabled one never reports a click.
pub fn glow_button(
    ui: &mut Ui,
    enabled: bool,
    label: &str,
    min_size: egui::Vec2,
    pulsing: bool,
) -> Response {
    let big = min_size.y > 48.0;
    let font = egui::FontId::proportional(if big { 24.0 } else { tokens::BODY });
    let text = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, Color32::WHITE);
    let size = vec2(
        min_size.x.max(text.size().x + tokens::XL + tokens::S),
        min_size.y.max(text.size().y + tokens::M),
    );
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let (hover, press) = hover_press(ui, &response, enabled);
    let pulse = if pulsing && enabled {
        ui.ctx().request_repaint();
        crate::anim::pulse(ui.input(|i| i.time), 1.4)
    } else {
        0.0
    };
    let scale = (1.0 + 0.03 * hover) * (1.0 - (1.0 - PRESSED_SCALE) * press);
    let body = Rect::from_center_size(rect.center(), size * scale);
    let radius = pill_radius(body);
    let painter = ui.painter();
    if enabled {
        let strength = 0.2 + 0.5 * hover + 0.6 * pulse - 0.15 * press;
        for layer in 1..=5_u8 {
            let alpha = (strength.max(0.0) * 70.0 / f32::from(layer)).min(255.0) as u8;
            painter.rect_filled(
                body.expand(f32::from(layer) * 3.5),
                CornerRadius::same(radius.nw.saturating_add(layer * 3)),
                Color32::from_rgba_unmultiplied(0xff, 0x2d, 0x95, alpha),
            );
        }
    }
    let fill = if enabled {
        lerp_color(theme::BUTTON_PINK, theme::BUTTON_PINK_HOVER, hover)
    } else {
        theme::BUTTON_OFF
    };
    painter.rect_filled(body, radius, fill);
    painter.rect_stroke(
        body,
        radius,
        Stroke::new(1.0_f32, Color32::WHITE.gamma_multiply(0.25 + 0.4 * hover)),
        StrokeKind::Inside,
    );
    let text_color = if enabled {
        Color32::WHITE
    } else {
        theme::TEXT_MUTED
    };
    painter.galley(body.center() - text.size() / 2.0, text, text_color);
    focus_ring(ui, &response, body, radius);
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A quiet button for secondary actions (Cancel, Copy log): an outlined pill that fills on hover.
pub fn secondary_button(ui: &mut Ui, enabled: bool, label: &str) -> Response {
    let font = egui::FontId::proportional(tokens::BODY);
    let color = if enabled {
        theme::TEXT
    } else {
        theme::TEXT_MUTED
    };
    let text = ui.painter().layout_no_wrap(label.to_string(), font, color);
    let size = vec2(text.size().x + tokens::L + tokens::S, 36.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let (hover, press) = hover_press(ui, &response, enabled);
    let body = Rect::from_center_size(rect.center(), size * (1.0 - (1.0 - PRESSED_SCALE) * press));
    let radius = pill_radius(body);
    let painter = ui.painter();
    painter.rect_filled(
        body,
        radius,
        lerp_color(theme::SURFACE, theme::SURFACE_HOVER, hover),
    );
    painter.rect_stroke(
        body,
        radius,
        Stroke::new(1.0_f32, theme::HOT_PINK.gamma_multiply(0.45 + 0.4 * hover)),
        StrokeKind::Inside,
    );
    painter.galley(body.center() - text.size() / 2.0, text, color);
    focus_ring(ui, &response, body, radius);
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A choice or tab as a pill; returns true when clicked.
pub fn pill(ui: &mut Ui, selected: bool, label: &str) -> bool {
    let font = egui::FontId::proportional(tokens::BODY);
    let text = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, Color32::WHITE);
    let size = vec2(text.size().x + tokens::L + tokens::S, 34.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let (hover, press) = hover_press(ui, &response, true);
    let body = Rect::from_center_size(rect.center(), size * (1.0 - (1.0 - PRESSED_SCALE) * press));
    let radius = pill_radius(body);
    let fill = if selected {
        theme::BUTTON_PINK
    } else {
        lerp_color(theme::SURFACE, theme::SURFACE_HOVER, hover)
    };
    let painter = ui.painter();
    painter.rect_filled(body, radius, fill);
    painter.rect_stroke(
        body,
        radius,
        Stroke::new(1.0_f32, theme::HOT_PINK.gamma_multiply(0.5 + 0.4 * hover)),
        StrokeKind::Inside,
    );
    painter.galley(body.center() - text.size() / 2.0, text, Color32::WHITE);
    focus_ring(ui, &response, body, radius);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// What a status chip says about its subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Good,
    Warn,
    Bad,
}

impl Status {
    pub fn color(self) -> Color32 {
        match self {
            Status::Good => theme::GOOD,
            Status::Warn => theme::WARN,
            Status::Bad => theme::BAD,
        }
    }
}

/// The status icon: a check, an exclamation mark or a cross on a coloured disc.
pub fn status_icon(painter: &egui::Painter, center: egui::Pos2, r: f32, status: Status) {
    painter.circle_filled(center, r, status.color());
    let ink = Stroke::new((r / 4.0).max(1.5), theme::NIGHT);
    let k = r * 0.45;
    match status {
        Status::Good => {
            let a = pos2(center.x - k, center.y);
            let b = pos2(center.x - k * 0.25, center.y + k * 0.7);
            let c = pos2(center.x + k, center.y - k * 0.6);
            painter.line_segment([a, b], ink);
            painter.line_segment([b, c], ink);
        }
        Status::Warn => {
            painter.line_segment(
                [
                    pos2(center.x, center.y - k),
                    pos2(center.x, center.y + k * 0.25),
                ],
                ink,
            );
            painter.circle_filled(
                pos2(center.x, center.y + k * 0.8),
                ink.width * 0.6,
                ink.color,
            );
        }
        Status::Bad => {
            let d = k * 0.8;
            painter.line_segment(
                [
                    pos2(center.x - d, center.y - d),
                    pos2(center.x + d, center.y + d),
                ],
                ink,
            );
            painter.line_segment(
                [
                    pos2(center.x - d, center.y + d),
                    pos2(center.x + d, center.y - d),
                ],
                ink,
            );
        }
    }
}

/// A status chip: an icon and a short text on a dark pill, with a tooltip that says what to do.
pub fn chip(ui: &mut Ui, status: Status, text: &str, tooltip: &str) -> Response {
    let font = egui::FontId::proportional(14.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, theme::TEXT);
    let size = vec2(galley.size().x + 44.0, 32.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id, response.hovered(), tokens::HOVER_TIME);
    let body = rect.translate(vec2(0.0, -2.0 * hover));
    let radius = pill_radius(body);
    let painter = ui.painter();
    painter.rect_filled(body, radius, theme::CHIP_FILL);
    painter.rect_stroke(
        body,
        radius,
        Stroke::new(1.0_f32, status.color().gamma_multiply(0.55 + 0.4 * hover)),
        StrokeKind::Inside,
    );
    status_icon(
        painter,
        pos2(body.left() + 17.0, body.center().y),
        8.0,
        status,
    );
    painter.galley(
        pos2(body.left() + 32.0, body.center().y - galley.size().y / 2.0),
        galley,
        theme::TEXT,
    );
    response.on_hover_text(tooltip)
}

/// An on/off switch that slides; returns true when it was flipped this frame.
pub fn toggle(ui: &mut Ui, value: &mut bool) -> bool {
    let size = vec2(46.0, 26.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    let flipped = response.clicked();
    if flipped {
        *value = !*value;
        response.mark_changed();
    }
    let on = ui
        .ctx()
        .animate_bool_with_time(response.id, *value, tokens::HOVER_TIME);
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    let track = lerp_color(
        lerp_color(theme::SURFACE, theme::SURFACE_HOVER, hover),
        theme::BUTTON_PINK,
        on,
    );
    let radius = CornerRadius::same(13);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, track);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0_f32, theme::HOT_PINK.gamma_multiply(0.6)),
        StrokeKind::Inside,
    );
    let x = rect.left() + 13.0 + (rect.width() - 26.0) * on;
    painter.circle_filled(pos2(x, rect.center().y), 9.5 + hover, Color32::WHITE);
    focus_ring(ui, &response, rect, radius);
    response.on_hover_cursor(egui::CursorIcon::PointingHand);
    flipped
}
