use anyhow::{Context, Result};
use std::process::Command;

/// All git context we gather to pass to the model
#[derive(Debug, Default)]
pub struct GitContext {
    pub repo_root: String,
    pub current_branch: String,
    pub recent_commits: Vec<CommitInfo>,
    pub staged_diff: String,
    pub unstaged_diff: String,
    pub untracked_files: Vec<String>,
    pub staged_files: Vec<StagedFile>,
}

#[derive(Debug)]
pub struct CommitInfo {
    pub hash: String,
    pub author: String,
    pub message: String,
}

#[derive(Debug)]
pub struct StagedFile {
    pub status: char,
    pub path: String,
}

impl GitContext {
    /// Gather all relevant git information from the repo at `path`.
    pub fn gather(repo_path: &str) -> Result<Self> {
        let root = get_repo_root(repo_path)?;

        let current_branch = get_branch(&root)?;
        let recent_commits = get_recent_commits(&root, 10)?;
        // Use --no-ext-diff to bypass external diff tool (like difft)
        let staged_diff = run_git(
            &root,
            &[
                "--no-pager",
                "diff",
                "--no-ext-diff",
                "--cached",
                "--stat",
                "-p",
                "--no-color",
            ],
        )?;
        let unstaged_diff = run_git(
            &root,
            &[
                "--no-pager",
                "diff",
                "--no-ext-diff",
                "--stat",
                "-p",
                "--no-color",
            ],
        )?;
        let (staged_files, untracked_files) = get_status(&root)?;

        Ok(GitContext {
            repo_root: root,
            current_branch,
            recent_commits,
            staged_diff,
            unstaged_diff,
            untracked_files,
            staged_files,
        })
    }

    /// Calculate the total volume of changes (staged + unstaged diffs).
    /// Returns the total number of lines in both diffs.
    pub fn diff_volume(&self) -> usize {
        let staged_lines = self.staged_diff.lines().count();
        let unstaged_lines = self.unstaged_diff.lines().count();
        staged_lines + unstaged_lines
    }

    /// Format the context into a prompt-friendly string.
    pub fn to_prompt(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!("Branch: {}\n\n", self.current_branch));

        if !self.recent_commits.is_empty() {
            out.push_str("Recent commits (for style reference):\n");
            for c in &self.recent_commits {
                out.push_str(&format!("  {} {} {}\n", c.hash, c.author, c.message));
            }
            out.push('\n');
        }

        if !self.staged_files.is_empty() {
            out.push_str("Staged files:\n");
            for f in &self.staged_files {
                out.push_str(&format!("  {} {}\n", f.status, f.path));
            }
            out.push('\n');
        }

        if !self.untracked_files.is_empty() {
            out.push_str("Untracked files:\n");
            for f in &self.untracked_files {
                out.push_str(&format!("  {}\n", f));
            }
            out.push('\n');
        }

        if !self.staged_diff.is_empty() {
            out.push_str("Staged diff:\n```diff\n");
            let pages = chunk_diff_by_file(&self.staged_diff, 32000);
            for (i, page) in pages.iter().enumerate() {
                if pages.len() > 1 {
                    out.push_str(&format!("=== DIFF PAGE {}/{} ===\n\n", i + 1, pages.len()));
                }
                out.push_str(page);
                out.push_str("\n");
            }
            out.push_str("```\n\n");
        }

        if !self.unstaged_diff.is_empty() {
            out.push_str("Unstaged diff (for context):\n```diff\n");
            let pages = chunk_diff_by_file(&self.unstaged_diff, 32000);
            for (i, page) in pages.iter().enumerate() {
                if pages.len() > 1 {
                    out.push_str(&format!("=== DIFF PAGE {}/{} ===\n\n", i + 1, pages.len()));
                }
                out.push_str(page);
                out.push_str("\n");
            }
            out.push_str("```\n\n");
        }

        out
    }

    pub fn has_changes(&self) -> bool {
        !self.staged_files.is_empty()
            || !self.untracked_files.is_empty()
            || !self.staged_diff.is_empty()
            || !self.unstaged_diff.is_empty()
    }
}

fn get_repo_root(path: &str) -> Result<String> {
    let out = Command::new("git")
        .args(["-C", path, "rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to run git")?;

    if !out.status.success() {
        anyhow::bail!(
            "Not inside a git repository: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn get_branch(root: &str) -> Result<String> {
    let out = run_git(root, &["branch", "--show-current"])?;
    let branch = out.trim().to_string();
    if branch.is_empty() {
        // Detached HEAD — show short SHA
        Ok(run_git(root, &["rev-parse", "--short", "HEAD"])?
            .trim()
            .to_string())
    } else {
        Ok(branch)
    }
}

fn get_recent_commits(root: &str, n: usize) -> Result<Vec<CommitInfo>> {
    let fmt = "%h|%an|%s";
    let count = format!("-{}", n);
    let out = run_git(root, &["log", &count, &format!("--pretty=format:{}", fmt)])?;

    let commits = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            CommitInfo {
                hash: parts.first().unwrap_or(&"").to_string(),
                author: parts.get(1).unwrap_or(&"").to_string(),
                message: parts.get(2).unwrap_or(&"").to_string(),
            }
        })
        .collect();

    Ok(commits)
}

fn get_status(root: &str) -> Result<(Vec<StagedFile>, Vec<String>)> {
    let out = run_git(root, &["status", "--porcelain", "-z"])?;
    let mut staged = Vec::new();
    let mut untracked = Vec::new();

    // --porcelain -z: entries separated by NUL, each is "XY path"
    for entry in out.split('\0') {
        if entry.len() < 3 {
            continue;
        }
        let xy = entry.as_bytes();
        let index_status = xy[0] as char;
        let work_status = xy[1] as char;
        let path = entry[3..].to_string();

        if path.is_empty() {
            continue;
        }

        if index_status == '?' && work_status == '?' {
            untracked.push(path);
        } else if index_status != ' ' && index_status != '?' {
            staged.push(StagedFile {
                status: index_status,
                path: path.clone(),
            });
        }
    }

    Ok((staged, untracked))
}

fn run_git(root: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(["-C", root])
        .args(args)
        .output()
        .context("Failed to run git")?;

    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Truncate a diff to approximately `max_chars` characters, preserving structure.
fn truncate_diff(diff: &str, max_chars: usize) -> String {
    if diff.chars().count() <= max_chars {
        return diff.to_string();
    }
    let truncated: String = diff.chars().take(max_chars).collect();
    format!(
        "{}\n\n... [diff truncated at {} chars, {} total] ...",
        truncated,
        max_chars,
        diff.len()
    )
}

/// Split a diff into pages, keeping each file's diff intact.
/// Returns a vector of diff pages that each fit within max_chars.
fn chunk_diff_by_file(diff: &str, max_chars: usize) -> Vec<String> {
    if diff.chars().count() <= max_chars {
        return vec![diff.to_string()];
    }

    let mut pages = Vec::new();
    let mut current_page = String::new();

    for line in diff.lines() {
        let line_len = line.chars().count();

        // Check if adding this line would exceed the limit
        if current_page.chars().count() + line_len > max_chars {
            // If we have content, save current page
            if !current_page.is_empty() {
                pages.push(current_page.clone());
                current_page.clear();
            }
        }

        // If this is a file header, prefer to start a new page if current is half full
        if line.starts_with("diff --git") || line.starts_with("--- ") || line.starts_with("+++ ") {
            if !current_page.is_empty() && current_page.chars().count() > max_chars / 2 {
                pages.push(current_page.clone());
                current_page.clear();
            }
        }

        current_page.push_str(line);
        current_page.push('\n');
    }

    // Push remaining content
    if !current_page.is_empty() {
        pages.push(current_page);
    }

    // Edge case: if everything is in one huge file, just return truncated
    if pages.is_empty() {
        return vec![truncate_diff(diff, max_chars)];
    }

    pages
}
