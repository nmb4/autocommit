use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::Terminal;
use std::io::{self, Write};

use codex_prompts::approve::ApproveResult;
use codex_prompts::questions::QuestionsResult;
use codex_prompts::action::ActionResult;
use codex_prompts::select::SelectResult;
use codex_prompts::*;

#[derive(Parser)]
#[command(name = "codex-prompts-cli")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show a single-selection list prompt
    Select,
    /// Show an approval/confirmation prompt
    Approve,
    /// Show a multi-question prompt with options + notes
    Questions,
    /// Show a generation review prompt (accept/retry+note/abort)
    Retry,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Select => run_select(),
        Commands::Approve => run_approve(),
        Commands::Questions => run_questions(),
        Commands::Retry => run_retry(),
    }
}

fn setup_terminal() -> Result<(Terminal<CrosstermBackend<io::Stdout>>, u16)> {
    enable_raw_mode()?;

    // Get current cursor position before starting prompt
    let cursor_pos = crossterm::cursor::position()
        .unwrap_or((0, 0));

    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;

    Ok((terminal, cursor_pos.1))
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    Ok(())
}

fn clear_prompt_area(start_row: u16, height: u16) -> Result<()> {
    // Move back to the start of the prompt area
    let mut current_row = start_row.saturating_add(height).saturating_sub(1);
    let target_row = start_row;

    while current_row > target_row {
        print!("\x1b[{};H", current_row + 1); // Move to row
        print!("\x1b[2K"); // Clear line
        current_row = current_row.saturating_sub(1);
    }

    // Clear the first line and position cursor there
    print!("\x1b[{};H", target_row + 1);
    print!("\x1b[2K");
    io::stdout().flush()?;

    Ok(())
}

fn run_select() -> Result<()> {
    let (mut terminal, start_row) = setup_terminal()?;

    let items = vec![
        SelectItem {
            name: "Read Only".to_string(),
            description: Some("Codex can read files".to_string()),
            is_disabled: false,
        },
        SelectItem {
            name: "Full Access".to_string(),
            description: Some("Codex can edit files".to_string()),
            is_disabled: false,
        },
        SelectItem {
            name: "Auto Edit".to_string(),
            description: Some("Codex can edit files without approval".to_string()),
            is_disabled: false,
        },
        SelectItem {
            name: "Disabled Option".to_string(),
            description: Some("This option is disabled".to_string()),
            is_disabled: true,
        },
    ];

    let mut prompt = SelectPrompt::new("Select Approval Mode".to_string(), items)
        .with_subtitle("Choose how Codex interacts with your files".to_string());

    let result = run_prompt_loop(&mut terminal, &mut prompt, start_row);
    restore_terminal()?;

    // Clear the prompt area
    let height = prompt.desired_height(terminal.size()?.width);
    clear_prompt_area(start_row, height)?;

    // Print result to normal buffer
    match &result {
        SelectResult::Selected(idx) => println!("Selected: index {idx}"),
        SelectResult::Cancelled => println!("Cancelled"),
    }

    Ok(())
}

fn run_approve() -> Result<()> {
    let (mut terminal, start_row) = setup_terminal()?;

    let choices = vec![
        ApproveChoice {
            label: "Yes, proceed".to_string(),
            shortcut: Some('y'),
        },
        ApproveChoice {
            label: "Yes, and don't ask again for this command".to_string(),
            shortcut: Some('a'),
        },
        ApproveChoice {
            label: "No, and tell Codex what to do differently".to_string(),
            shortcut: Some('n'),
        },
    ];

    let mut prompt = ApprovePrompt::new(
        "Would you like to run the following command?".to_string(),
        choices,
    )
    .with_detail("$ git add -A && git commit -m \"feat: add new feature\"".to_string());

    let result = run_approve_loop(&mut terminal, &mut prompt, start_row);
    restore_terminal()?;

    // Clear the prompt area
    let height = prompt.desired_height(terminal.size()?.width);
    clear_prompt_area(start_row, height)?;

    // Print result to normal buffer
    match &result {
        ApproveResult::Choice(idx) => println!("Choice: index {idx}"),
        ApproveResult::Cancelled => println!("Cancelled"),
    }

    Ok(())
}

fn run_questions() -> Result<()> {
    let (mut terminal, start_row) = setup_terminal()?;

    let questions = vec![
        Question {
            id: "area".to_string(),
            question: "What area would you like to work on?".to_string(),
            options: vec![
                QuestionOption {
                    label: "Frontend".to_string(),
                    description: "React, CSS, and UI components.".to_string(),
                },
                QuestionOption {
                    label: "Backend".to_string(),
                    description: "API endpoints, database, services.".to_string(),
                },
                QuestionOption {
                    label: "Infrastructure".to_string(),
                    description: "CI/CD, Docker, Kubernetes.".to_string(),
                },
            ],
            is_other: true,
        },
        Question {
            id: "goal".to_string(),
            question: "What's your main goal?".to_string(),
            options: vec![],
            is_other: false,
        },
    ];

    let mut prompt = QuestionsPrompt::new(questions);

    let result = run_questions_loop(&mut terminal, &mut prompt, start_row);
    restore_terminal()?;

    // Clear the prompt area
    let height = prompt.desired_height(terminal.size()?.width);
    clear_prompt_area(start_row, height)?;

    // Print result to normal buffer
    match &result {
        QuestionsResult::Answered(answers) => {
            for (i, answer) in answers.iter().enumerate() {
                println!(
                    "Q{}: selected={:?}, notes={:?}",
                    i + 1,
                    answer.selected_index,
                    answer.notes
                );
            }
        }
        QuestionsResult::Cancelled => println!("Cancelled"),
    }

    Ok(())
}

fn run_retry() -> Result<()> {
    let (mut terminal, start_row) = setup_terminal()?;

    let detail_lines = vec![
        "feat(cli): add action prompt for user actions".to_string(),
        "".to_string(),
        "This adds a new ActionPrompt type that accepts user input".to_string(),
        "with options: Accept / Retry (with note) / Abort.".to_string(),
        "".to_string(),
        "  codex-prompts/src/action.rs  | 85 ++++++++++".to_string(),
        "  codex-prompts/src/main.rs   | 42 +++++".to_string(),
        "  2 files changed, 127 insertions(+)".to_string(),
    ];

    let mut prompt = ActionPrompt::new(
        "Action Required".to_string(),
        detail_lines,
    );

    let result = run_retry_loop(&mut terminal, &mut prompt, start_row);
    restore_terminal()?;

    // Clear the prompt area
    let height = prompt.desired_height(terminal.size()?.width);
    clear_prompt_area(start_row, height)?;

    // Print result to normal buffer
    match &result {
        ActionResult::Accept => println!("Accepted"),
        ActionResult::Retry { note } => {
            if note.is_empty() {
                println!("Retry (no note)");
            } else {
                println!("Retry with note: {note:?}");
            }
        }
        ActionResult::Abort => println!("Aborted"),
    }

    Ok(())
}

fn run_prompt_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    prompt: &mut SelectPrompt,
    start_row: u16,
) -> SelectResult {
    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);

        // Position the prompt starting from the current cursor row
        let area = Rect::new(
            0,
            start_row,
            size.width,
            height.min(size.height.saturating_sub(start_row)),
        );

        terminal
            .draw(|f| {
                let mut buf = f.buffer_mut();
                prompt.render(area, &mut buf);
            })
            .ok();

        if event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    prompt.handle_key(key);
                    if prompt.is_done() {
                        return prompt.result().cloned().unwrap_or(SelectResult::Cancelled);
                    }
                }
            }
        }
    }
}

fn run_approve_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    prompt: &mut ApprovePrompt,
    start_row: u16,
) -> ApproveResult {
    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);
        let area = Rect::new(
            0,
            start_row,
            size.width,
            height.min(size.height.saturating_sub(start_row)),
        );

        terminal
            .draw(|f| {
                let mut buf = f.buffer_mut();
                prompt.render(area, &mut buf);
            })
            .ok();

        if event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    prompt.handle_key(key);
                    if prompt.is_done() {
                        return prompt.result().cloned().unwrap_or(ApproveResult::Cancelled);
                    }
                }
            }
        }
    }
}

fn run_questions_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    prompt: &mut QuestionsPrompt,
    start_row: u16,
) -> QuestionsResult {
    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);
        let area = Rect::new(
            0,
            start_row,
            size.width,
            height.min(size.height.saturating_sub(start_row)),
        );

        terminal
            .draw(|f| {
                let mut buf = f.buffer_mut();
                prompt.render(area, &mut buf);
            })
            .ok();

        if event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    prompt.handle_key(key);
                    if prompt.is_done() {
                        return prompt.result().cloned().unwrap_or(QuestionsResult::Cancelled);
                    }
                }
            }
        }
    }
}

fn run_retry_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    prompt: &mut ActionPrompt,
    start_row: u16,
) -> ActionResult {
    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);
        let area = Rect::new(
            0,
            start_row,
            size.width,
            height.min(size.height.saturating_sub(start_row)),
        );

        terminal
            .draw(|f| {
                let mut buf = f.buffer_mut();
                prompt.render(area, &mut buf);
            })
            .ok();

        if event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    prompt.handle_key(key);
                    if prompt.is_done() {
                        return prompt.result().cloned().unwrap_or(ActionResult::Abort);
                    }
                }
            }
        }
    }
}
