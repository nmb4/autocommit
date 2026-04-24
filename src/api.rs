use anyhow::{Context, Result};
use crate::conventions::CommitConventions;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.inceptionlabs.ai/v1";
const MODEL: &str = "mercury-2";

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    #[serde(rename = "instant")]
    Instant,
    Low,
    High,
}

impl Default for ReasoningEffort {
    fn default() -> Self {
        ReasoningEffort::Instant
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

pub struct ApiClient {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

#[derive(Debug, Clone)]
pub struct GenerationOptions {
    pub reasoning_effort: ReasoningEffort,
    pub retry_attempt: usize,
    pub previous_output: Option<String>,
    pub long_commits: bool,
}

impl ApiClient {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        ApiClient {
            client: reqwest::Client::new(),
            api_key,
            base_url: base_url.unwrap_or_else(|| BASE_URL.to_string()),
            model: model.unwrap_or_else(|| MODEL.to_string()),
        }
    }

    pub async fn generate_commits(
        &self,
        git_context: &str,
        options: &GenerationOptions,
        conventions: Option<&CommitConventions>,
    ) -> Result<String> {
        let system_prompt = Self::build_system_prompt(conventions, options.long_commits);

        let mut user_message = format!(
            "Here is the current git repository state:\n\n{}\n\nPlease analyze this and generate the appropriate git commands.",
            git_context
        );

        if options.retry_attempt > 0 {
            user_message.push_str(&format!(
                "\n\nThis is retry attempt {}. Produce a different commit plan than the previous attempt while still matching the repository state.",
                options.retry_attempt
            ));
            if let Some(previous_output) = &options.previous_output {
                user_message.push_str(&format!(
                    "\n\nPrevious attempt output:\n```shell\n{}\n```",
                    previous_output.trim()
                ));
            }
        }

        let reason_effort_str = match options.reasoning_effort {
            ReasoningEffort::Instant => None,
            ReasoningEffort::Low => Some("low".to_string()),
            ReasoningEffort::High => Some("high".to_string()),
        };

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: system_prompt,
                },
                Message {
                    role: "user".to_string(),
                    content: user_message,
                },
            ],
            max_tokens: 2048,
            temperature: 0.2,
            reasoning_effort: reason_effort_str,
        };

        let url = format!("{}/chat/completions", self.base_url);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Inception API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("API error {}: {}", status, body);
        }

        let chat: ChatResponse = resp.json().await.context("Failed to parse API response")?;

        chat.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .context("Empty response from API")
    }
}

const BASE_SYSTEM_PROMPT: &str = r#"
You are an expert at writing meaningful, well-structured git commit messages.

Given information about the current state of a git repository (branch, recent commits, staged/unstaged changes, diffs), your job is to produce a series of git commands that will stage, unstage, and commit the changes meaningfully.

RULES:
1. Analyze what changed and group related changes into logical commits.
2. Write commit messages following the Conventional Commits spec (feat, fix, refactor, docs, chore, test, style, perf, ci, build, etc.).
3. Commit messages should be concise, imperative mood, and describe the "why" not just "what".
4. If the changes are cohesive and belong together, produce a single commit.
5. If changes are clearly distinct concerns, split them into multiple commits.
6. Use `git reset` to unstage files when you need to reorganize commits. For example, if files A and B are staged but should go in separate commits, first use `git reset` to unstage, then `git add` for each commit individually.
7. Each commit block must include `git add` commands for specific files followed by a `git commit` command.
8. NEVER produce a commit without a message - every commit MUST have a meaningful `-m "message"` argument.
9. NEVER leave files staged without a corresponding commit command.
10. Never use `git add .` or `git add -A` unless all changes genuinely belong to one commit.
11. Prefer specific file paths when grouping makes sense.

IMPORTANT: Never leave any files staged without committing them.

OUTPUT FORMAT:
Output ONLY a fenced code block tagged with `shell`, containing valid shell commands. No explanation before or after. No markdown outside the code block. No comments inside the commands. Just the raw commands.

Example output with reset:
```shell
git reset src/mixed_file.rs
git add src/auth/login.rs
git commit -m "feat(auth): add session-based login"
git add src/utils/config.rs
git commit -m "chore(config): add default settings"
```

Example output without reset:
```shell
git add src/auth/login.rs src/auth/session.rs
git commit -m "feat(auth): add session-based login flow"
git add docs/README.md
git commit -m "docs: update README with auth setup instructions"
```

If there is nothing to commit (no staged or unstaged changes, no untracked files), output:
```shell
# nothing to commit
```
"#;

impl ApiClient {
    fn build_system_prompt(conventions: Option<&CommitConventions>, long_commits: bool) -> String {
        let mut prompt = BASE_SYSTEM_PROMPT.trim().to_string();

        if long_commits {
            prompt.push_str("\n\n");
            prompt.push_str("LONG COMMIT MESSAGES:\n");
            prompt.push_str("Each commit message MUST have a body (multiline). Format:\n");
            prompt.push_str("```\n");
            prompt.push_str("git commit -m \"type(scope): short summary\" -m \"Detailed explanation of the change, including:\n");
            prompt.push_str("- What changed and why\n");
            prompt.push_str("- Any context a future reader would need\n");
            prompt.push_str("- Breaking changes or migration notes if applicable\"\n");
            prompt.push_str("```\n");
        }

        if let Some(conv) = conventions {
            let conventions_text = conv.to_prompt_fragment();
            if !conventions_text.trim().is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str("PROJECT-SPECIFIC CONVENTIONS:\n\n");
                prompt.push_str(&conventions_text);
            }
        }

        prompt
    }
}
