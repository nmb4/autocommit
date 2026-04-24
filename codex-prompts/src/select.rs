use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::scroll_state::ScrollState;
use crate::selection_rendering::{self, GenericDisplayRow, render_menu_surface, measure_rows_height};

/// One selectable item in a list.
#[derive(Default, Clone)]
pub struct SelectItem {
    pub name: String,
    pub description: Option<String>,
    pub is_disabled: bool,
}

/// Result of a selection prompt.
#[derive(Debug, Clone)]
pub enum SelectResult {
    /// User selected an item (index into the items slice).
    Selected(usize),
    /// User cancelled (Esc / Ctrl+C).
    Cancelled,
}

const MAX_POPUP_ROWS: usize = 15;

/// A single-selection list prompt.
pub struct SelectPrompt {
    title: String,
    subtitle: Option<String>,
    items: Vec<SelectItem>,
    state: ScrollState,
    done: bool,
    result: Option<SelectResult>,
}

impl SelectPrompt {
    pub fn new(title: String, items: Vec<SelectItem>) -> Self {
        let mut state = ScrollState::new();
        if !items.is_empty() {
            state.selected_idx = Some(0);
        }
        Self {
            title,
            subtitle: None,
            items,
            state,
            done: false,
            result: None,
        }
    }

    pub fn with_subtitle(mut self, subtitle: String) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn result(&self) -> Option<&SelectResult> {
        self.result.as_ref()
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.done {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
            }
            KeyCode::Enter => {
                if let Some(idx) = self.state.selected_idx {
                    self.result = Some(SelectResult::Selected(idx));
                    self.done = true;
                }
            }
            KeyCode::Esc => {
                self.result = Some(SelectResult::Cancelled);
                self.done = true;
            }
            KeyCode::Char(c) => {
                if let Some(digit) = c.to_digit(10) {
                    if digit > 0 {
                        let idx = (digit - 1) as usize;
                        if idx < self.items.len() && !self.items[idx].is_disabled {
                            self.state.selected_idx = Some(idx);
                            self.result = Some(SelectResult::Selected(idx));
                            self.done = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn move_up(&mut self) {
        let len = self.items.len();
        self.state.move_up_wrap(len);
        self.skip_disabled_up();
        let visible = MAX_POPUP_ROWS.min(len);
        self.state.ensure_visible(len, visible);
    }

    fn move_down(&mut self) {
        let len = self.items.len();
        self.state.move_down_wrap(len);
        self.skip_disabled_down();
        let visible = MAX_POPUP_ROWS.min(len);
        self.state.ensure_visible(len, visible);
    }

    fn skip_disabled_down(&mut self) {
        let len = self.items.len();
        for _ in 0..len {
            if let Some(idx) = self.state.selected_idx {
                if self.items.get(idx).is_some_and(|item| item.is_disabled) {
                    self.state.move_down_wrap(len);
                } else {
                    break;
                }
            }
        }
    }

    fn skip_disabled_up(&mut self) {
        let len = self.items.len();
        for _ in 0..len {
            if let Some(idx) = self.state.selected_idx {
                if self.items.get(idx).is_some_and(|item| item.is_disabled) {
                    self.state.move_up_wrap(len);
                } else {
                    break;
                }
            }
        }
    }

    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        self.items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let is_selected = self.state.selected_idx == Some(idx);
                let prefix = if is_selected { '›' } else { ' ' };
                let n = idx + 1;
                let prefix_label = if item.is_disabled {
                    format!("{prefix}   {}.", " ".repeat(n.to_string().len()))
                } else {
                    format!("{prefix} {n}. ")
                };
                let wrap_indent = UnicodeWidthStr::width(prefix_label.as_str());
                GenericDisplayRow {
                    name: format!("{prefix_label}{}", item.name),
                    description: item.description.clone(),
                    wrap_indent: Some(wrap_indent),
                    is_disabled: item.is_disabled,
                    disabled_reason: None,
                }
            })
            .collect()
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        let rows = self.build_rows();
        let max_items = MAX_POPUP_ROWS.min(self.items.len().max(1));
        let rows_height = measure_rows_height(&rows, &self.state, max_items, width.saturating_sub(2));

        let mut header_lines = 1u16; // title
        if self.subtitle.is_some() {
            header_lines += 1;
        }
        header_lines += 1; // blank line after header
        let footer = 1u16; // hint line
        let padding = selection_rendering::menu_surface_padding_height();

        header_lines + rows_height + footer + padding
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let content_area = render_menu_surface(area, buf);
        if content_area.width == 0 || content_area.height == 0 {
            return;
        }

        // Header
        let mut header_lines: Vec<Line> = vec![Line::from(self.title.clone().bold())];
        if let Some(sub) = &self.subtitle {
            header_lines.push(Line::from(sub.clone().dim()));
        }
        header_lines.push(Line::from(""));

        let header_height = header_lines.len() as u16;
        let footer_height: u16 = 1;

        let [header_area, rows_area, footer_area] = Layout::vertical([
            Constraint::Length(header_height),
            Constraint::Fill(1),
            Constraint::Length(footer_height),
        ])
        .areas(content_area);

        for (offset, line) in header_lines.into_iter().enumerate() {
            let y = header_area.y + offset as u16;
            if y >= header_area.y + header_area.height {
                break;
            }
            line.render(
                Rect { x: header_area.x, y, width: header_area.width, height: 1 },
                buf,
            );
        }

        // Rows
        let rows = self.build_rows();
        let max_items = MAX_POPUP_ROWS.min(self.items.len().max(1));
        render_rows(&mut rows_area.clone(), buf, &rows, &self.state, max_items, "no items");

        // Footer hint
        let hint = Line::from(vec![
            "Press ".into(),
            "enter".dim(),
            " to confirm or ".into(),
            "esc".dim(),
            " to cancel".into(),
        ]);
        hint.render(
            Rect { x: footer_area.x + 2, y: footer_area.y, width: footer_area.width.saturating_sub(2), height: 1 },
            buf,
        );
    }
}

fn render_rows(area: &mut Rect, buf: &mut Buffer, rows: &[GenericDisplayRow], state: &ScrollState, max_results: usize, empty_message: &str) {
    selection_rendering::render_rows(*area, buf, rows, state, max_results, empty_message);
}
