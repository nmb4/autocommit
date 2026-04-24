use anyhow::Result;

/// A single parsed git command
#[derive(Debug, Clone)]
pub struct GitCommand {
    pub raw: String,
    pub kind: CommandKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandKind {
    Add { paths: Vec<String> },
    AddHunks { hunk_ids: Vec<String> },
    Reset { paths: Vec<String> },
    Commit { message: String, body: Option<String> },
    Comment,
    Other,
}

/// A logical commit group: one or more `git add` commands followed by a `git commit`
#[derive(Debug, Clone)]
pub struct CommitGroup {
    pub files: Vec<String>,
    pub hunk_ids: Vec<String>,
    pub message: String,
    pub body: Option<String>,
    pub add_commands: Vec<GitCommand>,
    pub commit_command: GitCommand,
}

/// A top-level execution step: either a standalone command (like git reset)
/// or a commit group
#[derive(Debug, Clone)]
pub enum ExecutionStep {
    Reset(GitCommand),
    CommitGroup(CommitGroup),
}

/// Parse the model's shell output into structured execution steps.
pub fn parse_commands(output: &str) -> Result<ParseResult> {
    let shell_block = extract_shell_block(output).unwrap_or(output);

    let commands: Vec<GitCommand> = shell_block
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(parse_single_command)
        .collect();

    // Check for "nothing to commit" marker
    let nothing = commands
        .iter()
        .any(|c| matches!(c.kind, CommandKind::Comment));

    if nothing
        && commands
            .iter()
            .all(|c| matches!(c.kind, CommandKind::Comment | CommandKind::Other))
    {
        return Ok(ParseResult::NothingToCommit {
            reason: extract_nothing_to_commit_reason(&commands),
        });
    }

    // Group into execution steps (resets + commit groups)
    let steps = group_into_steps(&commands)?;

    if steps.is_empty() {
        return Ok(ParseResult::NothingToCommit { reason: None });
    }

    Ok(ParseResult::Steps(steps))
}

pub enum ParseResult {
    Steps(Vec<ExecutionStep>),
    NothingToCommit { reason: Option<String> },
}

fn extract_nothing_to_commit_reason(commands: &[GitCommand]) -> Option<String> {
    let mut lines = Vec::new();

    for cmd in commands {
        let raw = cmd.raw.trim();
        if raw.is_empty() {
            continue;
        }

        let normalized = raw.trim_start_matches('#').trim().to_lowercase();
        if normalized == "nothing to commit" || normalized.starts_with("nothing to commit:") {
            continue;
        }

        match cmd.kind {
            CommandKind::Comment => {
                let text = raw.trim_start_matches('#').trim();
                if !text.is_empty() {
                    lines.push(text.to_string());
                }
            }
            CommandKind::Other => {
                lines.push(raw.to_string());
            }
            _ => {}
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" "))
    }
}

fn extract_shell_block(text: &str) -> Option<&str> {
    // Find ```shell ... ``` or ```bash ... ``` or ``` ... ```
    let starts = ["```shell\n", "```bash\n", "```\n"];
    for start in &starts {
        if let Some(begin) = text.find(start) {
            let content_start = begin + start.len();
            if let Some(end) = text[content_start..].find("```") {
                return Some(&text[content_start..content_start + end]);
            }
        }
    }
    None
}

fn parse_single_command(line: &str) -> GitCommand {
    if line.starts_with('#') {
        return GitCommand {
            raw: line.to_string(),
            kind: CommandKind::Comment,
        };
    }

    if let Some(rest) = line.strip_prefix("git add ") {
        let paths = rest
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        return GitCommand {
            raw: line.to_string(),
            kind: CommandKind::Add { paths },
        };
    }

    // git add-hunks <hunk_id> [<hunk_id>...]
    if let Some(rest) = line.strip_prefix("git add-hunks ") {
        let hunk_ids: Vec<String> = rest.split_whitespace().map(|s| s.to_string()).collect();
        return GitCommand {
            raw: line.to_string(),
            kind: CommandKind::AddHunks { hunk_ids },
        };
    }

    // Comment format for hunk selection: # hunks: src/file.rs:10..20, src/file.rs:30..40
    if let Some(rest) = line.strip_prefix("# hunks:") {
        let hunk_ids: Vec<String> = rest
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !hunk_ids.is_empty() {
            return GitCommand {
                raw: line.to_string(),
                kind: CommandKind::AddHunks { hunk_ids },
            };
        }
    }

    // git reset (unstage) - e.g., git reset HEAD file.rb or git reset file.rb
    if let Some(rest) = line.strip_prefix("git reset ") {
        let args: Vec<&str> = rest.split_whitespace().collect();
        let paths: Vec<String> = args
            .iter()
            .skip_while(|s| **s == "HEAD" || s.starts_with('-'))
            .map(|s| s.to_string())
            .collect();
        return GitCommand {
            raw: line.to_string(),
            kind: CommandKind::Reset { paths },
        };
    }

    if let Some(rest) = line.strip_prefix("git commit -m ") {
        let (message, body) = extract_commit_messages(rest.trim());
        return GitCommand {
            raw: line.to_string(),
            kind: CommandKind::Commit { message, body },
        };
    }

    // Handle multi-flag variants like: git commit --message "..."  or  git commit -m "..."
    if line.starts_with("git commit") {
        if let Some((msg, body)) = extract_commit_messages_full(line) {
            return GitCommand {
                raw: line.to_string(),
                kind: CommandKind::Commit { message: msg, body },
            };
        }
    }

    GitCommand {
        raw: line.to_string(),
        kind: CommandKind::Other,
    }
}

fn group_into_steps(commands: &[GitCommand]) -> Result<Vec<ExecutionStep>> {
    let mut steps = Vec::new();
    let mut pending_adds: Vec<GitCommand> = Vec::new();
    let mut pending_files: Vec<String> = Vec::new();
    let mut pending_hunk_ids: Vec<String> = Vec::new();

    for cmd in commands {
        match &cmd.kind {
            CommandKind::Reset { .. } => {
                // Flush any pending adds before the reset
                if !pending_adds.is_empty() {
                    let group =
                        build_commit_group(&pending_adds, "", &pending_files, &pending_hunk_ids, None);
                    steps.push(ExecutionStep::CommitGroup(group));
                    pending_adds.clear();
                    pending_files.clear();
                    pending_hunk_ids.clear();
                }
                steps.push(ExecutionStep::Reset(cmd.clone()));
            }
            CommandKind::Add { paths } => {
                pending_adds.push(cmd.clone());
                pending_files.extend(paths.clone());
            }
            CommandKind::AddHunks { hunk_ids } => {
                pending_adds.push(cmd.clone());
                pending_hunk_ids.extend(hunk_ids.clone());
            }
            CommandKind::Commit { message, body } => {
                if pending_adds.is_empty()
                    && pending_files.is_empty()
                    && pending_hunk_ids.is_empty()
                {
                    // Commit without explicit adds — implies "all staged"
                }

                let group =
                    build_commit_group(&pending_adds, message, &pending_files, &pending_hunk_ids, body.clone());
                steps.push(ExecutionStep::CommitGroup(group));
                pending_adds.clear();
                pending_files.clear();
                pending_hunk_ids.clear();
            }
            CommandKind::Comment | CommandKind::Other => {}
        }
    }

    // Flush any remaining pending adds - let caller detect invalid state
    if !pending_adds.is_empty() {
        // Don't emit warning - let the caller handle retries
    }

    Ok(steps)
}

fn build_commit_group(
    add_commands: &[GitCommand],
    message: &str,
    files: &[String],
    hunk_ids: &[String],
    body: Option<String>,
) -> CommitGroup {
    CommitGroup {
        files: files.to_vec(),
        hunk_ids: hunk_ids.to_vec(),
        message: message.to_string(),
        body,
        add_commands: add_commands.to_vec(),
        commit_command: GitCommand {
            raw: "".to_string(),
            kind: CommandKind::Other,
        },
    }
}

/// Strip surrounding quotes from a string
fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Extract the first message from a `-m "..."` style remainder (handles chained `-m` flags)
fn extract_commit_messages(rest: &str) -> (String, Option<String>) {
    // Split on consecutive -m or --message flags
    let mut parts = Vec::new();
    let mut current = rest.to_string();

    loop {
        let trimmed = current.trim();
        if trimmed.is_empty() {
            break;
        }

        let (msg, remainder) = if let Some(pos) = trimmed.find(" -m ") {
            let msg = unquote(trimmed[..pos].trim());
            let remainder = trimmed[pos + 4..].to_string();
            (msg, remainder)
        } else if let Some(pos) = trimmed.find(" --message ") {
            let msg = unquote(trimmed[..pos].trim());
            let remainder = trimmed[pos + 11..].to_string();
            (msg, remainder)
        } else {
            (unquote(trimmed), String::new())
        };

        parts.push(msg);
        current = remainder;
    }

    let message = parts.first().cloned().unwrap_or_default();
    let body = parts.get(1).cloned();
    (message, body)
}

/// Extract commit message (and optional body) from a full `git commit` line
fn extract_commit_messages_full(line: &str) -> Option<(String, Option<String>)> {
    // Find the first -m or --message flag
    let flag_pos = [" -m ", " --message "]
        .iter()
        .find_map(|f| line.find(f))
        .map(|pos| {
            let flag = if line[pos..].starts_with(" --message ") { " --message " } else { " -m " };
            pos + flag.len()
        })?;

    let rest = &line[flag_pos..];
    Some(extract_commit_messages(rest))
}
