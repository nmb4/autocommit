use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::scroll_state::ScrollState;

/// Render-ready representation of one row in a selection popup.
#[derive(Default, Clone)]
pub struct GenericDisplayRow {
    pub name: String,
    pub description: Option<String>,
    pub wrap_indent: Option<usize>,
    pub is_disabled: bool,
    pub disabled_reason: Option<String>,
}

const MENU_SURFACE_INSET_V: u16 = 1;
const MENU_SURFACE_INSET_H: u16 = 2;

pub(crate) fn menu_surface_inset(area: Rect) -> Rect {
    Rect {
        x: area.x + MENU_SURFACE_INSET_H,
        y: area.y + MENU_SURFACE_INSET_V,
        width: area.width.saturating_sub(MENU_SURFACE_INSET_H * 2),
        height: area.height.saturating_sub(MENU_SURFACE_INSET_V * 2),
    }
}

pub(crate) fn menu_surface_padding_height() -> u16 {
    MENU_SURFACE_INSET_V * 2
}

pub fn render_menu_surface(area: Rect, buf: &mut Buffer) -> Rect {
    if area.is_empty() {
        return area;
    }
    Block::default()
        .style(Style::default().bg(Color::DarkGray))
        .render(area, buf);
    menu_surface_inset(area)
}

fn build_full_line(row: &GenericDisplayRow, desc_col: usize) -> Line<'static> {
    let combined_description = match (&row.description, &row.disabled_reason) {
        (Some(desc), Some(reason)) => Some(format!("{desc} (disabled: {reason})")),
        (Some(desc), None) => Some(desc.clone()),
        (None, Some(reason)) => Some(format!("disabled: {reason}")),
        (None, None) => None,
    };

    let name_limit = combined_description
        .as_ref()
        .map(|_| desc_col.saturating_sub(2))
        .unwrap_or(usize::MAX);

    let mut name_spans: Vec<Span> = Vec::new();
    let mut used_width = 0usize;
    let mut truncated = false;

    for ch in row.name.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        let next_width = used_width.saturating_add(ch_w);
        if next_width > name_limit {
            truncated = true;
            break;
        }
        used_width = next_width;
        name_spans.push(ch.to_string().into());
    }

    if truncated {
        name_spans.push("…".into());
    }

    if row.disabled_reason.is_some() {
        name_spans.push(" (disabled)".dim());
    }

    let this_name_width = Line::from(name_spans.clone()).width();
    let mut full_spans: Vec<Span> = name_spans;
    if let Some(desc) = combined_description.as_ref() {
        let gap = desc_col.saturating_sub(this_name_width);
        if gap > 0 {
            full_spans.push(" ".repeat(gap).into());
        }
        full_spans.push(desc.clone().dim());
    }
    Line::from(full_spans)
}

fn apply_row_state_style(lines: &mut [Line<'static>], selected: bool, is_disabled: bool) {
    if selected {
        for line in lines.iter_mut() {
            line.spans.iter_mut().for_each(|span| {
                span.style = Style::default().fg(Color::Cyan).bold();
            });
        }
    }
    if is_disabled {
        for line in lines.iter_mut() {
            line.spans.iter_mut().for_each(|span| {
                span.style = span.style.dim();
            });
        }
    }
}

fn wrap_row_lines(row: &GenericDisplayRow, desc_col: usize, width: u16) -> Vec<Line<'static>> {
    let full_line = build_full_line(row, desc_col);
    let continuation_indent = row.wrap_indent.unwrap_or(0);
    let indent = " ".repeat(continuation_indent);
    let options = textwrap::Options::new(width.max(1) as usize)
        .initial_indent("")
        .subsequent_indent(&indent);
    textwrap::wrap(&full_line.to_string(), options)
        .into_iter()
        .map(|cow| Line::from(cow.to_string()))
        .collect()
}

fn compute_desc_col(rows: &[GenericDisplayRow], start_idx: usize, visible_items: usize, content_width: u16) -> usize {
    if content_width <= 1 {
        return 0;
    }
    let max_desc_col = content_width.saturating_sub(1) as usize;
    let max_auto_desc_col = max_desc_col.min(
        ((content_width as usize * 7) / 10).max(1),
    );
    let max_name_width = rows
        .iter()
        .skip(start_idx)
        .take(visible_items)
        .map(|row| UnicodeWidthStr::width(row.name.as_str()))
        .max()
        .unwrap_or(0);
    max_name_width.saturating_add(2).min(max_auto_desc_col)
}

/// Render a list of rows using the provided ScrollState.
/// Returns the number of terminal lines actually rendered.
pub fn render_rows(
    area: Rect,
    buf: &mut Buffer,
    rows: &[GenericDisplayRow],
    state: &ScrollState,
    max_results: usize,
    empty_message: &str,
) -> u16 {
    if rows.is_empty() {
        if area.height > 0 {
            Line::from(empty_message.dim().italic()).render(area, buf);
        }
        return u16::from(area.height > 0);
    }

    let max_items = max_results.min(rows.len()).min(area.height.max(1) as usize);
    if max_items == 0 {
        return 0;
    }

    let visible_items = max_items;

    let mut start_idx = state.scroll_top.min(rows.len().saturating_sub(1));
    if let Some(sel) = state.selected_idx {
        if sel < start_idx {
            start_idx = sel;
        } else {
            let bottom = start_idx.saturating_add(max_items.saturating_sub(1));
            if sel > bottom {
                start_idx = sel + 1 - max_items;
            }
        }
    }

    let desc_col = compute_desc_col(rows, start_idx, visible_items, area.width);
    let content_width = area.width.max(1);

    let mut cur_y = area.y;
    let mut rendered_lines: u16 = 0;
    for (i, row) in rows.iter().enumerate().skip(start_idx).take(max_items) {
        if cur_y >= area.y + area.height {
            break;
        }
        let mut wrapped = wrap_row_lines(row, desc_col, content_width);
        apply_row_state_style(
            &mut wrapped,
            Some(i) == state.selected_idx && !row.is_disabled,
            row.is_disabled,
        );
        for line in wrapped {
            if cur_y >= area.y + area.height {
                break;
            }
            line.render(
                Rect { x: area.x, y: cur_y, width: area.width, height: 1 },
                buf,
            );
            cur_y = cur_y.saturating_add(1);
            rendered_lines = rendered_lines.saturating_add(1);
        }
    }
    rendered_lines
}

/// Measure how many terminal rows are needed to render up to `max_results` items.
pub fn measure_rows_height(
    rows: &[GenericDisplayRow],
    state: &ScrollState,
    max_results: usize,
    width: u16,
) -> u16 {
    if rows.is_empty() {
        return 1;
    }
    let content_width = width.saturating_sub(1).max(1);
    let visible_items = max_results.min(rows.len());
    let mut start_idx = state.scroll_top.min(rows.len().saturating_sub(1));
    if let Some(sel) = state.selected_idx {
        if sel < start_idx {
            start_idx = sel;
        } else if visible_items > 0 {
            let bottom = start_idx + visible_items - 1;
            if sel > bottom {
                start_idx = sel + 1 - visible_items;
            }
        }
    }
    let desc_col = compute_desc_col(rows, start_idx, visible_items, content_width);
    let mut total: u16 = 0;
    for row in rows.iter().skip(start_idx).take(visible_items) {
        let wrapped_lines = wrap_row_lines(row, desc_col, content_width).len();
        total = total.saturating_add(wrapped_lines as u16);
    }
    total.max(1)
}
