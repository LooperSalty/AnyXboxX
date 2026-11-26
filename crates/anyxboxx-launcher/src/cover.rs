//! Placing a picture like CSS `object-fit: cover`: the part of the image (as texture coordinates)
//! that fills a frame without stretching, zoomed and shifted for the slow Ken Burns drift and the
//! pointer parallax. Plain maths, tested; the painters only feed it sizes and the clock.

use eframe::egui::{Rect, Vec2, pos2, vec2};

/// Extra zoom at the top of the drift (8 %).
pub const KEN_BURNS_DEPTH: f32 = 0.08;

/// The texture rectangle (0..1 on both axes) that covers a frame of size `frame` with an image of
/// size `image`, zoomed by `zoom` (1 = just covering) and shifted by `pan` (-1..1 on each axis, the
/// share of the spare room to move). The rectangle never leaves the image.
pub fn cover_uv(image: Vec2, frame: Vec2, zoom: f32, pan: Vec2) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 || frame.x <= 0.0 || frame.y <= 0.0 {
        return Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    }
    let image_aspect = image.x / image.y;
    let frame_aspect = frame.x / frame.y;
    // Share of the image's width and height the frame shows at zoom 1.
    let base = if frame_aspect > image_aspect {
        vec2(1.0, image_aspect / frame_aspect)
    } else {
        vec2(frame_aspect / image_aspect, 1.0)
    };
    let size = base / zoom.max(1.0);
    let spare = vec2(1.0 - size.x, 1.0 - size.y);
    let pan = vec2(pan.x.clamp(-1.0, 1.0), pan.y.clamp(-1.0, 1.0));
    let center = vec2(0.5 + pan.x * spare.x / 2.0, 0.5 + pan.y * spare.y / 2.0);
    Rect::from_center_size(pos2(center.x, center.y), size)
}

/// Zoom and pan of the slideshow picture number `index` at `progress` (0..1 of its life): even
/// pictures zoom in, odd ones zoom out, and the drift direction turns with the index. `depth`
/// scales the zoom range (1 = full). The 0.04 base leaves room for the pointer parallax.
pub fn slide_view(index: usize, progress: f32, depth: f32) -> (f32, Vec2) {
    let p = progress.clamp(0.0, 1.0);
    let t = if index.is_multiple_of(2) { p } else { 1.0 - p };
    let zoom = 1.04 + KEN_BURNS_DEPTH * depth * t;
    let dir = [
        vec2(1.0, 0.0),
        vec2(-1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(0.0, -1.0),
    ][index % 4];
    (zoom, dir * (p - 0.5) * 0.6)
}

/// The parallax shift for a pointer at `pointer` over `frame`: the picture leans away from the
/// pointer, at most `strength` (in the -1..1 pan units) on each axis. No pointer, no lean.
pub fn parallax(pointer: Option<eframe::egui::Pos2>, frame: Rect, strength: f32) -> Vec2 {
    let Some(p) = pointer else {
        return Vec2::ZERO;
    };
    if frame.width() <= 0.0 || frame.height() <= 0.0 {
        return Vec2::ZERO;
    }
    let rel = vec2(
        ((p.x - frame.center().x) / (frame.width() / 2.0)).clamp(-1.0, 1.0),
        ((p.y - frame.center().y) / (frame.height() / 2.0)).clamp(-1.0, 1.0),
    );
    -rel * strength
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn slides_alternate_zoom_direction_and_pan() {
        let (z0, _) = slide_view(0, 0.0, 1.0);
        let (z1, _) = slide_view(0, 1.0, 1.0);
        assert!(z1 > z0);
        let (o0, _) = slide_view(1, 0.0, 1.0);
        let (o1, _) = slide_view(1, 1.0, 1.0);
        assert!(o1 < o0);
        let (_, pan_a) = slide_view(0, 1.0, 1.0);
        let (_, pan_b) = slide_view(1, 1.0, 1.0);
        assert!(pan_a.x > 0.0 && pan_b.x < 0.0);
    }

    #[test]
    fn a_wide_frame_crops_the_top_and_bottom() {
        let uv = cover_uv(vec2(1000.0, 1000.0), vec2(200.0, 100.0), 1.0, Vec2::ZERO);
        assert!(close(uv.width(), 1.0));
        assert!(close(uv.height(), 0.5));
        assert!(close(uv.center().y, 0.5));
    }

    #[test]
    fn a_tall_frame_crops_the_sides() {
        let uv = cover_uv(vec2(1600.0, 900.0), vec2(900.0, 900.0), 1.0, Vec2::ZERO);
        assert!(close(uv.height(), 1.0));
        assert!(close(uv.width(), 900.0 / 1600.0));
    }

    #[test]
    fn zoom_shrinks_the_window_and_pan_stays_inside() {
        let uv = cover_uv(vec2(100.0, 100.0), vec2(100.0, 100.0), 2.0, vec2(1.0, -1.0));
        assert!(close(uv.width(), 0.5));
        assert!(close(uv.right(), 1.0));
        assert!(close(uv.top(), 0.0));
        let wild = cover_uv(vec2(100.0, 100.0), vec2(100.0, 100.0), 1.5, vec2(9.0, 9.0));
        assert!(wild.min.x >= 0.0 && wild.max.x <= 1.0 + 1e-6);
        assert!(wild.min.y >= 0.0 && wild.max.y <= 1.0 + 1e-6);
    }

    #[test]
    fn degenerate_sizes_show_the_whole_image() {
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        assert_eq!(
            cover_uv(Vec2::ZERO, vec2(10.0, 10.0), 1.0, Vec2::ZERO),
            full
        );
        assert_eq!(
            cover_uv(vec2(10.0, 10.0), Vec2::ZERO, 1.0, Vec2::ZERO),
            full
        );
    }

    #[test]
    fn zoom_below_one_never_uncovers_the_frame() {
        let uv = cover_uv(vec2(100.0, 100.0), vec2(100.0, 100.0), 0.3, Vec2::ZERO);
        assert!(close(uv.width(), 1.0));
    }

    #[test]
    fn parallax_leans_away_from_the_pointer_within_its_strength() {
        let frame = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0));
        assert_eq!(parallax(None, frame, 0.3), Vec2::ZERO);
        let right = parallax(Some(pos2(200.0, 50.0)), frame, 0.3);
        assert!(close(right.x, -0.3) && close(right.y, 0.0));
        let far = parallax(Some(pos2(-900.0, 900.0)), frame, 0.3);
        assert!(close(far.x, 0.3) && close(far.y, -0.3));
    }
}
