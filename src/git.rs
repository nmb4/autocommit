use anyhow::{Context, Result};
use crate::conventions::CommitConventions;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

/// All git context we gather to pass to the model
#[derive(Debug, Default)]
pub struct GitContext {
    pub repo_root: String,
    pub current_branch: String,
    pub recent_commits: Vec<CommitInfo>,
    pub prefer_long_commits: bool,
    pub staged_diff: String,
    pub unstaged_diff: String,
    pub untracked_diff: String,
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

#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub id: String,
    pub file: String,
    #[allow(dead_code)]
    hunk_header: String,
    pub content: String,
}

impl GitContext {
    /// Gather all relevant git information from the repo at `path`.
    pub fn gather(repo_path: &str) -> Result<Self> {
        let root = get_repo_root(repo_path)?;

        let current_branch = get_branch(&root)?;
        let recent_commits = get_recent_commits(&root, 10)?;
        let prefer_long_commits = infer_long_commit_preference(&root, 20)?;
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
        let untracked_diff = build_untracked_diff(&root, &untracked_files)?;

        Ok(GitContext {
            repo_root: root,
            current_branch,
            recent_commits,
            prefer_long_commits,
            staged_diff,
            unstaged_diff,
            untracked_diff,
            untracked_files,
            staged_files,
        })
    }

    /// Calculate the total volume of changes (staged + unstaged diffs).
    /// Returns the total number of lines in both diffs.
    pub fn diff_volume(&self) -> usize {
        let staged_lines = self.staged_diff.lines().count();
        let unstaged_lines = self.unstaged_diff.lines().count();
        let untracked_lines = self.untracked_diff.lines().count();
        staged_lines + unstaged_lines + untracked_lines
    }

    /// Parse the unstaged diff into individual hunks with unique IDs.
    pub fn parse_hunks(&self) -> Vec<DiffHunk> {
        parse_diff_hunks(&self.unstaged_diff)
    }

    /// Build a partial patch from specific hunk IDs.
    /// Each hunk_id format: "filename:start-end" (e.g., "src/main.rs:10-15")
    pub fn build_partial_patch(&self, hunk_ids: &[String]) -> String {
        let hunks = self.parse_hunks();
        let hunk_map: HashMap<&str, &DiffHunk> = hunks.iter().map(|h| (h.id.as_str(), h)).collect();

        let mut patch = String::new();
        let mut current_file: Option<&str> = None;

        for hunk_id in hunk_ids {
            if let Some(hunk) = hunk_map.get(hunk_id.as_str()) {
                if current_file != Some(hunk.file.as_str()) {
                    if !patch.is_empty() {
                        patch.push_str("\n");
                    }
                    current_file = Some(&hunk.file);
                }
                patch.push_str(&hunk.content);
                patch.push('\n');
            }
        }

        patch
    }

    /// Format the context into a prompt-friendly string.
    #[allow(dead_code)]
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

        // Parse hunks from unstaged diff for hunk-level selection
        let hunks = self.parse_hunks();
        let hunk_threshold = 5;

        if hunks.len() >= hunk_threshold {
            out.push_str(&format!(
                "Available hunks ({} total - use hunk IDs for precise commits):\n",
                hunks.len()
            ));
            out.push_str(
                "When splitting changes across commits, use hunk IDs to select specific changes.\n",
            );
            out.push_str("Format: `# hunks: path:start..end` (comma or space-separated)\n");
            out.push_str("Example: `# hunks: src/main.rs:10..20, src/main.rs:30..40`\n\n");
            for hunk in &hunks {
                out.push_str(&format!("  [{}]\n", hunk.id));
                for line in hunk.content.lines().take(6) {
                    out.push_str(&format!("    {}\n", line));
                }
                if hunk.content.lines().count() > 6 {
                    out.push_str("    ...\n");
                }
            }
            out.push('\n');
            out.push_str("Unstaged diff:\n```diff\n");
            let pages = chunk_diff_by_file(&self.unstaged_diff, 32000);
            for (i, page) in pages.iter().enumerate() {
                if pages.len() > 1 {
                    out.push_str(&format!("=== DIFF PAGE {}/{} ===\n\n", i + 1, pages.len()));
                }
                out.push_str(page);
                out.push_str("\n");
            }
            out.push_str("```\n\n");
        } else if !self.unstaged_diff.is_empty() {
            out.push_str("Unstaged diff:\n```diff\n");
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

        if !self.untracked_diff.is_empty() {
            out.push_str("Untracked file contents (new files):\n```diff\n");
            let pages = chunk_diff_by_file(&self.untracked_diff, 32000);
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

    /// Format the context into a prompt-friendly string with conventions.
    pub fn to_prompt_with_conventions(&self, _conventions: Option<&CommitConventions>) -> String {
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

        if !self.untracked_diff.is_empty() {
            out.push_str("Untracked file contents (new files):\n```diff\n");
            let pages = chunk_diff_by_file(&self.untracked_diff, 32000);
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

fn build_untracked_diff(root: &str, untracked_files: &[String]) -> Result<String> {
    const MAX_BYTES_PER_FILE: usize = 16 * 1024;
    const MAX_LINES_PER_FILE: usize = 400;

    let mut out = String::new();
    for rel_path in untracked_files {
        let abs_path = Path::new(root).join(rel_path);
        let bytes = match fs::read(&abs_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let is_binary = bytes
            .iter()
            .take(1024)
            .any(|b| *b == 0);

        out.push_str(&format!("diff --git a/{0} b/{0}\n", rel_path));
        out.push_str("new file mode 100644\n");
        out.push_str("index 0000000..0000000\n");
        out.push_str("--- /dev/null\n");
        out.push_str(&format!("+++ b/{}\n", rel_path));

        if is_binary {
            out.push_str("@@ -0,0 +1 @@\n");
            out.push_str("+[binary content omitted]\n\n");
            continue;
        }

        let clipped = &bytes[..bytes.len().min(MAX_BYTES_PER_FILE)];
        let text = String::from_utf8_lossy(clipped);
        let mut line_count = 0usize;
        for line in text.lines() {
            if line_count == 0 {
                // Unknown final length up front; keep hunk header generic for new file.
                out.push_str("@@ -0,0 +1,1 @@\n");
            }
            if line_count >= MAX_LINES_PER_FILE {
                out.push_str("+... [content truncated]\n");
                break;
            }
            out.push('+');
            out.push_str(line);
            out.push('\n');
            line_count += 1;
        }
        if line_count == 0 {
            out.push_str("@@ -0,0 +1,1 @@\n+\n");
        }
        out.push('\n');
    }
    Ok(out)
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

fn infer_long_commit_preference(root: &str, n: usize) -> Result<bool> {
    let count = format!("-{}", n);
    let out = run_git(root, &["log", &count, "--pretty=format:%B%x1f"])?;

    let messages: Vec<&str> = out
        .split('\x1f')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .collect();

    if messages.is_empty() {
        return Ok(false);
    }

    let long_count = messages
        .iter()
        .filter(|msg| {
            let mut lines = msg.lines();
            let _subject = lines.next();
            lines.any(|line| !line.trim().is_empty())
        })
        .count();

    let total = messages.len();
    let threshold = std::cmp::max(2, total / 2 + 1);
    Ok(long_count >= threshold)
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

fn parse_diff_hunks(diff: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current_file = String::new();
    let mut current_hunk_start = 0usize;
    let mut current_hunk_end = 0usize;
    let mut hunk_content = String::new();
    let mut hunk_count_per_file: HashMap<String, usize> = HashMap::new();

    let hunk_re = Regex::new(r"@@ -(\d+),?\d* \+(\d+),?(\d*) @@(.*)").unwrap();

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            if let Some(paths) = line.strip_prefix("diff --git a/") {
                if let Some(end) = paths.find(" b/") {
                    current_file = paths[..end].to_string();
                }
            }
        } else if line.starts_with("@@") {
            if !hunk_content.is_empty() {
                let id = format!(
                    "{}:{}..{}",
                    current_file, current_hunk_start, current_hunk_end
                );
                hunks.push(DiffHunk {
                    id,
                    file: current_file.clone(),
                    hunk_header: hunk_content.lines().next().unwrap_or("").to_string(),
                    content: hunk_content.trim().to_string(),
                });
            }

            hunk_content = String::new();
            *hunk_count_per_file.entry(current_file.clone()).or_insert(0) += 1;

            if let Some(caps) = hunk_re.captures(line) {
                current_hunk_start = caps
                    .get(1)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);
                let end_match = caps
                    .get(3)
                    .map(|m| m.as_str().parse::<usize>().ok())
                    .flatten();
                current_hunk_end = end_match.unwrap_or(current_hunk_start);
            }
        }

        hunk_content.push_str(line);
        hunk_content.push('\n');
    }

    if !hunk_content.is_empty() {
        let id = format!(
            "{}:{}..{}",
            current_file, current_hunk_start, current_hunk_end
        );
        hunks.push(DiffHunk {
            id,
            file: current_file,
            hunk_header: hunk_content.lines().next().unwrap_or("").to_string(),
            content: hunk_content.trim().to_string(),
        });
    }

    hunks
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
