//! The launcher's look: a dusk gradient (night purple to magenta to a warm orange glow), dark
//! translucent cards with a pink edge, pill tabs and a pink-to-orange progress bar. Colours only:
//! no logo, wordmark or font of any game. The colour maths is plain and tested; the painting
//! helpers only call it.

use eframe::egui::{
    self, Color32, CornerRadius, Rect, Shadow, Stroke, Ui, epaint::Mesh, pos2, vec2,
};

use crate::tokens;

pub const NIGHT: Color32 = Color32::from_rgb(0x1a, 0x0b, 0x2e);
pub const VIOLET: Color32 = Color32::from_rgb(0x4b, 0x1d, 0x6e);
pub const MAGENTA: Color32 = Color32::from_rgb(0xc2, 0x18, 0x5b);
pub const GLOW: Color32 = Color32::from_rgb(0xff, 0x8a, 0x3d);
pub const HOT_PINK: Color32 = Color32::from_rgb(0xff, 0x2d, 0x95);
pub const TEXT: Color32 = Color32::from_rgb(0xf7, 0xf0, 0xff);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0xcf, 0xc0, 0xe6);
pub const GOOD: Color32 = Color32::from_rgb(0x5d, 0xe0, 0x8a);
pub const BAD: Color32 = Color32::from_rgb(0xff, 0x7a, 0x8a);
pub const WARN: Color32 = Color32::from_rgb(0xff, 0xc8, 0x6b);
/// The keyboard focus ring: a warm light that reads over every surface of the window.
pub const FOCUS: Color32 = Color32::from_rgb(0xff, 0xe0, 0x9e);
/// Controls at rest and under the pointer (pills, switches, secondary buttons).
pub const SURFACE: Color32 = Color32::from_rgb(0x2a, 0x10, 0x46);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x5a, 0x1c, 0x6a);
/// The status chips' fill.
pub const CHIP_FILL: Color32 = Color32::from_rgb(0x12, 0x07, 0x20);
/// The fill of buttons that carry white text (5:1 or better), its hovered form, and a disabled one.
pub const BUTTON_PINK: Color32 = Color32::from_rgb(0xd4, 0x14, 0x7a);
pub const BUTTON_PINK_HOVER: Color32 = Color32::from_rgb(0xd9, 0x18, 0x7e);
pub const BUTTON_OFF: Color32 = Color32::from_rgb(0x3a, 0x22, 0x52);
/// The error card: a deep red surface with a brighter edge.
pub const ERROR_FILL: Color32 = Color32::from_rgb(0x3a, 0x0a, 0x1c);
pub const SUCCESS_FILL: Color32 = Color32::from_rgb(0x0d, 0x2a, 0x22);

/// Darkness of the overlay under the text on a picture (left edge of the hero, bottom of the launch
/// screen), out of 255: dark enough for AA text even on a white picture. The renderer blends in
/// linear light, where a dark veil hides less than its alpha suggests; the test models that.
pub const OVERLAY_RGB: [u8; 3] = [0x0c, 0x04, 0x18];
pub const OVERLAY_ALPHA: u8 = 236;

/// The card colour (dark, translucent) and its opacity over the gradient.
pub const CARD_RGB: [u8; 3] = [0x14, 0x08, 0x24];
pub const CARD_ALPHA: u8 = 215;

/// Gradient stops from the top of the window to the bottom.
const STOPS: [(f32, Color32); 4] = [(0.0, NIGHT), (0.42, VIOLET), (0.80, MAGENTA), (1.0, GLOW)];

#[cfg(test)]
fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8
}

/// The gradient colour at `t` (0 = top, 1 = bottom); `t` is clamped.
#[cfg(test)]
pub fn gradient_at(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    for pair in STOPS.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t <= t1 {
            let local = (t - t0) / (t1 - t0);
            return Color32::from_rgb(
                lerp_channel(c0.r(), c1.r(), local),
                lerp_channel(c0.g(), c1.g(), local),
                lerp_channel(c0.b(), c1.b(), local),
            );
        }
    }
    GLOW
}

#[cfg(test)]
fn linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.039_28 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
fn luminance(color: Color32) -> f64 {
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

/// WCAG contrast ratio between two opaque colours.
#[cfg(test)]
pub fn contrast_ratio(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// What a card looks like when it sits over `behind`.
#[cfg(test)]
pub fn card_over(behind: Color32) -> Color32 {
    let a = f32::from(CARD_ALPHA) / 255.0;
    Color32::from_rgb(
        lerp_channel(behind.r(), CARD_RGB[0], a),
        lerp_channel(behind.g(), CARD_RGB[1], a),
        lerp_channel(behind.b(), CARD_RGB[2], a),
    )
}

fn card_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(CARD_RGB[0], CARD_RGB[1], CARD_RGB[2], CARD_ALPHA)
}

/// Installs the dark visuals and the spacing of the launcher.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.window_fill = Color32::from_rgb(0x22, 0x0e, 0x3c);
    visuals.window_stroke = Stroke::new(1.0_f32, HOT_PINK.gamma_multiply(0.6));
    visuals.window_corner_radius = CornerRadius::same(16);
    visuals.extreme_bg_color = Color32::from_rgb(0x10, 0x06, 0x1c);
    visuals.hyperlink_color = Color32::from_rgb(0xff, 0xb3, 0xdd);
    visuals.selection.bg_fill = HOT_PINK.gamma_multiply(0.55);
    visuals.selection.stroke = Stroke::new(2.0_f32, FOCUS);
    let widgets = &mut visuals.widgets;
    widgets.inactive.bg_fill = Color32::from_rgb(0x3a, 0x17, 0x5c);
    widgets.inactive.weak_bg_fill = Color32::from_rgb(0x3a, 0x17, 0x5c);
    widgets.inactive.corner_radius = CornerRadius::same(10);
    widgets.hovered.bg_fill = Color32::from_rgb(0x8a, 0x1f, 0x7a);
    widgets.hovered.weak_bg_fill = Color32::from_rgb(0x8a, 0x1f, 0x7a);
    widgets.hovered.corner_radius = CornerRadius::same(10);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, HOT_PINK);
    widgets.active.bg_fill = BUTTON_PINK;
    widgets.active.weak_bg_fill = BUTTON_PINK;
    widgets.active.corner_radius = CornerRadius::same(10);
    widgets.open.corner_radius = CornerRadius::same(10);
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = vec2(tokens::S, tokens::S);
        style.spacing.button_padding = vec2(tokens::M, tokens::S);
        style.spacing.interact_size.y = 32.0;
        style.animation_time = tokens::HOVER_TIME;
        for (text_style, size) in [
            (egui::TextStyle::Heading, tokens::TITLE),
            (egui::TextStyle::Body, tokens::BODY),
            (egui::TextStyle::Button, tokens::BODY),
            (egui::TextStyle::Small, tokens::CAPTION),
            (egui::TextStyle::Monospace, tokens::CAPTION),
        ] {
            if let Some(font) = style.text_styles.get_mut(&text_style) {
                font.size = size;
            }
        }
    });
}

/// The vertical gradient as one mesh with a pair of vertices per stop.
pub fn gradient_mesh(rect: Rect) -> Mesh {
    let mut mesh = Mesh::default();
    let mut previous = None;
    for (t, color) in STOPS {
        let y = rect.top() + rect.height() * t;
        let left = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(rect.left(), y), color);
        mesh.colored_vertex(pos2(rect.right(), y), color);
        if let Some(prev) = previous {
            mesh.add_triangle(prev, prev + 1, left);
            mesh.add_triangle(prev + 1, left + 1, left);
        }
        previous = Some(left);
    }
    mesh
}

/// Paints the gradient behind everything.
pub fn paint_background(ctx: &egui::Context) {
    let rect = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::background())
        .add(egui::Shape::mesh(gradient_mesh(rect)));
}

/// A translucent dark card with a pink edge glow.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(card_fill())
        .stroke(Stroke::new(1.0_f32, HOT_PINK.gamma_multiply(0.45)))
        .corner_radius(tokens::radius(tokens::R_CARD))
        .inner_margin(tokens::card_margin())
        .shadow(Shadow {
            offset: [0, 0],
            blur: 22,
            spread: 1,
            color: HOT_PINK.gamma_multiply(0.22),
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The colour `t` of the way from `a` to `b` (`t` clamped).
pub(crate) fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| {
        (f32::from(x) + (f32::from(y) - f32::from(x)) * t.clamp(0.0, 1.0)).round() as u8
    };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// A quad shaded from `left` to `right`.
pub(crate) fn horizontal_mesh(rect: Rect, left: Color32, right: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_hits_its_stops_and_clamps() {
        assert_eq!(gradient_at(0.0), NIGHT);
        assert_eq!(gradient_at(0.42), VIOLET);
        assert_eq!(gradient_at(1.0), GLOW);
        assert_eq!(gradient_at(-3.0), NIGHT);
        assert_eq!(gradient_at(9.0), GLOW);
    }

    #[test]
    fn gradient_goes_from_dark_to_bright() {
        assert!(luminance(gradient_at(0.0)) < luminance(gradient_at(0.5)));
        assert!(luminance(gradient_at(0.5)) < luminance(gradient_at(1.0)));
    }

    #[test]
    fn text_on_cards_meets_wcag_aa_over_the_whole_gradient() {
        for i in 0..=20 {
            let card = card_over(gradient_at(i as f32 / 20.0));
            assert!(contrast_ratio(TEXT, card) >= 4.5, "body text at {i}");
            assert!(contrast_ratio(TEXT_MUTED, card) >= 4.5, "muted text at {i}");
            for status in [GOOD, BAD, WARN] {
                assert!(contrast_ratio(status, card) >= 4.5, "status colour at {i}");
            }
        }
    }

    #[test]
    fn white_text_on_the_button_pinks_meets_wcag_aa() {
        assert!(contrast_ratio(Color32::WHITE, BUTTON_PINK) >= 4.5);
        assert!(contrast_ratio(Color32::WHITE, BUTTON_PINK_HOVER) >= 4.5);
    }

    /// Contrast of `text` over the overlay on top of `behind`, blended in linear light (the
    /// renderer's way, and the worst case for a dark veil over a light picture).
    fn contrast_over_overlay(text: Color32, behind: Color32) -> f64 {
        let a = f64::from(OVERLAY_ALPHA) / 255.0;
        let [r, g, b] = OVERLAY_RGB;
        let veil = luminance(Color32::from_rgb(r, g, b));
        let ground = a * veil + (1.0 - a) * luminance(behind);
        let lt = luminance(text);
        let (hi, lo) = if lt >= ground {
            (lt, ground)
        } else {
            (ground, lt)
        };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn text_over_a_picture_meets_wcag_aa_whatever_the_picture() {
        for behind in [Color32::WHITE, Color32::BLACK, GLOW, HOT_PINK] {
            assert!(
                contrast_over_overlay(TEXT, behind) >= 4.5,
                "text over {behind:?}"
            );
            assert!(
                contrast_over_overlay(TEXT_MUTED, behind) >= 4.5,
                "muted over {behind:?}"
            );
        }
    }

    #[test]
    fn controls_and_cards_keep_their_text_readable() {
        for ground in [SURFACE, SURFACE_HOVER, CHIP_FILL, ERROR_FILL, SUCCESS_FILL] {
            assert!(contrast_ratio(TEXT, ground) >= 4.5, "text on {ground:?}");
        }
        assert!(
            contrast_ratio(TEXT_MUTED, BUTTON_OFF) >= 4.5,
            "disabled label"
        );
        assert!(contrast_ratio(BAD, ERROR_FILL) >= 4.5, "error title");
        assert!(contrast_ratio(GOOD, SUCCESS_FILL) >= 4.5, "success title");
        for status in [GOOD, WARN, BAD] {
            assert!(
                contrast_ratio(status, CHIP_FILL) >= 3.0,
                "chip icon (non-text, 3:1)"
            );
        }
    }

    #[test]
    fn the_focus_ring_stands_out_from_every_surface() {
        for ground in [NIGHT, VIOLET, MAGENTA, SURFACE, BUTTON_PINK] {
            assert!(contrast_ratio(FOCUS, ground) >= 3.0, "focus on {ground:?}");
        }
    }

    #[test]
    fn lerp_color_runs_between_its_ends() {
        assert_eq!(lerp_color(NIGHT, GLOW, 0.0), NIGHT);
        assert_eq!(lerp_color(NIGHT, GLOW, 1.0), GLOW);
        assert_eq!(lerp_color(NIGHT, GLOW, 9.0), GLOW);
    }

    #[test]
    fn contrast_of_black_on_white_is_21() {
        let ratio = contrast_ratio(Color32::BLACK, Color32::WHITE);
        assert!((ratio - 21.0).abs() < 0.01);
    }

    #[test]
    fn gradient_mesh_covers_the_rect() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 200.0));
        let mesh = gradient_mesh(rect);
        assert_eq!(mesh.vertices.len(), STOPS.len() * 2);
        assert_eq!(mesh.indices.len(), (STOPS.len() - 1) * 6);
        assert_eq!(mesh.calc_bounds(), rect);
    }
}
