use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.inceptionlabs.ai/v1";
const MODEL: &str = "mercury-2";

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    temperature: f32,
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

impl ApiClient {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        ApiClient {
            client: reqwest::Client::new(),
            api_key,
            base_url: base_url.unwrap_or_else(|| BASE_URL.to_string()),
            model: model.unwrap_or_else(|| MODEL.to_string()),
        }
    }

    pub async fn generate_commits(&self, git_context: &str) -> Result<String> {
        let system_prompt = SYSTEM_PROMPT.trim().to_string();

        let user_message = format!(
            "Here is the current git repository state:\n\n{}\n\nPlease analyze this and generate the appropriate git commands.",
            git_context
        );

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

        let chat: ChatResponse = resp
            .json()
            .await
            .context("Failed to parse API response")?;

        chat.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .context("Empty response from API")
    }
}

const SYSTEM_PROMPT: &str = r#"
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
8. Never use `git add .` or `git add -A` unless all changes genuinely belong to one commit.
9. Prefer specific file paths when grouping makes sense.

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
