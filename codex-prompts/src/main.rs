use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::Terminal;
use std::io;

use codex_prompts::approve::ApproveResult;
use codex_prompts::questions::QuestionsResult;
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Select => run_select(),
        Commands::Approve => run_approve(),
        Commands::Questions => run_questions(),
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal() -> Result<()> {
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

fn run_select() -> Result<()> {
    let mut terminal = setup_terminal()?;

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

    let result = run_prompt_loop(&mut terminal, &mut prompt);
    restore_terminal()?;

    match result {
        SelectResult::Selected(idx) => println!("Selected: index {idx}"),
        SelectResult::Cancelled => println!("Cancelled"),
    }
    Ok(())
}

fn run_approve() -> Result<()> {
    let mut terminal = setup_terminal()?;

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

    let result = run_approve_loop(&mut terminal, &mut prompt);
    restore_terminal()?;

    match result {
        ApproveResult::Choice(idx) => println!("Choice: index {idx}"),
        ApproveResult::Cancelled => println!("Cancelled"),
    }
    Ok(())
}

fn run_questions() -> Result<()> {
    let mut terminal = setup_terminal()?;

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

    let result = run_questions_loop(&mut terminal, &mut prompt);
    restore_terminal()?;

    match result {
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

fn run_prompt_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    prompt: &mut SelectPrompt,
) -> SelectResult {
    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);
        let area = Rect::new(
            0,
            size.height.saturating_sub(height),
            size.width,
            height.min(size.height),
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
) -> ApproveResult {
    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);
        let area = Rect::new(
            0,
            size.height.saturating_sub(height),
            size.width,
            height.min(size.height),
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
) -> QuestionsResult {
    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);
        let area = Rect::new(
            0,
            size.height.saturating_sub(height),
            size.width,
            height.min(size.height),
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
