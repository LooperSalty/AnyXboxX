//! SVG of the progress map, string for string as `emu/status_map.render`.

use std::collections::HashMap;

use super::treemap::{Rect, squarify};
use super::{Block, Section, Status};
use crate::text::escape;

const WIDTH: i64 = 960;
const MARGIN: i64 = 24;
const BG: &str = "#0d1117";
const PANEL: &str = "#161b22";
pub const GREEN: &str = "#2ea043";
pub const GREY: &str = "#484f58";
pub const ORANGE: &str = "#d29922";
const TEXT: &str = "#e6edf3";
const BADGE_H: i64 = 22;
const TITLE_H: i64 = 34;
const BLOCK_PAD: f64 = 3.0;
const MIN_CELL: i64 = 3;
const MAX_CELL: i64 = 10;
const PITCH_AREA: usize = 81;
const MIN_SECTION_H: i64 = 110;
const UNCOUNTED_CELLS: usize = 12;
const TITLE_SIZE: f64 = 11.0;

/// Green at 100 %, orange when there is progress, grey otherwise.
pub fn badge_color(share: f64) -> &'static str {
    if share >= 1.0 {
        GREEN
    } else if share > 0.0 {
        ORANGE
    } else {
        GREY
    }
}

fn badges(status: &Status, top: i64) -> Vec<String> {
    let mut parts = Vec::new();
    let mut x = MARGIN as f64;
    for badge in &status.badges {
        let left = 14.0 + 6.6 * badge.label.chars().count() as f64;
        let right = 14.0 + 6.6 * badge.value.chars().count() as f64;
        let (label, value) = (escape(&badge.label), escape(&badge.value));
        parts.push(format!(
            concat!(
                r#"<g><title>{label}: {value}</title>"#,
                r#"<rect x="{x:.1}" y="{top}" width="{width:.1}" height="{h}" rx="4" fill="{panel}"/>"#,
                r#"<rect x="{xr:.1}" y="{top}" width="{right:.1}" height="{h}" rx="4" fill="{color}"/>"#,
                r#"<text x="{xl:.1}" y="{ty}" text-anchor="middle" fill="{text}" font-size="12">{label}</text>"#,
                r##"<text x="{xv:.1}" y="{ty}" text-anchor="middle" fill="#ffffff" font-size="12" font-weight="600">{value}</text></g>"##
            ),
            label = label,
            value = value,
            x = x,
            top = top,
            width = left + right,
            h = BADGE_H,
            panel = PANEL,
            xr = x + left,
            right = right,
            color = badge_color(badge.share),
            xl = x + left / 2.0,
            ty = top + 15,
            text = TEXT,
            xv = x + left + right / 2.0,
        ));
        x += left + right + 8.0;
    }
    parts
}

/// Title shortened (…) to fit `width` pixels.
fn truncate(text: &str, width: f64) -> String {
    let fit = ((width - 8.0) / (TITLE_SIZE * 0.58)).trunc() as i64;
    if fit < 3 {
        return String::new();
    }
    let length = text.chars().count() as i64;
    if length <= fit {
        return text.to_string();
    }
    let kept: String = text.chars().take((fit - 1) as usize).collect();
    kept + "…"
}

/// Largest cell (3 to 10 px, 1 px apart) whose grid holds `count` cells.
fn grid_size(count: usize, width: f64, height: f64) -> i64 {
    (MIN_CELL..=MAX_CELL)
        .rev()
        .find(|size| {
            let pitch = size + 1;
            ((width.trunc() as i64) / pitch) * ((height.trunc() as i64) / pitch) >= count as i64
        })
        .unwrap_or(MIN_CELL)
}

/// Cells of a block: green first, then grey, below the title.
fn cells(block: &Block, rect: Rect, top: f64) -> Vec<String> {
    let inner_w = rect.w - 2.0 * BLOCK_PAD;
    let inner_h = rect.h - top - BLOCK_PAD;
    if block.total == 0 || inner_w < MIN_CELL as f64 || inner_h < MIN_CELL as f64 {
        return Vec::new();
    }
    let size = grid_size(block.total, inner_w, inner_h);
    let pitch = size + 1;
    let columns = ((inner_w.trunc() as i64) / pitch).max(1);
    (0..block.total as i64)
        .map(|index| {
            let (row, column) = (index / columns, index % columns);
            let css = if index < block.done as i64 {
                "ok"
            } else {
                "no"
            };
            format!(
                r#"<rect class="{css}" x="{:.1}" y="{:.1}" width="{size}" height="{size}"/>"#,
                rect.x + BLOCK_PAD + (column * pitch) as f64,
                rect.y + top + (row * pitch) as f64
            )
        })
        .collect()
}

fn block_svg(block: &Block, rect: Rect, counted: bool) -> Vec<String> {
    let name = escape(&block.name);
    let tip = if counted {
        format!("{name}: {}/{}", block.done, block.total)
    } else {
        name
    };
    let header = rect.w >= 40.0 && rect.h >= 30.0;
    let mut parts = vec![
        format!("<g><title>{tip}</title>"),
        format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="2" fill="{PANEL}" stroke="{BG}" stroke-width="1"/>"#,
            rect.x, rect.y, rect.w, rect.h
        ),
    ];
    let title = truncate(&block.name, rect.w);
    if header && !title.is_empty() {
        parts.push(format!(
            r#"<text x="{:.1}" y="{:.1}" font-size="11" fill="{TEXT}">{}</text>"#,
            rect.x + 4.0,
            rect.y + 12.0,
            escape(&title)
        ));
    }
    if counted {
        parts.extend(cells(block, rect, if header { 16.0 } else { BLOCK_PAD }));
    }
    parts.push("</g>".into());
    parts
}

fn section_height(section: &Section) -> i64 {
    let count = if section.counted {
        section.total()
    } else {
        section.blocks.len() * UNCOUNTED_CELLS
    };
    let wanted = ((count * PITCH_AREA) as f64 * 1.5 / (WIDTH - 2 * MARGIN) as f64).ceil() as i64;
    wanted.max(MIN_SECTION_H)
}

/// Title then treemap of the section; returns (SVG elements, height used).
fn section_svg(section: &Section, top: i64) -> (Vec<String>, i64) {
    let height = section_height(section);
    let area = Rect {
        x: MARGIN as f64,
        y: (top + TITLE_H) as f64,
        w: (WIDTH - 2 * MARGIN) as f64,
        h: height as f64,
    };
    let weights: Vec<usize> = section
        .blocks
        .iter()
        .map(|b| if section.counted { b.total } else { 1 })
        .collect();
    // `{b.name: b}`: with equal names, the last block wins.
    let by_name: HashMap<&str, &Block> = section
        .blocks
        .iter()
        .map(|b| (b.name.as_str(), b))
        .collect();
    let mut parts = vec![format!(
        r#"<text x="{MARGIN}" y="{}" font-size="15" font-weight="600" fill="{TEXT}">{}</text>"#,
        top + 20,
        escape(&section.heading())
    )];
    for (index, rect) in squarify(&weights, area) {
        let block = by_name[section.blocks[index].name.as_str()];
        parts.extend(block_svg(block, rect, section.counted));
    }
    (parts, TITLE_H + height + 22)
}

/// Standalone SVG image (960 px wide) of the progress map.
pub fn render(status: &Status) -> String {
    let mut top = MARGIN + BADGE_H + 18;
    let mut body = badges(status, MARGIN);
    for section in &status.sections {
        if section.blocks.is_empty() || (section.counted && section.total() == 0) {
            continue;
        }
        let (parts, used) = section_svg(section, top);
        body.extend(parts);
        top += used;
    }
    let height = top + MARGIN - 10;
    let mut lines = vec![
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}" font-family="Segoe UI, Arial, sans-serif" font-size="13">"#
        ),
        format!("<style>.ok{{fill:{GREEN}}}.no{{fill:{GREY}}}</style>"),
        format!(r#"<rect width="{WIDTH}" height="{height}" fill="{BG}"/>"#),
    ];
    lines.extend(body);
    lines.push("</svg>".into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_names_and_cuts_long_ones() {
        assert_eq!(truncate("XUser", 200.0), "XUser");
        assert_eq!(truncate("XTaskQueueCreate", 60.0), "XTaskQu…");
        assert_eq!(truncate("XUser", 20.0), "");
    }

    #[test]
    fn grid_size_shrinks_to_fit() {
        assert_eq!(grid_size(4, 22.0, 22.0), 10);
        assert_eq!(grid_size(100, 40.0, 40.0), 3);
        assert_eq!(grid_size(10_000, 10.0, 10.0), MIN_CELL);
    }

    #[test]
    fn badge_colors() {
        assert_eq!(badge_color(1.0), GREEN);
        assert_eq!(badge_color(0.3), ORANGE);
        assert_eq!(badge_color(0.0), GREY);
    }
}
