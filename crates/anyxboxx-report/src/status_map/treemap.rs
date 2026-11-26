//! "Squarified" treemap (Bruls, Huizing, van Wijk), operation for operation as
//! `emu/status_map.squarify`, so the rectangles land on the same floats.

use crate::text::py_sum;

/// Top-left corner (x, y), width w, height h.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Worst aspect ratio of a row of areas laid along a side of length `side`.
fn worst(row: &[f64], side: f64) -> f64 {
    let total = py_sum(row.iter().copied());
    row.iter()
        .map(|&a| {
            let p = side * side * a / (total * total);
            let q = total * total / (side * side * a);
            if q > p { q } else { p }
        })
        .fold(f64::NEG_INFINITY, |best, v| if v > best { v } else { best })
}

/// Lays a row along the short side of `rect`; returns its rectangles and the space left.
fn place_row(row: &[(usize, f64)], rect: Rect) -> (Vec<(usize, Rect)>, Rect) {
    let total = py_sum(row.iter().map(|(_, a)| *a));
    let mut placed = Vec::with_capacity(row.len());
    let mut offset = 0.0;
    if rect.w >= rect.h {
        // a column on the left
        let width = total / rect.h;
        for &(index, area) in row {
            let height = area / width;
            placed.push((
                index,
                Rect {
                    x: rect.x,
                    y: rect.y + offset,
                    w: width,
                    h: height,
                },
            ));
            offset += height;
        }
        let rest = Rect {
            x: rect.x + width,
            y: rect.y,
            w: rect.w - width,
            h: rect.h,
        };
        return (placed, rest);
    }
    // a line at the top
    let height = total / rect.w;
    for &(index, area) in row {
        let width = area / height;
        placed.push((
            index,
            Rect {
                x: rect.x + offset,
                y: rect.y,
                w: width,
                h: height,
            },
        ));
        offset += width;
    }
    (
        placed,
        Rect {
            x: rect.x,
            y: rect.y + height,
            w: rect.w,
            h: rect.h - height,
        },
    )
}

/// Treemap of `weights` in `rect`: rectangles with areas proportional to the weights, heaviest
/// first. Returns (index into `weights`, rectangle); zero weights are left out.
pub fn squarify(weights: &[usize], rect: Rect) -> Vec<(usize, Rect)> {
    let mut kept: Vec<(usize, usize)> = weights
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, w)| *w > 0)
        .collect();
    kept.sort_by_key(|&(_, w)| std::cmp::Reverse(w)); // stable, like Python's sorted
    if kept.is_empty() {
        return Vec::new();
    }
    let sum: usize = kept.iter().map(|(_, w)| w).sum();
    let scale = rect.w * rect.h / sum as f64;
    let pending: Vec<(usize, f64)> = kept.iter().map(|&(i, w)| (i, w as f64 * scale)).collect();
    let mut result = Vec::with_capacity(pending.len());
    let mut row: Vec<(usize, f64)> = Vec::new();
    let mut free = rect;
    let mut next = 0;
    while next < pending.len() {
        let side = if free.h < free.w { free.h } else { free.w };
        let areas: Vec<f64> = row.iter().map(|(_, a)| *a).collect();
        let mut candidate = areas.clone();
        candidate.push(pending[next].1);
        if row.is_empty() || worst(&candidate, side) <= worst(&areas, side) {
            row.push(pending[next]);
            next += 1;
            continue;
        }
        let (placed, rest) = place_row(&row, free);
        result.extend(placed);
        free = rest;
        row.clear();
    }
    let (placed, _) = place_row(&row, free);
    result.extend(placed);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 900.0,
        h: 300.0,
    };

    #[test]
    fn covers_the_rectangle_with_areas_proportional_to_weights() {
        let weights = [6, 6, 4, 3, 2, 2, 1];
        let placed = squarify(&weights, AREA);
        assert_eq!(placed.len(), weights.len());
        let total: f64 = placed.iter().map(|(_, r)| r.w * r.h).sum();
        assert!((total - AREA.w * AREA.h).abs() < 1e-6);
        for (index, rect) in &placed {
            let share = rect.w * rect.h / (AREA.w * AREA.h);
            assert!((share - weights[*index] as f64 / 24.0).abs() < 1e-9);
        }
    }

    #[test]
    fn edge_cases() {
        assert!(squarify(&[], AREA).is_empty());
        assert!(squarify(&[0], AREA).is_empty());
        assert_eq!(squarify(&[3], AREA), vec![(0, AREA)]);
    }

    #[test]
    fn equal_weights_keep_their_order() {
        let order: Vec<usize> = squarify(&[1, 5, 1], AREA).iter().map(|(i, _)| *i).collect();
        assert_eq!(order, [1, 0, 2]);
    }
}
