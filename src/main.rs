mod api;
mod git;
mod parser;

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use dialoguer::Confirm;
use indicatif::{ProgressBar, ProgressStyle};
use parser::{CommitGroup, ParseResult};
use std::process::Command;
use std::time::Duration;

/// autocommit — AI-powered git commit message generator
///
/// Analyzes your repository's staged and unstaged changes, then uses the
/// Mercury model via Inception Labs to generate meaningful, conventional
/// commit messages. Shows you the plan and asks for confirmation before
/// executing anything.
#[derive(Debug, Parser)]
#[command(
    name = "autocommit",
    about = "AI-powered git commits using Mercury via Inception Labs",
    long_about = None,
    version
)]
struct Args {
    /// Path to the git repository (defaults to current directory)
    #[arg(short, long, default_value = ".")]
    path: String,

    /// Inception Labs API key (or set INCEPTION_API_KEY env var)
    #[arg(long, env = "INCEPTION_API_KEY", hide_env_values = true)]
    api_key: String,

    /// Override the API base URL
    #[arg(long, env = "INCEPTION_BASE_URL")]
    base_url: Option<String>,

    /// Override the model name (default: mercury-coder)
    #[arg(long, env = "AUTOCOMMIT_MODEL")]
    model: Option<String>,

    /// Skip confirmation prompt and execute immediately
    #[arg(short = 'y', long)]
    yes: bool,

    /// Print the raw model output and exit (do not execute)
    #[arg(long)]
    dry_run: bool,

    /// Show the git context that will be sent to the model and exit
    #[arg(long)]
    show_context: bool,
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

    let prompt = ctx.to_prompt();

    if args.show_context {
        println!("{}", "── Git context sent to model ──".dimmed());
        println!("{}", prompt);
        return Ok(());
    }

    // ── 2. Call the model ────────────────────────────────────────────────────
    let sp2 = spinner("Asking Mercury to analyze your changes...");
    let client = api::ApiClient::new(args.api_key.clone(), args.base_url.clone(), args.model.clone());
    let raw_output = client.generate_commits(&prompt).await?;
    sp2.finish_and_clear();

    if args.dry_run {
        println!("{}", "── Raw model output ──".dimmed());
        println!("{}", raw_output);
        return Ok(());
    }

    // ── 3. Parse model output ────────────────────────────────────────────────
    let parsed = parser::parse_commands(&raw_output)
        .context("Failed to parse model output into git commands")?;

    match parsed {
        ParseResult::NothingToCommit => {
            println!(
                "{} The model determined there is nothing meaningful to commit.",
                "○".yellow()
            );
            return Ok(());
        }
        ParseResult::Commits(groups) => {
            run_commit_plan(&args, &groups, &ctx.repo_root).await?;
        }
    }

    Ok(())
}

async fn run_commit_plan(
    args: &Args,
    groups: &[CommitGroup],
    repo_root: &str,
) -> Result<()> {
    // ── Display plan ─────────────────────────────────────────────────────────
    println!();
    println!(
        "{} {} commit{} planned:",
        "●".cyan().bold(),
        groups.len(),
        if groups.len() == 1 { "" } else { "s" }
    );
    println!();

    for (i, group) in groups.iter().enumerate() {
        let num = format!("[{}/{}]", i + 1, groups.len()).dimmed();
        println!("  {} {}", num, group.message.green().bold());

        if !group.files.is_empty() {
            for file in &group.files {
                println!("      {} {}", "+".cyan(), file.dimmed());
            }
        }

        println!();
        // Show actual commands
        for cmd in &group.add_commands {
            println!("      {}", cmd.raw.dimmed());
        }
        println!("      {}", group.commit_command.raw.dimmed());
        println!();
    }

    // ── Confirm ──────────────────────────────────────────────────────────────
    if !args.yes {
        let confirmed = Confirm::new()
            .with_prompt("Execute these commits?")
            .default(true)
            .interact()
            .context("Prompt failed")?;

        if !confirmed {
            println!("{} Aborted.", "✗".red());
            return Ok(());
        }
    }

    // ── Execute ──────────────────────────────────────────────────────────────
    println!();
    for (i, group) in groups.iter().enumerate() {
        let label = format!("[{}/{}]", i + 1, groups.len());

        // Run all add commands
        for add_cmd in &group.add_commands {
            let args_vec: Vec<&str> = add_cmd.raw.split_whitespace().skip(1).collect(); // skip "git"
            run_git_command(repo_root, &args_vec, &label)?;
        }

        // If no explicit adds but we have files listed, add them
        if group.add_commands.is_empty() && !group.files.is_empty() {
            let mut add_args = vec!["add"];
            let file_refs: Vec<&str> = group.files.iter().map(|s| s.as_str()).collect();
            add_args.extend(file_refs);
            run_git_command(repo_root, &add_args, &label)?;
        }

        // Run commit
        let commit_args: Vec<&str> = group
            .commit_command
            .raw
            .split_whitespace()
            .skip(1)
            .collect();
        run_git_command(repo_root, &commit_args, &label)?;

        println!(
            "  {} {} {}",
            "✓".green().bold(),
            label.dimmed(),
            group.message.bold()
        );
    }

    println!();
    println!(
        "{} All done! {} commit{} created.",
        "✓".green().bold(),
        groups.len(),
        if groups.len() == 1 { "" } else { "s" }
    );

    Ok(())
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
