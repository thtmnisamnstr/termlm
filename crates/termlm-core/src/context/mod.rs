use crate::tasks::{ClassificationResult, TaskClassification};
use chrono::{DateTime, Utc};
use termlm_config::AppConfig;

#[derive(Debug, Clone)]
pub struct ToolExposureProfile {
    pub execute_shell_command: bool,
    pub lookup_command_docs: bool,
    pub local_file_tools: bool,
    pub terminal_context_tool: bool,
    pub web_tools: bool,
}

#[derive(Debug, Clone)]
pub struct TerminalSnippet {
    pub command: String,
    pub cwd: String,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub exit_code: i32,
    pub output_capture_status: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub redactions_applied: bool,
    pub detected_error_lines: Vec<String>,
    pub detected_paths: Vec<String>,
    pub detected_urls: Vec<String>,
    pub detected_commands: Vec<String>,
    pub stdout_head: String,
    pub stdout_tail: String,
    pub stderr_head: String,
    pub stderr_tail: String,
    pub stdout_full_ref: Option<String>,
    pub stderr_full_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContextAssembly {
    pub prompt: String,
    pub included_blocks: Vec<String>,
}

pub fn exposure_for(classification: &TaskClassification) -> ToolExposureProfile {
    match classification {
        TaskClassification::FreshCommandRequest => ToolExposureProfile {
            execute_shell_command: true,
            lookup_command_docs: true,
            local_file_tools: false,
            terminal_context_tool: false,
            web_tools: true,
        },
        TaskClassification::DiagnosticDebugging => ToolExposureProfile {
            execute_shell_command: true,
            lookup_command_docs: true,
            local_file_tools: true,
            terminal_context_tool: true,
            web_tools: true,
        },
        TaskClassification::WebCurrentInfoQuestion => ToolExposureProfile {
            execute_shell_command: false,
            lookup_command_docs: true,
            local_file_tools: false,
            terminal_context_tool: false,
            web_tools: true,
        },
        TaskClassification::DocumentationQuestion => ToolExposureProfile {
            execute_shell_command: false,
            lookup_command_docs: true,
            local_file_tools: false,
            terminal_context_tool: false,
            web_tools: false,
        },
        TaskClassification::ReferentialFollowup | TaskClassification::ExploratoryShellQuestion => {
            ToolExposureProfile {
                execute_shell_command: true,
                lookup_command_docs: true,
                local_file_tools: true,
                terminal_context_tool: true,
                web_tools: true,
            }
        }
    }
}

pub fn determine_tool_exposure(
    classification: &TaskClassification,
    cfg: &AppConfig,
) -> ToolExposureProfile {
    let mut profile = if cfg.tool_routing.dynamic_exposure_enabled {
        exposure_for(classification)
    } else {
        ToolExposureProfile {
            execute_shell_command: true,
            lookup_command_docs: true,
            local_file_tools: true,
            terminal_context_tool: true,
            web_tools: true,
        }
    };

    if cfg.tool_routing.always_expose_execute {
        profile.execute_shell_command = true;
    }
    if cfg.tool_routing.always_expose_lookup_docs {
        profile.lookup_command_docs = true;
    }
    if !cfg.local_tools.enabled {
        profile.local_file_tools = false;
        profile.terminal_context_tool = false;
    }
    if !cfg.tool_routing.expose_file_tools_for_local_questions {
        profile.local_file_tools = false;
    }
    if !cfg.tool_routing.expose_terminal_context_only_when_needed {
        profile.terminal_context_tool = cfg.local_tools.enabled;
    }
    if !cfg.web.enabled || !cfg.web.expose_tools {
        profile.web_tools = false;
    }
    if cfg.tool_routing.expose_web_only_when_needed
        && !matches!(classification, TaskClassification::WebCurrentInfoQuestion)
        && !profile.execute_shell_command
    {
        profile.web_tools = false;
    }
    profile
}

pub fn assemble_user_prompt(
    question: &str,
    classification: &ClassificationResult,
    observed: &[TerminalSnippet],
    session_memory: &[String],
    cfg: &AppConfig,
) -> ContextAssembly {
    if !cfg.context_budget.enabled {
        return ContextAssembly {
            prompt: question.to_string(),
            included_blocks: vec!["current_question".to_string()],
        };
    }

    let mut included_blocks = Vec::<String>::new();
    let mut out = String::new();

    let mut budget = cfg
        .context_budget
        .max_total_context_tokens
        .saturating_sub(cfg.context_budget.reserve_response_tokens);
    if budget == 0 {
        budget = cfg.context_budget.current_question_tokens.max(256);
    }

    let current_cap = cfg.context_budget.current_question_tokens.min(budget);
    let current = trim_to_tokens(question, current_cap);
    let current_tokens = estimate_tokens(&current).min(budget);
    out.push_str(&current);
    budget = budget.saturating_sub(current_tokens);
    included_blocks.push("current_question".to_string());

    let include_recent = if cfg.tool_routing.expose_terminal_context_only_when_needed {
        matches!(
            classification.classification,
            TaskClassification::DiagnosticDebugging | TaskClassification::ReferentialFollowup
        )
    } else {
        cfg.terminal_context.enabled
    };
    let include_older = include_recent
        && (references_older_state(question)
            || observed.is_empty()
            || contains_retry_intent(question));

    if include_recent && budget > 0 {
        let mut ordered = observed.to_vec();
        if cfg.context_budget.trim_strategy == "priority_newest_first" {
            ordered.sort_by_key(|b| std::cmp::Reverse(b.started_at));
        } else {
            ordered.sort_by_key(|b| b.started_at);
        }
        let block = format_recent_terminal_block(&ordered);
        let cap = cfg
            .context_budget
            .recent_terminal_tokens
            .min(cfg.terminal_context.recent_context_max_tokens)
            .min(budget);
        let trimmed = trim_to_tokens(&block, cap);
        if !trimmed.trim().is_empty() {
            out.push_str("\n\n");
            out.push_str(&trimmed);
            let used = estimate_tokens(&trimmed).min(budget);
            budget = budget.saturating_sub(used);
            included_blocks.push("recent_terminal".to_string());
        }
    }

    if include_older && budget > 0 && !session_memory.is_empty() {
        let block = format_older_session_block(session_memory);
        let cap = cfg
            .context_budget
            .older_session_tokens
            .min(cfg.terminal_context.older_context_max_tokens)
            .min(budget);
        let trimmed = trim_to_tokens(&block, cap);
        if !trimmed.trim().is_empty() {
            out.push_str("\n\n");
            out.push_str(&trimmed);
            included_blocks.push("older_session".to_string());
        }
    }

    out.push_str(&format!(
        "\n\n[task_classification={} confidence={:.2}]",
        classification_label(&classification.classification),
        classification.confidence
    ));

    ContextAssembly {
        prompt: out,
        included_blocks,
    }
}

fn format_recent_terminal_block(entries: &[TerminalSnippet]) -> String {
    let mut out = String::from("## Recent terminal context\n");
    let mut exact_failed_stderr_included = false;
    for entry in entries.iter().take(25) {
        out.push_str(&format!(
            "- {} | cwd={} | exit={} | duration_ms={} | capture={}\n  $ {}\n",
            entry
                .started_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            entry.cwd,
            entry.exit_code,
            entry.duration_ms,
            entry.output_capture_status,
            entry.command
        ));
        if entry.stdout_truncated || entry.stderr_truncated {
            out.push_str(&format!(
                "  truncation: stdout={} stderr={}\n",
                entry.stdout_truncated, entry.stderr_truncated
            ));
        }
        if entry.redactions_applied {
            out.push_str("  redaction: true\n");
        }
        if !entry.detected_commands.is_empty() {
            out.push_str("  detected_commands: ");
            out.push_str(&entry.detected_commands.join(", "));
            out.push('\n');
        }
        if !entry.detected_paths.is_empty() {
            out.push_str("  detected_paths: ");
            out.push_str(&entry.detected_paths.join(", "));
            out.push('\n');
        }
        if !entry.detected_urls.is_empty() {
            out.push_str("  detected_urls: ");
            out.push_str(&entry.detected_urls.join(", "));
            out.push('\n');
        }
        if !entry.detected_error_lines.is_empty() {
            out.push_str("  error_lines: ");
            out.push_str(&entry.detected_error_lines.join(" | "));
            out.push('\n');
        }
        if !entry.stdout_head.trim().is_empty() {
            out.push_str("  stdout_head: ");
            out.push_str(&entry.stdout_head.replace('\n', "\\n"));
            out.push('\n');
        }
        if !entry.stdout_tail.trim().is_empty() {
            out.push_str("  stdout_tail: ");
            out.push_str(&entry.stdout_tail.replace('\n', "\\n"));
            out.push('\n');
        }
        if !entry.stderr_head.trim().is_empty() {
            out.push_str("  stderr_head: ");
            out.push_str(&entry.stderr_head.replace('\n', "\\n"));
            out.push('\n');
        }
        if !entry.stderr_tail.trim().is_empty() {
            out.push_str("  stderr_tail: ");
            out.push_str(&entry.stderr_tail.replace('\n', "\\n"));
            out.push('\n');
            if !exact_failed_stderr_included && entry.exit_code != 0 {
                out.push_str("  recent_failed_stderr_exact:\n");
                for line in entry.stderr_tail.lines().take(8) {
                    out.push_str("    ");
                    out.push_str(line);
                    out.push('\n');
                }
                exact_failed_stderr_included = true;
            }
        }
        if let Some(stdout_ref) = entry.stdout_full_ref.as_ref() {
            out.push_str("  stdout_ref: ");
            out.push_str(stdout_ref);
            out.push('\n');
        }
        if let Some(stderr_ref) = entry.stderr_full_ref.as_ref() {
            out.push_str("  stderr_ref: ");
            out.push_str(stderr_ref);
            out.push('\n');
        }
    }
    out
}

fn format_older_session_block(entries: &[String]) -> String {
    let mut out = String::from("## Older session memory\n");
    let len = entries.len();
    let start = len.saturating_sub(20);
    for item in &entries[start..] {
        out.push_str("- ");
        out.push_str(item);
        out.push('\n');
    }
    out
}

fn classification_label(classification: &TaskClassification) -> &'static str {
    match classification {
        TaskClassification::FreshCommandRequest => "fresh_command_request",
        TaskClassification::ReferentialFollowup => "referential_followup",
        TaskClassification::DiagnosticDebugging => "diagnostic_debugging",
        TaskClassification::DocumentationQuestion => "documentation_question",
        TaskClassification::WebCurrentInfoQuestion => "web_current_info_question",
        TaskClassification::ExploratoryShellQuestion => "exploratory_shell_question",
    }
}

fn contains_retry_intent(prompt: &str) -> bool {
    let p = prompt.to_ascii_lowercase();
    ["again", "retry", "re-run", "rerun", "try again"]
        .iter()
        .any(|k| p.contains(k))
}

fn references_older_state(prompt: &str) -> bool {
    let p = prompt.to_ascii_lowercase();
    [
        "earlier",
        "before",
        "previous",
        "last time",
        "prior",
        "that output",
    ]
    .iter()
    .any(|k| p.contains(k))
}

pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    (chars / 4).max(1)
}

pub fn trim_to_tokens(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    let max_chars = max_tokens.saturating_mul(4);
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_trim_to_tokens() {
        let s = "abcdefghij";
        assert_eq!(trim_to_tokens(s, 2), "abcdefg…");
    }

    #[test]
    fn web_classification_turns_on_web_tools() {
        let cfg = AppConfig::default();
        let profile = determine_tool_exposure(&TaskClassification::WebCurrentInfoQuestion, &cfg);
        assert!(profile.web_tools);
        assert!(!profile.local_file_tools);
        assert!(!profile.terminal_context_tool);
    }

    #[test]
    fn default_routing_exposes_web_fallback_for_local_command_tasks() {
        let cfg = AppConfig::default();
        let profile = determine_tool_exposure(&TaskClassification::FreshCommandRequest, &cfg);
        assert!(profile.web_tools);
    }

    #[test]
    fn web_config_can_disable_web_tools() {
        let mut cfg = AppConfig::default();
        cfg.web.enabled = false;
        let profile = determine_tool_exposure(&TaskClassification::WebCurrentInfoQuestion, &cfg);
        assert!(!profile.web_tools);

        let mut cfg = AppConfig::default();
        cfg.web.expose_tools = false;
        let profile = determine_tool_exposure(&TaskClassification::WebCurrentInfoQuestion, &cfg);
        assert!(!profile.web_tools);
    }

    #[test]
    fn file_tools_flag_only_disables_file_tools() {
        let mut cfg = AppConfig::default();
        cfg.tool_routing.expose_file_tools_for_local_questions = false;
        let profile = determine_tool_exposure(&TaskClassification::DiagnosticDebugging, &cfg);
        assert!(!profile.local_file_tools);
        assert!(profile.terminal_context_tool);
    }

    #[test]
    fn terminal_context_flag_can_force_exposure() {
        let mut cfg = AppConfig::default();
        cfg.tool_routing.expose_terminal_context_only_when_needed = false;
        let profile = determine_tool_exposure(&TaskClassification::FreshCommandRequest, &cfg);
        assert!(profile.terminal_context_tool);
    }

    #[test]
    fn fresh_task_skips_terminal_context_block() {
        let cfg = AppConfig::default();
        let result = assemble_user_prompt(
            "list files",
            &ClassificationResult {
                classification: TaskClassification::FreshCommandRequest,
                confidence: 0.8,
            },
            &[],
            &[],
            &cfg,
        );
        assert_eq!(result.included_blocks, vec!["current_question".to_string()]);
    }
}
