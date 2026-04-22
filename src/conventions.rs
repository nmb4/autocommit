use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Project-specific commit conventions
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CommitConventions {
    pub types: Vec<CommitType>,
    pub scopes: Vec<String>,
    pub examples: Vec<String>,
    pub workflow_rules: Option<WorkflowRules>,
    pub branch_conventions: Option<BranchConventions>,
}

#[derive(Debug, Clone)]
pub struct CommitType {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct WorkflowRules {
    pub file_groupings: Vec<FileGroupingRule>,
    pub always_separate: Vec<String>,
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileGroupingRule {
    pub pattern: String,
    pub scope: String,
    pub prefer_separate_commit: bool,
    pub group_with: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct BranchConventions {
    pub rules: Vec<BranchRule>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BranchRule {
    pub branch_pattern: String,
    pub prefix: String,
    pub require_body: bool,
}

/// Parsed commitlint configuration
#[derive(Debug, Deserialize)]
struct CommitlintConfig {
    #[serde(rename = "rules")]
    _rules: Option<Vec<CommitlintRule>>,
    #[serde(rename = "extends")]
    _extends: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CommitlintRule {
    #[serde(rename = "type")]
    _type: Option<String>,
    #[serde(rename = "level")]
    _level: Option<String>,
    #[serde(rename = "value")]
    _value: Option<serde_json::Value>,
}

/// Package.json with commitlint section
#[derive(Debug, Deserialize)]
struct PackageJson {
    #[serde(rename = "commitlint")]
    commitlint: Option<CommitlintConfig>,
}

impl CommitConventions {
    /// Discover commit conventions from any supported source
    /// Tries in order: .commit-conventions.md, .commitlintrc.json,
    /// package.json, CONTRIBUTING.md
    pub fn discover_any(repo_root: &str) -> Result<Option<Self>> {
        // 1. Try .commit-conventions.md (our proposed standard)
        let markdown_path = format!("{}/.commit-conventions.md", repo_root);
        if Path::new(&markdown_path).exists() {
            return Self::from_markdown(&markdown_path).map(Some);
        }

        // 2. Try .commitlintrc.json
        let commitlint_path = format!("{}/.commitlintrc.json", repo_root);
        if Path::new(&commitlint_path).exists() {
            return Self::from_commitlint_json(&commitlint_path).map(Some);
        }

        // 3. Try commitlintrc.js (basic parsing)
        let js_path = format!("{}/.commitlintrc.js", repo_root);
        if Path::new(&js_path).exists() {
            if let Ok(conv) = Self::from_commitlint_js(&js_path) {
                return Ok(Some(conv));
            }
        }

        // 4. Try commitlint.config.js
        let js_config_path = format!("{}/commitlint.config.js", repo_root);
        if Path::new(&js_config_path).exists() {
            if let Ok(conv) = Self::from_commitlint_js(&js_config_path) {
                return Ok(Some(conv));
            }
        }

        // 5. Try package.json
        let package_path = format!("{}/package.json", repo_root);
        if Path::new(&package_path).exists() {
            if let Ok(conv) = Self::from_package_json(&package_path) {
                return Ok(Some(conv));
            }
        }

        // 6. Try CONTRIBUTING.md
        let contributing_path = format!("{}/CONTRIBUTING.md", repo_root);
        if Path::new(&contributing_path).exists() {
            if let Ok(conv) = Self::from_contributing_md(&contributing_path) {
                return Ok(Some(conv));
            }
        }

        Ok(None)
    }

    /// Parse from .commit-conventions.md
    pub fn from_markdown(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path))?;

        Self::parse_markdown(&content)
    }

    /// Parse markdown content into structured conventions
    fn parse_markdown(md: &str) -> Result<Self> {
        let mut conventions = Self::default();
        let mut current_section: Option<String> = None;
        let mut in_code_block = false;
        let mut code_block_content = String::new();

        for line in md.lines() {
            // Track code blocks
            if line.starts_with("```") {
                if in_code_block {
                    // End of code block
                    if current_section.as_deref() == Some("Examples") {
                        conventions.examples.push(code_block_content.trim().to_string());
                    }
                    code_block_content.clear();
                    in_code_block = false;
                } else {
                    in_code_block = true;
                }
                continue;
            }

            if in_code_block {
                code_block_content.push_str(line);
                code_block_content.push('\n');
                continue;
            }

            // Parse sections
            if line.starts_with("## ") {
                current_section = Some(line[3..].trim().to_string());
                continue;
            }

            // Parse types section
            if current_section.as_deref() == Some("Types") {
                if let Some(rest) = line.strip_prefix("- `") {
                    if let Some(type_end) = rest.find('`') {
                        let type_name = rest[..type_end].to_string();
                        let description = rest[type_end + 1..].trim().trim_start_matches('-').trim().to_string();
                        if !type_name.is_empty() {
                            conventions.types.push(CommitType {
                                name: type_name,
                                description,
                            });
                        }
                    }
                }
            }

            // Parse scopes section
            if current_section.as_deref() == Some("Scopes") {
                if let Some(rest) = line.strip_prefix("- `") {
                    if let Some(scope_end) = rest.find('`') {
                        let scope = rest[..scope_end].to_string();
                        if !scope.is_empty() {
                            conventions.scopes.push(scope);
                        }
                    }
                }
            }

            // Parse workflow rules
            if current_section.as_deref() == Some("Workflow Rules")
                || current_section.as_deref() == Some("File Grouping")
            {
                if let Some(pattern) = line.strip_prefix("- `") {
                    if let Some(pattern_end) = pattern.find('`') {
                        let pattern_str = pattern[..pattern_end].to_string();
                        let rest = pattern[pattern_end + 1..].trim();
                        if let Some(sep_pos) = rest.find("→") {
                            let scope_part = rest[..sep_pos].trim().to_string();
                            conventions
                                .workflow_rules
                                .get_or_insert_with(WorkflowRules::default)
                                .file_groupings
                                .push(FileGroupingRule {
                                    pattern: pattern_str,
                                    scope: scope_part,
                                    prefer_separate_commit: true,
                                    group_with: None,
                                });
                        }
                    }
                }
            }
        }

        Ok(conventions)
    }

    /// Parse from .commitlintrc.json
    fn from_commitlint_json(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path))?;

        let _config: CommitlintConfig = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path))?;

        // Commitlint config is mostly about rules, not conventions
        // Return basic conventional commits defaults
        Ok(Self::conventional_commits_defaults())
    }

    /// Parse from .commitlintrc.js or commitlint.config.js
    fn from_commitlint_js(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path))?;

        // Basic parsing - look for common patterns
        if content.contains("'@commitlint/config-conventional'")
            || content.contains("\"@commitlint/config-conventional\"")
        {
            return Ok(Self::conventional_commits_defaults());
        }

        // Try to extract custom types from the JS config
        let conventions = Self::conventional_commits_defaults();

        // Look for extends array
        if let Some(start) = content.find("extends:") {
            let section = &content[start..];
            // Extract array contents
            if let Some(array_start) = section.find('[') {
                let array_section = &section[array_start..];
                if let Some(array_end) = array_section.find(']') {
                    let array_content = &array_section[..array_end];
                    // Parse extends
                    for line in array_content.lines() {
                        if line.contains("conventional") || line.contains("config-conventional") {
                            return Ok(Self::conventional_commits_defaults());
                        }
                    }
                }
            }
        }

        Ok(conventions)
    }

    /// Parse from package.json (commitlint section)
    fn from_package_json(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path))?;

        let pkg: PackageJson = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path))?;

        if pkg.commitlint.is_some() {
            Ok(Self::conventional_commits_defaults())
        } else {
            // No commitlint config in package.json
            anyhow::bail!("No commitlint config found in package.json");
        }
    }

    /// Parse from CONTRIBUTING.md
    fn from_contributing_md(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path))?;

        // Look for commit message section
        let in_commit_section = content
            .to_lowercase()
            .contains("commit")
            && (content.contains("convention") || content.contains("guideline"));

        if !in_commit_section {
            anyhow::bail!("No commit message section found in CONTRIBUTING.md");
        }

        // Try to extract examples and patterns
        let mut conventions = Self::conventional_commits_defaults();

        // Extract code blocks that look like commit messages
        for line in content.lines() {
            if line.contains("feat(") || line.contains("fix(") || line.contains("chore:") {
                conventions.examples.push(line.trim().to_string());
            }
        }

        Ok(conventions)
    }

    /// Get default Conventional Commits specification
    fn conventional_commits_defaults() -> Self {
        Self {
            types: vec![
                CommitType {
                    name: "feat".to_string(),
                    description: "New feature".to_string(),
                },
                CommitType {
                    name: "fix".to_string(),
                    description: "Bug fix".to_string(),
                },
                CommitType {
                    name: "docs".to_string(),
                    description: "Documentation changes".to_string(),
                },
                CommitType {
                    name: "style".to_string(),
                    description: "Code style changes".to_string(),
                },
                CommitType {
                    name: "refactor".to_string(),
                    description: "Code refactoring".to_string(),
                },
                CommitType {
                    name: "test".to_string(),
                    description: "Test additions or changes".to_string(),
                },
                CommitType {
                    name: "chore".to_string(),
                    description: "Maintenance tasks".to_string(),
                },
                CommitType {
                    name: "perf".to_string(),
                    description: "Performance improvements".to_string(),
                },
                CommitType {
                    name: "ci".to_string(),
                    description: "CI/CD changes".to_string(),
                },
                CommitType {
                    name: "build".to_string(),
                    description: "Build system changes".to_string(),
                },
            ],
            scopes: vec![],
            examples: vec![],
            workflow_rules: None,
            branch_conventions: None,
        }
    }

    /// Convert conventions to a prompt fragment for the AI model
    pub fn to_prompt_fragment(&self) -> String {
        let mut out = String::new();

        if !self.types.is_empty() {
            out.push_str("Allowed commit types:\n");
            for t in &self.types {
                out.push_str(&format!("  - {}: {}\n", t.name, t.description));
            }
            out.push('\n');
        }

        if !self.scopes.is_empty() {
            out.push_str("Common scopes for this project:\n");
            for scope in &self.scopes {
                out.push_str(&format!("  - {}\n", scope));
            }
            out.push('\n');
        }

        if !self.examples.is_empty() {
            out.push_str("Example commit messages from this project:\n");
            for ex in self.examples.iter().take(5) {
                out.push_str(&format!("  {}\n", ex));
            }
            out.push('\n');
        }

        if let Some(rules) = &self.workflow_rules {
            if !rules.file_groupings.is_empty() {
                out.push_str("File grouping rules:\n");
                for rule in &rules.file_groupings {
                    out.push_str(&format!(
                        "  - Files matching {} should use scope '{}'\n",
                        rule.pattern, rule.scope
                    ));
                    if rule.prefer_separate_commit {
                        out.push_str(&format!("    (prefer separate commit)\n"));
                    }
                }
                out.push('\n');
            }

            if !rules.always_separate.is_empty() {
                out.push_str("Files that should always be separate commits:\n");
                for file in &rules.always_separate {
                    out.push_str(&format!("  - {}\n", file));
                }
                out.push('\n');
            }

            if !rules.ignore.is_empty() {
                out.push_str("Files to ignore:\n");
                for file in &rules.ignore {
                    out.push_str(&format!("  - {}\n", file));
                }
                out.push('\n');
            }
        }

        out
    }

    /// Suggest file groupings based on workflow rules
    #[allow(dead_code)]
    pub fn suggest_file_groupings(&self, files: &[crate::git::StagedFile]) -> Vec<FileGrouping> {
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();

        if let Some(rules) = &self.workflow_rules {
            for file in files {
                let mut grouped = false;

                for rule in &rules.file_groupings {
                    if Self::pattern_matches(&rule.pattern, &file.path) {
                        groups
                            .entry(rule.scope.clone())
                            .or_insert_with(Vec::new)
                            .push(file.path.clone());
                        grouped = true;
                        break;
                    }
                }

                if !grouped {
                    groups
                        .entry("other".to_string())
                        .or_insert_with(Vec::new)
                        .push(file.path.clone());
                }
            }
        }

        groups
            .into_iter()
            .map(|(scope, files)| FileGrouping { scope, files })
            .collect()
    }

    /// Check if a glob pattern matches a file path
    fn pattern_matches(pattern: &str, path: &str) -> bool {
        // Simple glob matching - expand this for production
        if pattern.contains("**") {
            let base = pattern.replace("**/*", "").replace("**", "");
            path.starts_with(&base) || path.contains(&base)
        } else if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                path.starts_with(parts[0]) && path.ends_with(parts[1])
            } else {
                path.contains(&pattern.replace('*', ""))
            }
        } else {
            path == pattern || path.starts_with(&format!("{}/", pattern))
        }
    }

    /// Check if a file should be ignored
    #[allow(dead_code)]
    pub fn should_ignore_file(&self, path: &str) -> bool {
        if let Some(rules) = &self.workflow_rules {
            for pattern in &rules.ignore {
                if Self::pattern_matches(pattern, path) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a file should always be a separate commit
    #[allow(dead_code)]
    pub fn should_be_separate(&self, path: &str) -> bool {
        if let Some(rules) = &self.workflow_rules {
            for pattern in &rules.always_separate {
                if Self::pattern_matches(pattern, path) {
                    return true;
                }
            }
        }
        false
    }

    /// Apply branch-specific conventions
    #[allow(dead_code)]
    pub fn apply_branch_rules(&self, branch: &str) -> CommitConventions {
        let mut result = self.clone();

        if let Some(branch_conv) = &self.branch_conventions {
            for rule in &branch_conv.rules {
                if Self::pattern_matches(&rule.branch_pattern, branch) {
                    // Filter examples to those matching the prefix
                    if !rule.prefix.is_empty() {
                        result.examples = self
                            .examples
                            .iter()
                            .filter(|ex| ex.starts_with(&rule.prefix))
                            .cloned()
                            .collect();
                    }
                    break;
                }
            }
        }

        result
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileGrouping {
    pub scope: String,
    pub files: Vec<String>,
}
