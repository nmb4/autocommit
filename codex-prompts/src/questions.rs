use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::scroll_state::ScrollState;
use crate::selection_rendering::{self, GenericDisplayRow, render_menu_surface, measure_rows_height, menu_surface_inset, menu_surface_padding_height};

/// An option in a question.
#[derive(Clone)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// A question to ask the user.
#[derive(Clone)]
pub struct Question {
    pub id: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    /// If true, adds a "None of the above" option
    pub is_other: bool,
}

/// Answer to a single question.
#[derive(Debug, Clone)]
pub struct QuestionAnswer {
    /// Index of selected option, or None.
    pub selected_index: Option<usize>,
    /// Freeform notes the user typed.
    pub notes: String,
}

/// Result of a multi-question prompt.
#[derive(Debug, Clone)]
pub enum QuestionsResult {
    /// User answered all questions (or submitted with unanswered ones).
    Answered(Vec<QuestionAnswer>),
    /// User cancelled.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Options,
    Notes,
}

struct AnswerState {
    options_state: ScrollState,
    notes: String,
    notes_visible: bool,
    committed: bool,
}

const NOTES_PLACEHOLDER: &str = "Add notes";
const ANSWER_PLACEHOLDER: &str = "Type your answer";
const OTHER_LABEL: &str = "None of the above";
const OTHER_DESC: &str = "Optionally, add details in notes (tab).";

/// A multi-question prompt with option selection + notes.
pub struct QuestionsPrompt {
    questions: Vec<Question>,
    answers: Vec<AnswerState>,
    current_idx: usize,
    focus: Focus,
    done: bool,
    result: Option<QuestionsResult>,
    notes_draft: String,
}

impl QuestionsPrompt {
    pub fn new(questions: Vec<Question>) -> Self {
        let answers: Vec<AnswerState> = questions
            .iter()
            .map(|q| {
                let has_options = !q.options.is_empty();
                let mut options_state = ScrollState::new();
                if has_options {
                    options_state.selected_idx = Some(0);
                }
                AnswerState {
                    options_state,
                    notes: String::new(),
                    notes_visible: !has_options,
                    committed: false,
                }
            })
            .collect();
        Self {
            questions,
            answers,
            current_idx: 0,
            focus: Focus::Options,
            done: false,
            result: None,
            notes_draft: String::new(),
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn result(&self) -> Option<&QuestionsResult> {
        self.result.as_ref()
    }

    fn current_question(&self) -> Option<&Question> {
        self.questions.get(self.current_idx)
    }

    fn current_answer(&self) -> Option<&AnswerState> {
        self.answers.get(self.current_idx)
    }

    fn current_answer_mut(&mut self) -> Option<&mut AnswerState> {
        self.answers.get_mut(self.current_idx)
    }

    fn has_options(&self) -> bool {
        self.current_question()
            .is_some_and(|q| !q.options.is_empty())
    }

    fn options_len(&self) -> usize {
        self.current_question()
            .map(|q| {
                let len = q.options.len();
                if q.is_other { len + 1 } else { len }
            })
            .unwrap_or(0)
    }

    fn notes_ui_visible(&self) -> bool {
        if !self.has_options() {
            return true;
        }
        self.current_answer()
            .is_some_and(|a| a.notes_visible || !a.notes.is_empty())
    }

    fn move_question(&mut self, next: bool) {
        self.save_current();
        let len = self.questions.len();
        if len == 0 { return; }
        let offset = if next { 1 } else { len.saturating_sub(1) };
        self.current_idx = (self.current_idx + offset) % len;
        self.restore_current();
        self.ensure_focus();
    }

    fn save_current(&mut self) {
        let draft = self.notes_draft.clone();
        if let Some(idx) = self.current_idx.checked_add(0) {
            if let Some(answer) = self.answers.get_mut(idx) {
                answer.notes = draft;
            }
        }
    }

    fn restore_current(&mut self) {
        if let Some(answer) = self.current_answer() {
            self.notes_draft = answer.notes.clone();
        } else {
            self.notes_draft.clear();
        }
    }

    fn ensure_focus(&mut self) {
        if !self.has_options() {
            self.focus = Focus::Notes;
            if let Some(a) = self.current_answer_mut() {
                a.notes_visible = true;
            }
        }
    }

    fn select_current_option(&mut self, committed: bool) {
        if let Some(answer) = self.current_answer_mut() {
            answer.committed = committed;
        }
    }

    fn go_next_or_submit(&mut self) {
        self.save_current();
        if self.current_idx + 1 >= self.questions.len() {
            self.submit_answers();
        } else {
            self.move_question(true);
        }
    }

    fn submit_answers(&mut self) {
        self.save_current();
        let answers: Vec<QuestionAnswer> = self
            .questions
            .iter()
            .enumerate()
            .map(|(idx, q)| {
                let state = &self.answers[idx];
                let selected = if !q.options.is_empty() && state.committed {
                    state.options_state.selected_idx
                } else {
                    None
                };
                QuestionAnswer {
                    selected_index: selected,
                    notes: state.notes.clone(),
                }
            })
            .collect();
        self.result = Some(QuestionsResult::Answered(answers));
        self.done = true;
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.done {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if self.has_options() && self.focus == Focus::Notes && self.notes_ui_visible() {
                    self.notes_draft.clear();
                    if let Some(a) = self.current_answer_mut() {
                        a.notes.clear();
                        a.notes_visible = false;
                    }
                    self.focus = Focus::Options;
                    return;
                }
                self.result = Some(QuestionsResult::Cancelled);
                self.done = true;
                return;
            }
            _ => {}
        }

        // Question navigation
        match key.code {
            KeyCode::Left if self.has_options() && self.focus == Focus::Options => {
                self.move_question(false);
                return;
            }
            KeyCode::Right if self.has_options() && self.focus == Focus::Options => {
                self.move_question(true);
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::Options => {
                let options_len = self.options_len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(a) = self.current_answer_mut() {
                            a.options_state.move_up_wrap(options_len);
                            a.committed = false;
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(a) = self.current_answer_mut() {
                            a.options_state.move_down_wrap(options_len);
                            a.committed = false;
                        }
                    }
                    KeyCode::Tab => {
                        if self.current_answer().and_then(|a| a.options_state.selected_idx).is_some() {
                            self.focus = Focus::Notes;
                            if let Some(a) = self.current_answer_mut() {
                                a.notes_visible = true;
                            }
                        }
                    }
                    KeyCode::Enter => {
                        self.select_current_option(true);
                        self.go_next_or_submit();
                    }
                    KeyCode::Char(c) => {
                        if let Some(digit) = c.to_digit(10) {
                            if digit > 0 {
                                let idx = (digit - 1) as usize;
                                if idx < options_len {
                                    if let Some(a) = self.current_answer_mut() {
                                        a.options_state.selected_idx = Some(idx);
                                    }
                                    self.select_current_option(true);
                                    self.go_next_or_submit();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Focus::Notes => {
                match key.code {
                    KeyCode::Tab => {
                        if self.has_options() {
                            self.notes_draft.clear();
                            if let Some(a) = self.current_answer_mut() {
                                a.notes.clear();
                                a.notes_visible = false;
                            }
                            self.focus = Focus::Options;
                            return;
                        }
                    }
                    KeyCode::Backspace => {
                        if self.notes_draft.is_empty() && self.has_options() {
                            self.save_current();
                            if let Some(a) = self.current_answer_mut() {
                                a.notes_visible = false;
                            }
                            self.focus = Focus::Options;
                            return;
                        }
                        self.notes_draft.pop();
                    }
                    KeyCode::Enter => {
                        self.select_current_option(true);
                        self.go_next_or_submit();
                    }
                    KeyCode::Up | KeyCode::Down => {
                        let options_len = self.options_len();
                        if let Some(a) = self.current_answer_mut() {
                            match key.code {
                                KeyCode::Up => a.options_state.move_up_wrap(options_len),
                                KeyCode::Down => a.options_state.move_down_wrap(options_len),
                                _ => {}
                            }
                            a.committed = false;
                        }
                    }
                    KeyCode::Char(c) => {
                        self.notes_draft.push(c);
                    }
                    _ => {}
                }
            }
        }
    }

    fn option_rows(&self) -> Vec<GenericDisplayRow> {
        let Some(q) = self.current_question() else { return vec![] };
        let selected_idx = self.current_answer().and_then(|a| a.options_state.selected_idx);
        let mut rows: Vec<GenericDisplayRow> = q
            .options
            .iter()
            .enumerate()
            .map(|(idx, opt)| {
                let selected = selected_idx.is_some_and(|s| s == idx);
                let prefix = if selected { '›' } else { ' ' };
                let n = idx + 1;
                let prefix_label = format!("{prefix} {n}. ");
                let wrap_indent = UnicodeWidthStr::width(prefix_label.as_str());
                GenericDisplayRow {
                    name: format!("{prefix_label}{}", opt.label),
                    description: Some(opt.description.clone()),
                    wrap_indent: Some(wrap_indent),
                    ..Default::default()
                }
            })
            .collect();

        if q.is_other && !q.options.is_empty() {
            let idx = q.options.len();
            let selected = selected_idx.is_some_and(|s| s == idx);
            let prefix = if selected { '›' } else { ' ' };
            let n = idx + 1;
            let prefix_label = format!("{prefix} {n}. ");
            let wrap_indent = UnicodeWidthStr::width(prefix_label.as_str());
            rows.push(GenericDisplayRow {
                name: format!("{prefix_label}{OTHER_LABEL}"),
                description: Some(OTHER_DESC.to_string()),
                wrap_indent: Some(wrap_indent),
                ..Default::default()
            });
        }
        rows
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        let outer = Rect::new(0, 0, width, u16::MAX);
        let inner = menu_surface_inset(outer);
        let inner_width = inner.width.max(1);

        let has_options = self.has_options();
        let q = self.current_question();
        let question_height = q
            .map(|q| textwrap::wrap(&q.question, inner_width as usize).len())
            .unwrap_or(0) as u16;

        let options_height = if has_options {
            let rows = self.option_rows();
            let mut st = self.current_answer().map(|a| a.options_state).unwrap_or_default();
            if st.selected_idx.is_none() { st.selected_idx = Some(0); }
            measure_rows_height(&rows, &st, rows.len().max(1), inner_width.max(1))
        } else {
            0
        };

        let notes_height = if self.notes_ui_visible() { 3u16 } else { 0 };
        let footer_height: u16 = 1;
        let padding = menu_surface_padding_height();

        1 + question_height + options_height + notes_height + footer_height + padding + 2
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let content_area = render_menu_surface(area, buf);
        if content_area.width == 0 || content_area.height == 0 {
            return;
        }

        let q = self.current_question();
        let question_count = self.questions.len();
        let current_idx = self.current_idx;

        // Progress line
        let progress = if question_count > 0 {
            format!("Question {}/{current_idx}", current_idx + 1)
        } else {
            "No questions".to_string()
        };
        let mut y = content_area.y;
        Line::from(progress.dim()).render(
            Rect { x: content_area.x, y, width: content_area.width, height: 1 },
            buf,
        );
        y += 1;

        // Question text
        if let Some(q) = q {
            let wrapped = textwrap::wrap(&q.question, content_area.width as usize);
            for line in wrapped {
                if y >= content_area.y + content_area.height { return; }
                Line::from(line.to_string()).render(
                    Rect { x: content_area.x, y, width: content_area.width, height: 1 },
                    buf,
                );
                y += 1;
            }
        }

        // Options
        if self.has_options() {
            y += 1; // spacer
            let rows = self.option_rows();
            let options_state = self.current_answer()
                .map(|a| a.options_state)
                .unwrap_or_default();
            let rows_height = measure_rows_height(
                &rows,
                &options_state,
                rows.len().max(1),
                content_area.width,
            );
            let rows_area = Rect { x: content_area.x, y, width: content_area.width, height: rows_height.min(content_area.y + content_area.height - y) };
            selection_rendering::render_rows(
                rows_area,
                buf,
                &rows,
                &options_state,
                rows.len().max(1),
                "No options",
            );
            y += rows_area.height;
        }

        // Notes input
        if self.notes_ui_visible() && y + 3 < content_area.y + content_area.height {
            y += 1;
            let notes_area = Rect { x: content_area.x, y, width: content_area.width, height: 3 };
            let placeholder = if self.has_options() { NOTES_PLACEHOLDER } else { ANSWER_PLACEHOLDER };
            let display = if self.notes_draft.is_empty() {
                Line::from(placeholder.dim())
            } else {
                Line::from(self.notes_draft.as_str())
            };
            Paragraph::new(display).render(notes_area, buf);
            y += 3;
        }

        // Footer hints
        if y < content_area.y + content_area.height {
            let mut hints = Vec::new();
            if self.has_options() && self.focus == Focus::Options {
                hints.push("tab to add notes");
            }
            let is_last = self.current_idx + 1 >= self.questions.len();
            if is_last {
                hints.push("enter to submit all");
            } else {
                hints.push("enter to submit answer");
            }
            if self.questions.len() > 1 {
                hints.push("←/→ navigate questions");
            }
            hints.push("esc to cancel");
            Line::from(hints.join(" | ").dim()).render(
                Rect { x: content_area.x + 2, y, width: content_area.width.saturating_sub(2), height: 1 },
                buf,
            );
        }
    }
}
