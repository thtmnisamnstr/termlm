use crate::redaction::redact_secrets;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedTerminalEntry {
    pub command_seq: u64,
    pub command: String,
    pub cwd: String,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub exit_code: i32,
    #[serde(default)]
    pub detected_urls: Vec<String>,
    pub stderr_head: String,
    pub stderr_tail: String,
    pub stdout_head: String,
    pub stdout_tail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_full_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_full_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSearchResult {
    pub query: String,
    pub results: Vec<ObservedTerminalEntry>,
}

pub fn search_terminal_context(
    entries: &[ObservedTerminalEntry],
    query: &str,
    max_results: usize,
) -> TerminalSearchResult {
    let mut results = entries
        .iter()
        .rev()
        .filter(|e| {
            e.command.contains(query)
                || e.stderr_head.contains(query)
                || e.stderr_tail.contains(query)
                || e.stdout_head.contains(query)
                || e.stdout_tail.contains(query)
                || e.detected_urls.iter().any(|u| u.contains(query))
        })
        .take(max_results)
        .cloned()
        .collect::<Vec<_>>();

    for r in &mut results {
        r.command = redact_secrets(&r.command);
        r.stderr_head = redact_secrets(&r.stderr_head);
        r.stderr_tail = redact_secrets(&r.stderr_tail);
        r.stdout_head = redact_secrets(&r.stdout_head);
        r.stdout_tail = redact_secrets(&r.stdout_tail);
        r.detected_urls = r
            .detected_urls
            .iter()
            .map(|u| redact_secrets(u))
            .collect::<Vec<_>>();
    }

    TerminalSearchResult {
        query: query.to_string(),
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::{ObservedTerminalEntry, search_terminal_context};
    use chrono::Utc;

    fn sample_entry() -> ObservedTerminalEntry {
        ObservedTerminalEntry {
            command_seq: 1,
            command: "curl https://example.com?token=abc123".to_string(),
            cwd: "/tmp".to_string(),
            started_at: Utc::now(),
            duration_ms: 42,
            exit_code: 1,
            detected_urls: vec!["https://example.com?token=abc123".to_string()],
            stderr_head: "Authorization: Bearer abc".to_string(),
            stderr_tail: "Permission denied".to_string(),
            stdout_head: "Cookie: session=topsecret".to_string(),
            stdout_tail: "done".to_string(),
            stdout_full_ref: Some("/tmp/stdout.txt".to_string()),
            stderr_full_ref: Some("/tmp/stderr.txt".to_string()),
        }
    }

    #[test]
    fn search_matches_urls_and_redacts_output() {
        let entries = vec![sample_entry()];
        let result = search_terminal_context(&entries, "example.com", 10);
        assert_eq!(result.results.len(), 1);
        let first = &result.results[0];
        assert!(first.detected_urls[0].contains("<redacted>"));
        assert!(first.stderr_head.contains("<redacted>"));
        assert!(first.stdout_head.contains("<redacted>"));
    }

    #[test]
    fn search_preserves_output_refs() {
        let entries = vec![sample_entry()];
        let result = search_terminal_context(&entries, "Permission denied", 10);
        assert_eq!(result.results.len(), 1);
        let first = &result.results[0];
        assert_eq!(first.stdout_full_ref.as_deref(), Some("/tmp/stdout.txt"));
        assert_eq!(first.stderr_full_ref.as_deref(), Some("/tmp/stderr.txt"));
    }
}
