mod api;
mod conventions;
mod git;
mod parser;

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use indicatif::{ProgressBar, ProgressStyle};
use parser::{ExecutionStep, ParseResult};
use ratatui::{backend::CrosstermBackend, layout::Rect, layout::Size, Terminal};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use codex_prompts::{ActionPrompt, ActionResult};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "lowercase")]
enum ReasoningEffortArg {
    Auto,
    Instant,
    Low,
    High,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "lowercase")]
enum ProviderArg {
    Auto,
    Inception,
    Openrouter,
}

impl ProviderArg {
    fn resolve(
        self,
        has_inception_key: bool,
        has_openrouter_key: bool,
    ) -> Result<api::Provider> {
        match self {
            ProviderArg::Inception => Ok(api::Provider::Inception),
            ProviderArg::Openrouter => Ok(api::Provider::OpenRouter),
            ProviderArg::Auto => {
                if has_inception_key {
                    Ok(api::Provider::Inception)
                } else if has_openrouter_key {
                    Ok(api::Provider::OpenRouter)
                } else {
                    anyhow::bail!(
                        "No API key found. Set INCEPTION_API_KEY or AC_OR_KEY, or pass --api-key/--or-key."
                    )
                }
            }
        }
    }
}

impl From<ReasoningEffortArg> for api::ReasoningEffort {
    fn from(arg: ReasoningEffortArg) -> Self {
        match arg {
            ReasoningEffortArg::Auto => api::ReasoningEffort::Instant,
            ReasoningEffortArg::Instant => api::ReasoningEffort::Instant,
            ReasoningEffortArg::Low => api::ReasoningEffort::Low,
            ReasoningEffortArg::High => api::ReasoningEffort::High,
        }
    }
}

impl ReasoningEffortArg {
    /// Resolve to actual reasoning effort, either using explicit choice or auto-detecting from diff volume.
    fn resolve(self, diff_volume: usize) -> api::ReasoningEffort {
        match self {
            ReasoningEffortArg::Auto => {
                if diff_volume > 2000 {
                    api::ReasoningEffort::High
                } else if diff_volume > 500 {
                    api::ReasoningEffort::Low
                } else {
                    api::ReasoningEffort::Instant
                }
            }
            _ => self.into(),
        }
    }
}

fn parse_temperature(input: &str) -> Result<f32, String> {
    let value: f32 = input
        .parse()
        .map_err(|_| format!("invalid float: {input}"))?;
    if !(0.0..=2.0).contains(&value) {
        return Err(format!(
            "temperature must be between 0.0 and 2.0 (got {value})"
        ));
    }
    Ok(value)
}

/// autocommit — AI-powered git commit message generator
///
/// Analyzes your repository's staged and unstaged changes, then uses the
/// configured provider/model to generate meaningful, conventional
/// commit messages. Shows you the plan and asks for confirmation before
/// executing anything.
#[derive(Debug, Parser)]
#[command(
    name = "autocommit",
    about = "AI-powered git commits using Inception or OpenRouter models",
    long_about = None,
    version
)]
struct Args {
    /// Path to the git repository (defaults to current directory)
    #[arg(short, long, default_value = ".")]
    path: String,

    /// API provider: auto (default), inception, or openrouter
    #[arg(long, value_enum, default_value = "auto")]
    provider: ProviderArg,

    /// Inception Labs API key (or set INCEPTION_API_KEY env var)
    #[arg(long = "api-key", env = "INCEPTION_API_KEY", hide_env_values = true)]
    inception_api_key: Option<String>,

    /// OpenRouter API key (or set AC_OR_KEY env var)
    #[arg(long = "or-key", env = "AC_OR_KEY", hide_env_values = true)]
    openrouter_api_key: Option<String>,

    /// Override the API base URL
    #[arg(long, env = "INCEPTION_BASE_URL")]
    base_url: Option<String>,

    /// Override the model name (default depends on provider)
    #[arg(long, env = "AUTOCOMMIT_MODEL")]
    model: Option<String>,

    /// Override OpenRouter model ID (or set AC_OR_MODEL env var)
    #[arg(long = "or-model", env = "AC_OR_MODEL")]
    openrouter_model: Option<String>,

    /// Reasoning effort: auto (default), instant, low, or high. Auto selects based on diff volume.
    #[arg(short = 'r', long, value_enum, default_value = "auto")]
    reasoning: ReasoningEffortArg,

    /// Sampling temperature for model output (0.0 to 2.0, default: 0.1)
    #[arg(long, env = "AC_TEMPERATURE", default_value_t = 0.1, value_parser = parse_temperature)]
    temperature: f32,

    /// Use long (multiline) commit messages with a body explaining the change
    #[arg(short = 'l', long)]
    long: bool,

    /// Skip confirmation prompt and execute immediately
    #[arg(short = 'y', long)]
    yes: bool,

    /// Print the raw model output and exit (do not execute)
    #[arg(long)]
    dry_run: bool,

    /// Show the git context that will be sent to the model and exit
    #[arg(long)]
    show_context: bool,

    /// Path to debug log file for provider requests/responses (JSONL)
    #[arg(long, env = "AC_DEBUG_LOG_FILE")]
    debug_log_file: Option<String>,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{} {}", "error:".red().bold(), e);
        // Print cause chain
        let mut source = e.source();
        while let Some(cause) = source {
            eprintln!("  {} {}", "caused by:".dimmed(), cause);
            source = cause.source();
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args = Args::parse();

    // ── 1. Gather git context ────────────────────────────────────────────────
    let sp1 = spinner("Gathering git context...");
    let ctx = git::GitContext::gather(&args.path)
        .context("Failed to gather git context. Are you inside a git repository?")?;
    sp1.finish_and_clear();

    if !ctx.has_changes() {
        println!(
            "{} No staged or unstaged changes detected.",
            "✓".green().bold()
        );
        println!(
            "  Stage some files with {} or make some edits first.",
            "git add".cyan()
        );
        return Ok(());
    }

    // ── 1.5. Discover commit conventions ─────────────────────────────────────
    let conventions = conventions::CommitConventions::discover_any(&ctx.repo_root)?;

    if conventions.is_some() {
        println!("{} Loaded commit conventions", "●".cyan());
    }

    let prompt = ctx.to_prompt_with_conventions(conventions.as_ref());

    if args.show_context {
        println!("{}", "── Git context sent to model ──".dimmed());
        println!("{}", prompt);
        return Ok(());
    }

    // ── 2. Call the model ────────────────────────────────────────────────────
    let provider = args.provider.resolve(
        args.inception_api_key.is_some(),
        args.openrouter_api_key.is_some(),
    )?;
    let debug_log_path = args
        .debug_log_file
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&ctx.repo_root).join(".git").join("autocommit-model-debug.jsonl"));
    let model_override = match provider {
        api::Provider::OpenRouter => args
            .openrouter_model
            .clone()
            .or_else(|| args.model.clone()),
        api::Provider::Inception => args.model.clone(),
    };
    let client = api::ApiClient::new(
        provider,
        args.inception_api_key.clone(),
        args.openrouter_api_key.clone(),
        args.base_url.clone(),
        model_override,
        args.temperature,
        Some(debug_log_path),
    )?;
    let reasoning_effort = args.reasoning.resolve(ctx.diff_volume());
    let initial_long_commits = if args.long {
        true
    } else {
        ctx.prefer_long_commits
    };
    if initial_long_commits && !args.long {
        println!(
            "{} Inferred long commit mode from recent git history.",
            "●".cyan()
        );
    }

    let raw_output = generate_plan(
        &client,
        &prompt,
        reasoning_effort,
        0,
        None,
        None,
        conventions.as_ref(),
        initial_long_commits,
    )
    .await?;

    if args.dry_run {
        println!("{}", "── Raw model output ──".dimmed());
        println!("{}", raw_output);
        return Ok(());
    }

    review_and_execute_plan(
        &args,
        &client,
        &prompt,
        &ctx,
        reasoning_effort,
        raw_output,
        conventions.as_ref(),
        initial_long_commits,
    )
    .await
}

async fn review_and_execute_plan(
    args: &Args,
    client: &api::ApiClient,
    prompt: &str,
    ctx: &git::GitContext,
    reasoning_effort: api::ReasoningEffort,
    mut raw_output: String,
    conventions: Option<&conventions::CommitConventions>,
    long_commits: bool,
) -> Result<()> {
    let mut retry_attempt = 0;
    let mut long_commits = long_commits;
    let mut retry_note: Option<String> = None;

    loop {
        let mut reset_retry_context = false;
        let parsed = parser::parse_commands(&raw_output)
            .context("Failed to parse model output into git commands")?;

        match parsed {
            ParseResult::NothingToCommit { reason } => {
                println!(
                    "{} The model determined there is nothing meaningful to commit.",
                    "○".yellow()
                );
                if let Some(reason) = reason {
                    println!("  {} {}", "Reason:".dimmed(), reason);
                }
                return Ok(());
            }
            ParseResult::Steps(mut steps) => {
                enforce_commit_message_mode(&mut steps, long_commits);
                match execute_or_retry(args, &steps, ctx, long_commits)? {
                PlanAction::Execute => return Ok(()),
                PlanAction::Retry { note } => {
                    if note.is_some() {
                        retry_note = note;
                    }
                    println!("{} Retrying with the current commit mode.", "●".cyan());
                }
                PlanAction::ToggleLongCommits => {
                    long_commits = !long_commits;
                    println!(
                        "{} Switching to {} commit mode and retrying.",
                        "●".cyan(),
                        if long_commits { "long" } else { "short" }
                    );
                    reset_retry_context = true;
                }
                PlanAction::Abort => {
                    println!("{} Aborted.", "✗".red());
                    return Ok(());
                }
                }
            }
        }

        if reset_retry_context {
            retry_attempt = 0;
        } else {
            retry_attempt += 1;
        }
        let previous_output = if reset_retry_context {
            None
        } else {
            Some(raw_output.as_str())
        };

        raw_output = generate_plan(
            client,
            prompt,
            reasoning_effort,
            retry_attempt,
            previous_output,
            retry_note.as_deref(),
            conventions,
            long_commits,
        )
        .await?;
    }
}

fn enforce_commit_message_mode(steps: &mut [ExecutionStep], long_commits: bool) {
    if long_commits {
        return;
    }
    for step in steps {
        if let ExecutionStep::CommitGroup(group) = step {
            group.body = None;
        }
    }
}

async fn generate_plan(
    client: &api::ApiClient,
    prompt: &str,
    reasoning_effort: api::ReasoningEffort,
    retry_attempt: usize,
    previous_output: Option<&str>,
    retry_note: Option<&str>,
    conventions: Option<&conventions::CommitConventions>,
    long_commits: bool,
) -> Result<String> {
    let message = if retry_attempt == 0 {
        "Asking Mercury to analyze your changes..."
    } else {
        "Retrying commit plan generation..."
    };
    let spinner = spinner(message);
    let result = client
        .generate_commits(
            prompt,
            &api::GenerationOptions {
                reasoning_effort,
                retry_attempt,
                previous_output: previous_output.map(str::to_owned),
                retry_note: retry_note.map(str::to_owned),
                long_commits,
            },
            conventions,
        )
        .await;
    spinner.finish_and_clear();
    result
}

fn execute_or_retry(
    args: &Args,
    steps: &[ExecutionStep],
    ctx: &git::GitContext,
    long_commits: bool,
) -> Result<PlanAction> {
    let repo_root = &ctx.repo_root;
    let commit_count = steps
        .iter()
        .filter(|s| matches!(s, ExecutionStep::CommitGroup(_)))
        .count();

    // ── Display plan ─────────────────────────────────────────────────────────
    println!(
        "{} {} step{} planned ({} commit{}):",
        "●".cyan().bold(),
        steps.len(),
        if steps.len() == 1 { "" } else { "s" },
        commit_count,
        if commit_count == 1 { "" } else { "s" }
    );
    println!();

    for (i, step) in steps.iter().enumerate() {
        let num = format!("[{}/{}]", i + 1, steps.len()).dimmed();

        match step {
            ExecutionStep::Reset(cmd) => {
                println!("  {} {} Unstage files:", num, "↺".yellow().bold());
                println!("      {}", cmd.raw.dimmed());
            }
            ExecutionStep::CommitGroup(group) => {
                println!("  {} Commit:", num);
                println!("      {}", group.message.green().bold());
                if let Some(ref body) = group.body {
                    for line in body.lines().filter(|line| !line.trim().is_empty()).take(5) {
                        println!("      {}", line.dimmed());
                    }
                }
                if !group.hunk_ids.is_empty() {
                    for hunk_id in &group.hunk_ids {
                        println!("      {} {}", "~".cyan(), hunk_id.dimmed());
                    }
                }
                for cmd in &group.add_commands {
                    match &cmd.kind {
                        parser::CommandKind::Add { .. } => println!("      {}", "git add …".dimmed()),
                        _ => println!("      {}", cmd.raw.dimmed()),
                    }
                }
                for file in &group.files {
                    println!("      {} {}", "+".cyan(), file.dimmed());
                }
                if !group.commit_command.raw.trim().is_empty() {
                    println!("      {}", group.commit_command.raw.dimmed());
                }
            }
        }
        if i + 1 < steps.len() {
            println!();
        }
    }

    // ── Confirm ──────────────────────────────────────────────────────────────
    if !args.yes {
        match prompt_for_plan_action(long_commits)? {
            PlanAction::Execute => {}
            PlanAction::Retry { note } => return Ok(PlanAction::Retry { note }),
            PlanAction::ToggleLongCommits => return Ok(PlanAction::ToggleLongCommits),
            PlanAction::Abort => return Ok(PlanAction::Abort),
        }
    }

    // ── Execute ─────────────────────────────────────────────────────────────
    for (i, step) in steps.iter().enumerate() {
        let label = format!("[{}/{}]", i + 1, steps.len());

        match step {
            ExecutionStep::Reset(cmd) => {
                let args_vec: Vec<&str> = cmd.raw.split_whitespace().skip(1).collect();
                run_git_command(repo_root, &args_vec, &label)?;
                println!("  {} {} Unstage", "↺".yellow().bold(), label.dimmed());
            }
            ExecutionStep::CommitGroup(group) => {
                // Handle hunk-based staging
                if !group.hunk_ids.is_empty() {
                    stage_hunks(repo_root, &group.hunk_ids, ctx)?;
                } else {
                    // Handle file-based staging (existing logic)
                    for add_cmd in &group.add_commands {
                        if let parser::CommandKind::Add { paths } = &add_cmd.kind {
                            let mut args_vec = vec!["add"];
                            args_vec.extend(paths.iter().map(|s| s.as_str()));
                            run_git_command(repo_root, &args_vec, &label)?;
                        }
                    }

                    if group.add_commands.is_empty() && !group.files.is_empty() {
                        let mut add_args = vec!["add"];
                        let file_refs: Vec<&str> = group.files.iter().map(|s| s.as_str()).collect();
                        add_args.extend(file_refs);
                        run_git_command(repo_root, &add_args, &label)?;
                    }
                }

                if let Some(ref body) = group.body {
                    run_git_command(
                        repo_root,
                        &["commit", "-m", &group.message, "-m", body],
                        &label,
                    )?;
                } else {
                    run_git_command(repo_root, &["commit", "-m", &group.message], &label)?;
                }

                if let Some(ref body) = group.body {
                    println!(
                        "  {} {} {}",
                        "✓".green().bold(),
                        label.dimmed(),
                        group.message.bold()
                    );
                    for line in body.lines().filter(|line| !line.trim().is_empty()).take(3) {
                        println!("      {}", line.dimmed());
                    }
                } else {
                    println!(
                        "  {} {} {}",
                        "✓".green().bold(),
                        label.dimmed(),
                        group.message.bold()
                    );
                }
            }
        }
    }

    println!();
    println!(
        "{} All done! {} commit{} created.",
        "✓".green().bold(),
        commit_count,
        if commit_count == 1 { "" } else { "s" }
    );

    Ok(PlanAction::Execute)
}

fn run_git_command(repo_root: &str, args: &[&str], label: &str) -> Result<()> {
    let status = Command::new("git")
        .args(["-C", repo_root])
        .args(args)
        .status()
        .with_context(|| format!("Failed to run: git {}", args.join(" ")))?;

    if !status.success() {
        anyhow::bail!(
            "{} git {} failed with exit code {:?}",
            label,
            args.join(" "),
            status.code()
        );
    }

    Ok(())
}

fn stage_hunks(repo_root: &str, hunk_ids: &[String], ctx: &git::GitContext) -> Result<()> {
    // Reset index to HEAD first
    run_git_command(repo_root, &["reset", "HEAD"], "[stage]")?;

    // Build partial patch from selected hunks
    let partial_patch = ctx.build_partial_patch(hunk_ids);

    if partial_patch.is_empty() {
        anyhow::bail!("No hunks found for the specified IDs");
    }

    // Apply patch to index only (--cached)
    // We need to use stdin to pass the patch
    let mut child = Command::new("git")
        .args(["-C", repo_root, "apply", "--cached", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("Failed to spawn git apply --cached")?;

    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(partial_patch.as_bytes())?;
    }

    let status = child
        .wait()
        .context("Failed to wait for git apply --cached")?;

    if !status.success() {
        anyhow::bail!(
            "git apply --cached failed with exit code {:?}",
            status.code()
        );
    }

    Ok(())
}

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

enum PlanAction {
    Execute,
    Retry { note: Option<String> },
    ToggleLongCommits,
    Abort,
}

fn prompt_for_plan_action(long_commits: bool) -> Result<PlanAction> {
    let mut prompt = ActionPrompt::new("Ready to commit?".to_string(), vec![], long_commits);

    let result = run_action_prompt(&mut prompt)?;

    match result {
        ActionResult::Accept => Ok(PlanAction::Execute),
        ActionResult::Retry { note } => {
            let note = note.trim();
            let note = if note.is_empty() {
                None
            } else {
                Some(note.to_string())
            };
            Ok(PlanAction::Retry { note })
        }
        ActionResult::ToggleLongCommits => Ok(PlanAction::ToggleLongCommits),
        ActionResult::Abort => Ok(PlanAction::Abort),
    }
}

fn run_action_prompt(prompt: &mut ActionPrompt) -> Result<ActionResult> {
    // Get current cursor position and create dedicated prompt space if needed.
    let cursor_pos = crossterm::cursor::position().unwrap_or((0, 0));
    let terminal_size = crossterm::terminal::size().unwrap_or((80, 24));

    let prompt_height = prompt.desired_height(terminal_size.0);
    let available_space = terminal_size.1.saturating_sub(cursor_pos.1);
    let start_row = if available_space < prompt_height {
        // Scroll just enough so the prompt fits without creating extra vertical gap.
        // Printing (prompt_height - 1) newlines from any row yields exactly the
        // required scroll amount when space is insufficient.
        let newlines = prompt_height.saturating_sub(1);
        for _ in 0..newlines {
            print!("\r\n");
        }
        io::stdout().flush()?;
        terminal_size.1.saturating_sub(prompt_height)
    } else {
        cursor_pos.1
    };

    enable_raw_mode()?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_action_loop(&mut terminal, prompt, start_row);
    disable_raw_mode()?;

    // Clear the prompt area
    let height = prompt.desired_height(terminal.size()?.width);
    clear_prompt_area(start_row, height)?;

    Ok(result)
}

fn run_action_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    prompt: &mut ActionPrompt,
    start_row: u16,
) -> ActionResult {
    use crossterm::event::{self, Event, KeyEventKind};

    loop {
        let size = terminal.size().unwrap_or_else(|_| Size::new(80, 24));
        let height = prompt.desired_height(size.width);
        let effective_start_row = start_row.min(size.height.saturating_sub(height));
        let area = Rect::new(
            0,
            effective_start_row,
            size.width,
            height.min(size.height.saturating_sub(effective_start_row)),
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
