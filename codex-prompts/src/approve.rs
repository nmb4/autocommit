use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Widget;

use crate::scroll_state::ScrollState;
use crate::selection_rendering::{self, GenericDisplayRow, render_menu_surface, measure_rows_height};

/// A choice in an approval prompt.
#[derive(Clone)]
pub struct ApproveChoice {
    pub label: String,
    pub shortcut: Option<char>,
}

/// Result of an approval prompt.
#[derive(Debug, Clone)]
pub enum ApproveResult {
    /// User selected a choice (index into the choices slice).
    Choice(usize),
    /// User cancelled (Esc / Ctrl+C).
    Cancelled,
}

/// An approval/confirmation prompt.
pub struct ApprovePrompt {
    message: String,
    detail: Option<String>,
    choices: Vec<ApproveChoice>,
    state: ScrollState,
    done: bool,
    result: Option<ApproveResult>,
}

impl ApprovePrompt {
    pub fn new(message: String, choices: Vec<ApproveChoice>) -> Self {
        let mut state = ScrollState::new();
        if !choices.is_empty() {
            state.selected_idx = Some(0);
        }
        Self {
            message,
            detail: None,
            choices,
            state,
            done: false,
            result: None,
        }
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn result(&self) -> Option<&ApproveResult> {
        self.result.as_ref()
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.done {
            return;
        }
        // Check shortcut keys first
        if let KeyCode::Char(c) = key.code {
            for (idx, choice) in self.choices.iter().enumerate() {
                if choice.shortcut == Some(c) {
                    self.result = Some(ApproveResult::Choice(idx));
                    self.done = true;
                    return;
                }
            }
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let len = self.choices.len();
                self.state.move_up_wrap(len);
                self.state.ensure_visible(len, len);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = self.choices.len();
                self.state.move_down_wrap(len);
                self.state.ensure_visible(len, len);
            }
            KeyCode::Enter => {
                if let Some(idx) = self.state.selected_idx {
                    self.result = Some(ApproveResult::Choice(idx));
                    self.done = true;
                }
            }
            KeyCode::Esc => {
                self.result = Some(ApproveResult::Cancelled);
                self.done = true;
            }
            _ => {}
        }
    }

    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        self.choices
            .iter()
            .enumerate()
            .map(|(idx, choice)| {
                let is_selected = self.state.selected_idx == Some(idx);
                let prefix = if is_selected { '›' } else { ' ' };
                let n = idx + 1;
                let shortcut_hint = choice.shortcut
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default();
                GenericDisplayRow {
                    name: format!("{prefix} {n}. {}{shortcut_hint}", choice.label),
                    description: None,
                    wrap_indent: Some(4),
                    is_disabled: false,
                    disabled_reason: None,
                }
            })
            .collect()
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        let rows = self.build_rows();
        let rows_height = measure_rows_height(&rows, &self.state, self.choices.len().max(1), width.saturating_sub(2));

        let mut header_lines = 1u16; // message
        if self.detail.is_some() {
            header_lines += 2; // detail + blank line
        }
        header_lines += 1; // blank line
        let footer = 1u16;
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

        let mut header_lines: Vec<Line> = vec![Line::from(self.message.clone().bold())];
        if let Some(detail) = &self.detail {
            header_lines.push(Line::from(detail.clone().italic()));
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

        let rows = self.build_rows();
        selection_rendering::render_rows(
            rows_area,
            buf,
            &rows,
            &self.state,
            self.choices.len().max(1),
            "no choices",
        );

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
