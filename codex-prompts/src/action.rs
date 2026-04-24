use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Widget;

use crate::scroll_state::ScrollState;
use crate::selection_rendering::{menu_surface_padding_height, render_menu_surface};

#[derive(Debug, Clone)]
pub enum ActionResult {
    Accept,
    Retry { note: String },
    ToggleLongCommits,
    Abort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Options,
    Note,
}

pub struct ActionPrompt {
    message: String,
    detail_lines: Vec<String>,
    long_commits_enabled: bool,
    state: ScrollState,
    focus: Focus,
    note_draft: String,
    done: bool,
    result: Option<ActionResult>,
}

const ACCEPT: usize = 0;
const RETRY: usize = 1;
const TOGGLE_LONG: usize = 2;
const NUM_CHOICES: usize = 4;
const PLACEHOLDER: &str = "note…";

impl ActionPrompt {
    pub fn new(message: String, detail_lines: Vec<String>, long_commits_enabled: bool) -> Self {
        let mut state = ScrollState::new();
        state.selected_idx = Some(ACCEPT);
        Self {
            message,
            detail_lines,
            long_commits_enabled,
            state,
            focus: Focus::Options,
            note_draft: String::new(),
            done: false,
            result: None,
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn result(&self) -> Option<&ActionResult> {
        self.result.as_ref()
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.done {
            return;
        }

        match self.focus {
            Focus::Options => {
                if let KeyCode::Char(c) = key.code {
                    // Number shortcuts
                    if let Some(digit) = c.to_digit(10) {
                        if digit >= 1 && digit as usize <= NUM_CHOICES {
                            self.state.selected_idx = Some((digit - 1) as usize);
                            self.submit();
                            return;
                        }
                    }
                    // Letter shortcuts
                    match c {
                        'a' => {
                            self.result = Some(ActionResult::Accept);
                            self.done = true;
                            return;
                        }
                        'r' => {
                            self.result = Some(ActionResult::Retry {
                                note: self.note_draft.clone(),
                            });
                            self.done = true;
                            return;
                        }
                        'l' => {
                            self.result = Some(ActionResult::ToggleLongCommits);
                            self.done = true;
                            return;
                        }
                        'n' | 'q' => {
                            self.result = Some(ActionResult::Abort);
                            self.done = true;
                            return;
                        }
                        'j' | 'k' => {}
                        _ => return,
                    }
                }
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => self.move_up(),
                    KeyCode::Down | KeyCode::Char('j') => self.move_down(),
                    KeyCode::Tab => {
                        if self.state.selected_idx == Some(RETRY) {
                            self.focus = Focus::Note;
                        }
                    }
                    KeyCode::Enter => self.submit(),
                    KeyCode::Esc => {
                        self.result = Some(ActionResult::Abort);
                        self.done = true;
                    }
                    _ => {}
                }
            }
            Focus::Note => match key.code {
                KeyCode::Tab | KeyCode::Esc => {
                    self.focus = Focus::Options;
                }
                KeyCode::Enter => self.submit(),
                KeyCode::Backspace => {
                    self.note_draft.pop();
                }
                KeyCode::Char(c) => {
                    self.note_draft.push(c);
                }
                _ => {}
            },
        }
    }

    fn move_up(&mut self) {
        if let Some(idx) = self.state.selected_idx {
            if idx > 0 {
                self.state.selected_idx = Some(idx - 1);
            }
        }
    }

    fn move_down(&mut self) {
        if let Some(idx) = self.state.selected_idx {
            if idx + 1 < NUM_CHOICES {
                self.state.selected_idx = Some(idx + 1);
            }
        }
    }

    fn submit(&mut self) {
        let note = self.note_draft.clone();
        match self.state.selected_idx {
            Some(ACCEPT) => self.result = Some(ActionResult::Accept),
            Some(RETRY) => self.result = Some(ActionResult::Retry { note }),
            Some(TOGGLE_LONG) => self.result = Some(ActionResult::ToggleLongCommits),
            _ => self.result = Some(ActionResult::Abort),
        }
        self.done = true;
    }

    fn cursor_visible(&self) -> bool {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        (ms % 1000) < 500
    }

    pub fn desired_height(&self, _width: u16) -> u16 {
        let header = 1u16 + self.detail_lines.len() as u16 + 1;
        let rows = NUM_CHOICES as u16;
        let spacer = 1u16;
        let footer = 1u16;
        let padding = menu_surface_padding_height();
        header + rows + spacer + footer + padding
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let content_area = render_menu_surface(area, buf);
        if content_area.width == 0 || content_area.height == 0 {
            return;
        }

        let mut y = content_area.y;
        let max_y = content_area.y + content_area.height;

        // Header
        Line::from(self.message.clone().bold()).render(
            Rect {
                x: content_area.x,
                y,
                width: content_area.width,
                height: 1,
            },
            buf,
        );
        y += 1;

        for line in &self.detail_lines {
            if y >= max_y {
                return;
            }
            Line::from(line.clone()).render(
                Rect {
                    x: content_area.x,
                    y,
                    width: content_area.width,
                    height: 1,
                },
                buf,
            );
            y += 1;
        }
        y += 1; // spacer

        // Choices
        for idx in 0..NUM_CHOICES {
            if y >= max_y {
                break;
            }
            let is_selected = self.state.selected_idx == Some(idx);
            let is_note_focused = self.focus == Focus::Note;
            let has_text = !self.note_draft.is_empty();
            let (label, shortcut, has_note) = match idx {
                ACCEPT => ("Accept", 'a', false),
                RETRY => ("Retry", 'r', true),
                TOGGLE_LONG => (
                    if self.long_commits_enabled {
                        "Use short commits"
                    } else {
                        "Use long commits"
                    },
                    'l',
                    false,
                ),
                _ => ("Abort", 'n', false),
            };

            let mut spans: Vec<Span> = Vec::new();
            let row_style = if is_selected {
                Style::default().fg(Color::Cyan).bold()
            } else {
                Style::default()
            };
            let dim = Style::default().dim();

            let prefix = if is_selected { "› " } else { "  " };
            spans.push(Span::styled(
                format!("{prefix}{}. {label}", idx + 1),
                row_style,
            ));

            if is_selected && has_note {
                if has_text {
                    spans.push(Span::styled(", ", dim));
                    let note_style = if is_note_focused {
                        Style::default().fg(Color::Blue)
                    } else {
                        dim
                    };
                    spans.push(Span::styled(self.note_draft.clone(), note_style));
                    if is_note_focused {
                        let cursor = if self.cursor_visible() { "█" } else { " " };
                        spans.push(Span::styled(cursor, Style::default().fg(Color::Blue)));
                    }
                    spans.push(Span::styled(format!(" ({shortcut})"), dim));
                } else if is_note_focused {
                    spans.push(Span::styled(" ", Style::default()));
                    spans.push(Span::styled(
                        PLACEHOLDER,
                        Style::default().fg(Color::DarkGray).dim(),
                    ));
                    let cursor = if self.cursor_visible() { "█" } else { " " };
                    spans.push(Span::styled(cursor, Style::default().fg(Color::Blue)));
                    spans.push(Span::styled(format!(" ({shortcut})"), dim));
                } else {
                    spans.push(Span::styled(format!(" ({shortcut})"), dim));
                }
            } else {
                spans.push(Span::styled(format!(" ({shortcut})"), dim));
            }

            Line::from(spans).render(
                Rect {
                    x: content_area.x,
                    y,
                    width: content_area.width,
                    height: 1,
                },
                buf,
            );
            y += 1;
        }

        // Spacer before footer
        y += 1;

        // Footer hints
        if y < max_y {
            let sep = Span::styled(" · ", Style::default().dim());
            let dim = Style::default().dim();
            let w = Style::default().fg(Color::White);
            let can_add_note = self.state.selected_idx == Some(RETRY);

            let spans: Vec<Span> = match self.focus {
                Focus::Options => {
                    let mut hints = Vec::new();
                    if can_add_note {
                        hints.push(Span::styled("tab", w));
                        hints.push(Span::styled(" add note", dim));
                        hints.push(sep.clone());
                    }
                    hints.push(Span::styled("enter", w));
                    hints.push(Span::styled(" confirm", dim));
                    hints.push(sep.clone());
                    hints.push(Span::styled("esc", w));
                    hints.push(Span::styled(" abort", dim));
                    hints
                }
                Focus::Note => vec![
                    Span::styled("tab/esc", w),
                    Span::styled(" close note", dim),
                    sep.clone(),
                    Span::styled("enter", w),
                    Span::styled(" confirm", dim),
                ],
            };
            Line::from(spans).render(
                Rect {
                    x: content_area.x + 2,
                    y,
                    width: content_area.width.saturating_sub(2),
                    height: 1,
                },
                buf,
            );
        }
    }
}
