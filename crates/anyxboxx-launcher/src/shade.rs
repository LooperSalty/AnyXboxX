//! Shading a rounded rectangle with a colour that changes from left to right, as one mesh of
//! vertical strips: the hero's dark overlay must follow the picture's rounded corners, which a plain
//! rectangle mesh would overshoot.

use eframe::egui::{Color32, Rect, epaint::Mesh, pos2};

/// Vertical strips between the corners (they carry the colour changes), and inside each corner
/// (they carry its curve).
const STRIPS: usize = 64;
const CORNER_STRIPS: usize = 12;

/// How far the rounded corner cuts into the top and bottom at `x` (0 outside the corners).
pub fn corner_inset(rect: Rect, radius: f32, x: f32) -> f32 {
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    if r == 0.0 {
        return 0.0;
    }
    let dx = if x < rect.left() + r {
        rect.left() + r - x
    } else if x > rect.right() - r {
        x - (rect.right() - r)
    } else {
        return 0.0;
    };
    let dx = dx.min(r);
    r - (r * r - dx * dx).max(0.0).sqrt()
}

/// The rounded rectangle as strips, each pair of vertices coloured by `color_at(t)`, `t` running
/// from 0 at the left edge to 1 at the right edge.
pub fn rounded_horizontal(rect: Rect, radius: f32, color_at: impl Fn(f32) -> Color32) -> Mesh {
    let mut mesh = Mesh::default();
    for (i, x) in strip_edges(rect, radius).into_iter().enumerate() {
        let t = if rect.width() > 0.0 {
            (x - rect.left()) / rect.width()
        } else {
            0.0
        };
        let inset = corner_inset(rect, radius, x);
        let color = color_at(t);
        let index = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(x, rect.top() + inset), color);
        mesh.colored_vertex(pos2(x, rect.bottom() - inset), color);
        if i > 0 {
            mesh.add_triangle(index - 2, index - 1, index);
            mesh.add_triangle(index - 1, index + 1, index);
        }
    }
    mesh
}

/// The x of every strip edge, left to right: dense inside the corners, even between them.
fn strip_edges(rect: Rect, radius: f32) -> Vec<f32> {
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let (left, right) = (rect.left() + r, rect.right() - r);
    let mut xs: Vec<f32> = (0..CORNER_STRIPS)
        .map(|i| rect.left() + r * i as f32 / CORNER_STRIPS as f32)
        .collect();
    xs.extend((0..STRIPS).map(|i| left + (right - left) * i as f32 / STRIPS as f32));
    xs.extend((0..=CORNER_STRIPS).map(|i| right + r * i as f32 / CORNER_STRIPS as f32));
    xs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_edges_run_left_to_right_from_edge_to_edge() {
        let rect = Rect::from_min_max(pos2(10.0, 0.0), pos2(410.0, 100.0));
        let xs = strip_edges(rect, 24.0);
        assert!(xs.windows(2).all(|w| w[0] < w[1]));
        assert!((xs[0] - 10.0).abs() < 1e-4);
        assert!((xs[xs.len() - 1] - 410.0).abs() < 1e-3);
    }

    #[test]
    fn the_corner_cuts_deepest_at_the_edge_and_not_at_all_inside() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0));
        assert!((corner_inset(rect, 20.0, 0.0) - 20.0).abs() < 1e-4);
        assert_eq!(corner_inset(rect, 20.0, 100.0), 0.0);
        assert_eq!(corner_inset(rect, 20.0, 20.0), 0.0);
        assert!((corner_inset(rect, 20.0, 200.0) - 20.0).abs() < 1e-4);
        assert_eq!(corner_inset(rect, 0.0, 0.0), 0.0);
    }

    #[test]
    fn the_mesh_spans_the_rect_with_one_colour_per_strip_edge() {
        let rect = Rect::from_min_max(pos2(10.0, 10.0), pos2(210.0, 110.0));
        let mesh = rounded_horizontal(rect, 16.0, |t| {
            Color32::from_black_alpha((255.0 * (1.0 - t)) as u8)
        });
        let edges = STRIPS + 2 * CORNER_STRIPS + 1;
        assert_eq!(mesh.vertices.len(), edges * 2);
        assert_eq!(mesh.indices.len(), (edges - 1) * 6);
        let bounds = mesh.calc_bounds();
        assert!((bounds.left() - rect.left()).abs() < 1e-4);
        assert!((bounds.right() - rect.right()).abs() < 1e-4);
        assert_eq!(mesh.vertices[0].color.a(), 255);
        assert_eq!(mesh.vertices.last().map(|v| v.color.a()), Some(0));
    }
}
