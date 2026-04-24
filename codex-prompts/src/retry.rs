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
use unicode_width::UnicodeWidthStr;

use crate::scroll_state::ScrollState;
use crate::selection_rendering::{render_menu_surface, menu_surface_padding_height};

/// Result of a retry prompt.
#[derive(Debug, Clone)]
pub enum RetryResult {
    /// Accept the current output.
    Accept,
    /// Retry generation, optionally with a focus note.
    Retry { note: String },
    /// Abort entirely.
    Abort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Options,
    Note,
}

/// An approval prompt tailored for generation review:
/// Accept / Retry (default, with optional inline note) / Abort.
pub struct RetryPrompt {
    message: String,
    detail_lines: Vec<String>,
    state: ScrollState,
    focus: Focus,
    note_draft: String,
    done: bool,
    result: Option<RetryResult>,
}

// Choice indices: 0 = Accept, 1 = Retry, 2 = Abort
const ACCEPT: usize = 0;
const RETRY: usize = 1;
const ABORT: usize = 2;

impl RetryPrompt {
    pub fn new(message: String, detail_lines: Vec<String>) -> Self {
        let mut state = ScrollState::new();
        // Default to Retry
        state.selected_idx = Some(RETRY);
        Self {
            message,
            detail_lines,
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

    pub fn result(&self) -> Option<&RetryResult> {
        self.result.as_ref()
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.done {
            return;
        }

        // Shortcut keys work from any focus
        if let KeyCode::Char(c) = key.code {
            match c {
                'a' => {
                    self.result = Some(RetryResult::Accept);
                    self.done = true;
                    return;
                }
                'r' => {
                    self.result = Some(RetryResult::Retry {
                        note: self.note_draft.clone(),
                    });
                    self.done = true;
                    return;
                }
                'n' | 'q' => {
                    self.result = Some(RetryResult::Abort);
                    self.done = true;
                    return;
                }
                _ => {}
            }
        }

        match self.focus {
            Focus::Options => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    let len = 3;
                    self.state.move_up_wrap(len);
                    self.state.ensure_visible(len, len);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let len = 3;
                    self.state.move_down_wrap(len);
                    self.state.ensure_visible(len, len);
                }
                KeyCode::Tab => {
                    // Open note input on retry row
                    self.state.selected_idx = Some(RETRY);
                    self.focus = Focus::Note;
                }
                KeyCode::Enter => {
                    self.submit();
                }
                KeyCode::Esc => {
                    self.result = Some(RetryResult::Abort);
                    self.done = true;
                }
                _ => {}
            },
            Focus::Note => match key.code {
                KeyCode::Tab | KeyCode::Esc => {
                    self.focus = Focus::Options;
                }
                KeyCode::Enter => {
                    self.submit();
                }
                KeyCode::Backspace => {
                    self.note_draft.pop();
                }
                KeyCode::Up | KeyCode::Down => {
                    // Allow navigating options while in note mode
                    let len = 3;
                    match key.code {
                        KeyCode::Up => self.state.move_up_wrap(len),
                        KeyCode::Down => self.state.move_down_wrap(len),
                        _ => {}
                    }
                }
                KeyCode::Char(c) => {
                    self.note_draft.push(c);
                }
                _ => {}
            },
        }
    }

    fn submit(&mut self) {
        let note = self.note_draft.clone();
        match self.state.selected_idx {
            Some(ACCEPT) => {
                self.result = Some(RetryResult::Accept);
            }
            Some(RETRY) => {
                self.result = Some(RetryResult::Retry { note });
            }
            Some(ABORT) | _ => {
                self.result = Some(RetryResult::Abort);
            }
        }
        self.done = true;
    }

    fn should_show_cursor(&self) -> bool {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        (millis % 1000) < 500
    }

    pub fn desired_height(&self, _width: u16) -> u16 {
        let header = 1u16 // message
            + self.detail_lines.len() as u16
            + 1; // blank line
        let rows = 3u16; // 3 choices
        let footer = 1u16;
        let padding = menu_surface_padding_height();
        header + rows + footer + padding
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

        // Header: bold message
        Line::from(self.message.clone().bold()).render(
            Rect { x: content_area.x, y, width: content_area.width, height: 1 },
            buf,
        );
        y += 1;

        // Detail lines
        for line in &self.detail_lines {
            if y >= content_area.y + content_area.height {
                return;
            }
            Line::from(line.clone()).render(
                Rect { x: content_area.x, y, width: content_area.width, height: 1 },
                buf,
            );
            y += 1;
        }

        // Spacer
        y += 1;

        // Choices
        let choices = [
            ("Accept", 'a', false),
            ("Retry", 'r', true),
            ("Abort", 'n', false),
        ];
        for (idx, (label, shortcut, supports_note)) in choices.iter().enumerate() {
            if y >= content_area.y + content_area.height {
                break;
            }
            let is_selected = self.state.selected_idx == Some(idx);
            let prefix = if is_selected { '›' } else { ' ' };
            let default_marker = if *supports_note { " (default)" } else { "" };

            let row_text = format!("{prefix} {}. {}{default_marker} ({shortcut})", idx + 1, label);
            let style = if is_selected {
                Style::default().fg(Color::Cyan).bold()
            } else {
                Style::default()
            };
            let row_width = UnicodeWidthStr::width(row_text.as_str()) as u16;
            Span::styled(row_text.clone(), style).render(
                Rect {
                    x: content_area.x,
                    y,
                    width: row_width.min(content_area.width),
                    height: 1,
                },
                buf,
            );

            // Inline note on the retry row
            if is_selected && *supports_note {
                let remaining = content_area.width.saturating_sub(row_width).saturating_sub(3);
                if remaining > 0 {
                    if self.focus == Focus::Note {
                        let cursor = if self.should_show_cursor() { "▌" } else { "│" };
                        let note_preview = if self.note_draft.is_empty() {
                            format!(" {cursor}")
                        } else {
                            let max = remaining.saturating_sub(1) as usize;
                            let truncated: String = self.note_draft.chars().take(max).collect();
                            format!(" {truncated}{cursor}")
                        };
                        Span::styled(note_preview, Style::default().fg(Color::Yellow)).render(
                            Rect {
                                x: content_area.x + row_width.min(content_area.width),
                                y,
                                width: content_area.width.saturating_sub(row_width.min(content_area.width)),
                                height: 1,
                            },
                            buf,
                        );
                    } else if !self.note_draft.is_empty() {
                        let max = remaining.saturating_sub(1) as usize;
                        let truncated: String = self.note_draft.chars().take(max).collect();
                        Span::styled(format!(" {truncated}"), Style::default().dim()).render(
                            Rect {
                                x: content_area.x + row_width.min(content_area.width),
                                y,
                                width: content_area.width.saturating_sub(row_width.min(content_area.width)),
                                height: 1,
                            },
                            buf,
                        );
                    }
                }
            }

            y += 1;
        }

        // Footer hints
        if y < content_area.y + content_area.height {
            let hints = match self.focus {
                Focus::Options => vec![
                    "tab to add retry note",
                    "enter to confirm",
                    "esc to abort",
                ],
                Focus::Note => vec![
                    "tab/esc to close note",
                    "enter to confirm",
                ],
            };
            Line::from(hints.join(" | ").dim()).render(
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
