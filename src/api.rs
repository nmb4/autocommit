use anyhow::{Context, Result};
use crate::conventions::CommitConventions;
use serde::Serialize;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const INCEPTION_BASE_URL: &str = "https://api.inceptionlabs.ai/v1";
const INCEPTION_MODEL: &str = "mercury-2";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_MODEL: &str = "inclusionai/ling-2.6-1t:free";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Inception,
    OpenRouter,
}

impl Provider {
    fn default_base_url(self) -> &'static str {
        match self {
            Provider::Inception => INCEPTION_BASE_URL,
            Provider::OpenRouter => OPENROUTER_BASE_URL,
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Provider::Inception => INCEPTION_MODEL,
            Provider::OpenRouter => OPENROUTER_MODEL,
        }
    }
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
}

#[derive(Debug, Serialize)]
struct ReasoningConfig {
    effort: String,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

pub struct ApiClient {
    client: reqwest::Client,
    provider: Provider,
    api_key: String,
    base_url: String,
    model: String,
    temperature: f32,
    debug_log_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct GenerationOptions {
    pub reasoning_effort: ReasoningEffort,
    pub retry_attempt: usize,
    pub previous_output: Option<String>,
    pub retry_note: Option<String>,
    pub long_commits: bool,
}

impl ApiClient {
    pub fn new(
        provider: Provider,
        inception_api_key: Option<String>,
        openrouter_api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
        temperature: f32,
        debug_log_path: Option<PathBuf>,
    ) -> Result<Self> {
        let api_key = match provider {
            Provider::Inception => inception_api_key
                .context("Missing Inception API key (set INCEPTION_API_KEY or pass --api-key)")?,
            Provider::OpenRouter => openrouter_api_key
                .context("Missing OpenRouter API key (set AC_OR_KEY or pass --or-key)")?,
        };

        Ok(ApiClient {
            client: reqwest::Client::new(),
            provider,
            api_key,
            base_url: base_url
                .unwrap_or_else(|| provider.default_base_url().to_string()),
            model: model.unwrap_or_else(|| provider.default_model().to_string()),
            temperature,
            debug_log_path,
        })
    }

    pub fn format_model_name(&self) -> String {
        format_model_name(&self.model)
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
                "\n\nThis is retry attempt {}. Regenerate the plan for the same repository state. Keep grouping and scope stable unless the retry instruction requires changes or you detect a concrete issue in the previous attempt.",
                options.retry_attempt
            ));
            if let Some(previous_output) = &options.previous_output {
                let previous_shell = strip_outer_code_fence(previous_output.trim());
                user_message.push_str(&format!(
                    "\n\nPrevious attempt output:\n```shell\n{}\n```",
                    previous_shell
                ));
            }
        }
        if let Some(retry_note) = options.retry_note.as_deref() {
            let retry_note = retry_note.trim();
            if !retry_note.is_empty() {
                user_message.push_str(&format!(
                    "\n\nUser retry instruction (highest priority):\n{}\nHonor this instruction in your next output.",
                    retry_note
                ));
            }
        }

        let effort = match options.reasoning_effort {
            ReasoningEffort::Instant => None,
            ReasoningEffort::Low => Some("low"),
            ReasoningEffort::High => Some("high"),
        };

        let (reasoning_effort, reasoning) = match self.provider {
            Provider::Inception => (effort.map(str::to_string), None),
            Provider::OpenRouter => (
                None,
                effort.map(|e| ReasoningConfig {
                    effort: e.to_string(),
                }),
            ),
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
            temperature: self.temperature,
            reasoning_effort,
            reasoning,
        };

        if let Err(e) = self.log_request(options, &request) {
            eprintln!("warning: failed to write debug request log: {e}");
        }

        let url = format!("{}/chat/completions", self.base_url);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context("Failed to send request to provider API")?;

        let status = resp.status();
        let response_body = resp
            .text()
            .await
            .context("Failed to read provider response body")?;

        if let Err(e) = self.log_response(options, status.as_u16(), &response_body) {
            eprintln!("warning: failed to write debug response log: {e}");
        }

        if !status.is_success() {
            anyhow::bail!("API error {}: {}", status, response_body);
        }

        let json: Value = serde_json::from_str(&response_body)
            .context("Failed to parse API response")?;
        extract_text_from_chat_response(&json).ok_or_else(|| {
            let snippet = serde_json::to_string(&json)
                .map(|s| truncate_for_error(&s, 800))
                .unwrap_or_else(|_| "<unprintable json>".to_string());
            anyhow::anyhow!("Empty textual response from API. Response snippet: {}", snippet)
        })
    }

    fn log_request(&self, options: &GenerationOptions, request: &ChatRequest) -> Result<()> {
        let Some(path) = &self.debug_log_path else {
            return Ok(());
        };

        let (system_prompt, user_context) = match request.messages.as_slice() {
            [sys, user, ..] => (sys.content.clone(), user.content.clone()),
            _ => (String::new(), String::new()),
        };

        let entry = serde_json::json!({
            "kind": "request",
            "ts_unix_ms": unix_ms_now(),
            "provider": match self.provider {
                Provider::Inception => "inception",
                Provider::OpenRouter => "openrouter",
            },
            "base_url": self.base_url,
            "model": self.model,
            "retry_attempt": options.retry_attempt,
            "retry_note": options.retry_note,
            "reasoning_effort": format!("{:?}", options.reasoning_effort).to_lowercase(),
            "long_commits": options.long_commits,
            "system_prompt": system_prompt,
            "user_context": user_context,
            "request_payload": request,
        });

        append_jsonl(path, &entry)
    }

    fn log_response(
        &self,
        options: &GenerationOptions,
        status_code: u16,
        response_body: &str,
    ) -> Result<()> {
        let Some(path) = &self.debug_log_path else {
            return Ok(());
        };

        let entry = serde_json::json!({
            "kind": "response",
            "ts_unix_ms": unix_ms_now(),
            "provider": match self.provider {
                Provider::Inception => "inception",
                Provider::OpenRouter => "openrouter",
            },
            "model": self.model,
            "retry_attempt": options.retry_attempt,
            "status_code": status_code,
            "response_body": response_body,
        });

        append_jsonl(path, &entry)
    }
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn append_jsonl(path: &PathBuf, value: &Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Unable to open debug log file: {}", path.display()))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn extract_text_from_chat_response(json: &Value) -> Option<String> {
    let choice0 = json.get("choices")?.as_array()?.first()?;

    if let Some(content) = choice0
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(extract_text_from_content_value)
    {
        if !content.trim().is_empty() {
            return Some(content);
        }
    }

    if let Some(text) = choice0.get("text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return Some(text.to_string());
        }
    }

    if let Some(refusal) = choice0
        .get("message")
        .and_then(|m| m.get("refusal"))
        .and_then(Value::as_str)
    {
        if !refusal.trim().is_empty() {
            return Some(refusal.to_string());
        }
    }

    if let Some(reasoning) = choice0
        .get("message")
        .and_then(|m| m.get("reasoning"))
        .and_then(Value::as_str)
    {
        if !reasoning.trim().is_empty() {
            return Some(reasoning.to_string());
        }
    }

    if let Some(args) = choice0
        .get("message")
        .and_then(|m| m.get("tool_calls"))
        .and_then(Value::as_array)
        .and_then(|calls| calls.first())
        .and_then(|call| call.get("function"))
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
    {
        if !args.trim().is_empty() {
            return Some(args.to_string());
        }
    }

    if let Some(output_text) = json
        .get("output")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("content"))
        .and_then(extract_text_from_content_value)
    {
        if !output_text.trim().is_empty() {
            return Some(output_text);
        }
    }

    if let Some(msg) = choice0.get("message") {
        if let Some(fallback) = collect_text_fragments(msg) {
            if !fallback.trim().is_empty() {
                return Some(fallback);
            }
        }
    }

    None
}

fn extract_text_from_content_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                } else if let Some(t) = part.get("content").and_then(Value::as_str) {
                    out.push_str(t);
                } else if let Some(t) = part.get("output_text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
        _ => None,
    }
}

fn collect_text_fragments(v: &Value) -> Option<String> {
    let mut out = String::new();
    collect_text_fragments_into(v, &mut out);
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn collect_text_fragments_into(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            if !s.trim().is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(s);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_text_fragments_into(item, out);
            }
        }
        Value::Object(map) => {
            for (k, val) in map {
                if matches!(k.as_str(), "text" | "content" | "output_text" | "reasoning" | "refusal")
                {
                    collect_text_fragments_into(val, out);
                }
            }
        }
        _ => {}
    }
}

fn truncate_for_error(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{}… [truncated]", head)
}

fn strip_outer_code_fence(s: &str) -> &str {
    let t = s.trim();
    let starts_with_fence = t.starts_with("```");
    let ends_with_fence = t.ends_with("```");
    if !(starts_with_fence && ends_with_fence) {
        return t;
    }
    let after_open = match t.find('\n') {
        Some(idx) => &t[idx + 1..],
        None => return t,
    };
    match after_open.rfind("```") {
        Some(end_idx) => after_open[..end_idx].trim(),
        None => t,
    }
}

fn format_model_name(model: &str) -> String {
    model
        .split('/')
        .last()
        .unwrap_or(model)
        .replace('-', " ")
        .split(' ')
        .map(|word| {
            let chars: Vec<char> = word.chars().collect();
            if chars.is_empty() {
                String::new()
            } else {
                let first = chars[0].to_uppercase().to_string();
                let rest: String = chars[1..].iter().collect();
                format!("{}{}", first, rest)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
12. Ground every commit summary/body strictly in the provided repository context and diffs.
13. Do not invent implementation details, architecture claims, or motivations that are not directly supported by the provided context.
14. If context is limited, keep commit messages factual and minimal instead of speculative.
15. Prefer stable output: for the same context, keep commit grouping and message intent consistent.
16. Only change grouping on retry when user retry instructions request it or when correcting a clear mistake.

IMPORTANT: Never leave any files staged without committing them.
IMPORTANT: Always follow explicit user retry instructions when they are present.

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
# reason: <one concise sentence explaining why there is nothing to commit>
```

If you choose "nothing to commit", the reason line is REQUIRED.
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
            prompt.push_str("Only include details that are directly supported by provided diffs/context.\n");
        } else {
            prompt.push_str("\n\n");
            prompt.push_str("SHORT COMMIT MESSAGES:\n");
            prompt.push_str("Use exactly one `-m \"type(scope): summary\"` per commit.\n");
            prompt.push_str("Do NOT include a second `-m` body message in short mode.\n");
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
