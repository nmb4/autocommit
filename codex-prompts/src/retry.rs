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
    Accept,
    Retry { note: String },
    Abort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Options,
    Note,
}

pub struct RetryPrompt {
    message: String,
    detail_lines: Vec<String>,
    state: ScrollState,
    focus: Focus,
    note_draft: String,
    done: bool,
    result: Option<RetryResult>,
}

const ACCEPT: usize = 0;
const RETRY: usize = 1;
const NUM_CHOICES: usize = 3;

impl RetryPrompt {
    pub fn new(message: String, detail_lines: Vec<String>) -> Self {
        let mut state = ScrollState::new();
        state.selected_idx = Some(ACCEPT);
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

        match self.focus {
            Focus::Options => {
                // Shortcuts only active in options mode
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
                        // j/k handled below
                        'j' | 'k' => {}
                        _ => return,
                    }
                }
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.move_up();
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.move_down();
                    }
                    KeyCode::Tab => {
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
                }
            }
            Focus::Note => {
                // In note mode, char keys go to the draft — no shortcuts
                match key.code {
                    KeyCode::Tab | KeyCode::Esc => {
                        self.focus = Focus::Options;
                    }
                    KeyCode::Enter => {
                        self.submit();
                    }
                    KeyCode::Backspace => {
                        self.note_draft.pop();
                    }
                    KeyCode::Up => {
                        self.move_up();
                    }
                    KeyCode::Down => {
                        self.move_down();
                    }
                    KeyCode::Char(c) => {
                        self.note_draft.push(c);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Move up without wrapping — clamp at 0.
    fn move_up(&mut self) {
        if let Some(idx) = self.state.selected_idx {
            if idx > 0 {
                self.state.selected_idx = Some(idx - 1);
            }
        }
    }

    /// Move down without wrapping — clamp at last item.
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
            Some(ACCEPT) => {
                self.result = Some(RetryResult::Accept);
            }
            Some(RETRY) => {
                self.result = Some(RetryResult::Retry { note });
            }
            _ => {
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
        let header = 1u16
            + self.detail_lines.len() as u16
            + 1; // blank line
        let rows = NUM_CHOICES as u16;
        let spacer = 1u16; // blank line before footer
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

        // Header
        Line::from(self.message.clone().bold()).render(
            Rect { x: content_area.x, y, width: content_area.width, height: 1 },
            buf,
        );
        y += 1;

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

        y += 1; // spacer

        // Choices — hide shortcut labels when in note mode
        let hide_shortcuts = self.focus == Focus::Note;
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
            let shortcut_part = if hide_shortcuts {
                String::new()
            } else {
                format!(" ({shortcut})")
            };

            let row_text = format!("{prefix} {}. {}{shortcut_part}", idx + 1, label);
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
                        let cursor = if self.should_show_cursor() { "█" } else { " " };
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

        // Spacer line before footer
        y += 1;

        // Footer hints — white keys, dimmed text, centered dot separator
        if y < content_area.y + content_area.height {
            let sep = Span::styled(" · ", Style::default().dim());
            let dim = Style::default().dim();
            let white = Style::default().fg(Color::White);

            let spans: Vec<Span> = match self.focus {
                Focus::Options => vec![
                    Span::styled("tab", white),
                    Span::styled(" add note", dim),
                    sep.clone(),
                    Span::styled("enter", white),
                    Span::styled(" confirm", dim),
                    sep.clone(),
                    Span::styled("esc", white),
                    Span::styled(" abort", dim),
                ],
                Focus::Note => vec![
                    Span::styled("tab/esc", white),
                    Span::styled(" close note", dim),
                    sep.clone(),
                    Span::styled("enter", white),
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
