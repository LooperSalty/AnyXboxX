//! The design tokens of the window: one spacing scale on an 8-point grid, one type scale, one set
//! of corner radii and the motion timings. Every page takes its gaps, sizes and durations from
//! here, so the rhythm stays the same from page to page.

use eframe::egui::{CornerRadius, Margin, RichText};

use crate::theme;

/// Spacing scale (8-point grid; `XS` is the half step for tight pairs such as a title and its
/// caption).
pub const XS: f32 = 4.0;
pub const S: f32 = 8.0;
pub const M: f32 = 16.0;
pub const L: f32 = 24.0;
pub const XL: f32 = 32.0;
pub const XXL: f32 = 48.0;

/// Type scale, in points: display (the hero), title (a page), subtitle (a card), body, caption.
pub const DISPLAY: f32 = 48.0;
pub const TITLE: f32 = 26.0;
pub const SUBTITLE: f32 = 18.0;
pub const BODY: f32 = 15.0;
pub const CAPTION: f32 = 13.0;

/// Corner radii: controls, cards, hero surfaces. A card's radius is its control radius plus its
/// padding would be too round, so cards and controls are separate surfaces with their own radius.
pub const R_CONTROL: u8 = 10;
pub const R_CARD: u8 = 16;
pub const R_HERO: u8 = 24;

/// Inner padding of every card.
pub const CARD_PAD: i8 = 24;

/// Motion: hover and press feedback, page change, toast entrance (seconds).
pub const HOVER_TIME: f32 = 0.15;
pub const PAGE_TIME: f64 = 0.22;
pub const TOAST_IN: f64 = 0.24;

pub fn radius(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

pub fn card_margin() -> Margin {
    Margin::same(CARD_PAD)
}

/// The hero line: the largest size, heavy.
pub fn display(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .size(DISPLAY)
        .strong()
        .color(theme::TEXT)
}

/// A page title.
pub fn title(text: impl Into<String>) -> RichText {
    RichText::new(text).size(TITLE).strong().color(theme::TEXT)
}

/// A card title.
pub fn subtitle(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .size(SUBTITLE)
        .strong()
        .color(theme::TEXT)
}

/// Running text.
pub fn body(text: impl Into<String>) -> RichText {
    RichText::new(text).size(BODY).color(theme::TEXT)
}

/// Secondary text under a title or beside a control.
pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text).size(BODY).color(theme::TEXT_MUTED)
}

/// Small print: paths, versions, hints.
pub fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text).size(CAPTION).color(theme::TEXT_MUTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_sits_on_the_eight_point_grid() {
        for step in [S, M, L, XL, XXL] {
            assert_eq!(step % 8.0, 0.0, "{step}");
        }
        assert_eq!(XS * 2.0, S);
        assert_eq!(f32::from(CARD_PAD) % 8.0, 0.0);
    }

    #[test]
    fn the_type_scale_steps_down() {
        let scale = [DISPLAY, TITLE, SUBTITLE, BODY, CAPTION];
        assert!(scale.windows(2).all(|w| w[0] > w[1]));
        assert!(
            scale.iter().all(|&size| size >= 12.0),
            "small print stays readable"
        );
    }

    #[test]
    fn radii_grow_with_the_surface() {
        let radii = [R_CONTROL, R_CARD, R_HERO];
        assert!(radii.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn motion_stays_in_the_150_to_250_ms_band() {
        assert!((0.15..=0.25).contains(&f64::from(HOVER_TIME)));
        assert!((0.15..=0.25).contains(&PAGE_TIME));
        assert!((0.15..=0.25).contains(&TOAST_IN));
    }
}
