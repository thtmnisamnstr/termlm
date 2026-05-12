use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationFinding {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundedProposal {
    pub command: String,
    pub intent: String,
    pub expected_effect: String,
    pub commands_used: Vec<String>,
    pub risk_level: String,
    pub destructive: bool,
    pub requires_approval: bool,
    pub grounding: Vec<String>,
    pub validation: Vec<ValidationFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationContext {
    pub prompt: String,
    pub command_exists: bool,
    pub docs_excerpt: String,
    pub validate_command_flags: bool,
    pub parse_ambiguous: bool,
    pub parse_warnings: Vec<String>,
    pub parse_risky_constructs: bool,
}

pub fn validate_round(
    proposal: &GroundedProposal,
    ctx: &ValidationContext,
) -> Vec<ValidationFinding> {
    let mut findings = Vec::new();
    let cmd = proposal.command.trim();

    if cmd.is_empty() {
        findings.push(ValidationFinding {
            kind: "insufficient_draft".to_string(),
            detail: "draft command is empty".to_string(),
        });
        return findings;
    }

    if !ctx.command_exists {
        findings.push(ValidationFinding {
            kind: "unknown_command".to_string(),
            detail: "first significant token is not installed in this shell context".to_string(),
        });
    }

    if ctx.parse_ambiguous {
        let detail = if ctx.parse_warnings.is_empty() {
            "shell parse was ambiguous".to_string()
        } else {
            ctx.parse_warnings.join("; ")
        };
        findings.push(ValidationFinding {
            kind: "parse_ambiguous".to_string(),
            detail,
        });
    }

    if ctx.parse_ambiguous && ctx.parse_risky_constructs {
        findings.push(ValidationFinding {
            kind: "parse_ambiguous_risky".to_string(),
            detail: "ambiguous parse intersects pipelines/redirections/control operators"
                .to_string(),
        });
    }

    if proposal.grounding.is_empty() {
        findings.push(ValidationFinding {
            kind: "missing_grounding".to_string(),
            detail: "proposal has no local grounding evidence".to_string(),
        });
    }

    if ctx.validate_command_flags && !ctx.docs_excerpt.is_empty() {
        let docs = ctx.docs_excerpt.to_ascii_lowercase();
        let mut missing_flags = BTreeSet::new();
        for flag in extract_flags(cmd) {
            if flag == "-" || flag == "--" {
                continue;
            }
            if !docs.contains(&flag.to_ascii_lowercase()) {
                missing_flags.insert(flag);
            }
        }
        if !missing_flags.is_empty() {
            findings.push(ValidationFinding {
                kind: "unsupported_flag".to_string(),
                detail: format!(
                    "flags missing from local docs: {}",
                    missing_flags.into_iter().collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }

    let p = ctx.prompt.to_ascii_lowercase();
    let c = cmd.to_ascii_lowercase();
    if (p.contains("modified") || p.contains("mtime") || p.contains("sorted"))
        && c.starts_with("ls ")
        && !command_has_flag(cmd, 't', "--sort")
    {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "request asked for sorted/mtime behavior but draft lacks sort flags"
                .to_string(),
        });
    }

    findings
}

fn extract_flags(command: &str) -> Vec<String> {
    let mut flags = Vec::new();
    for token in command.split_whitespace() {
        if token == "--" {
            break;
        }
        if token.starts_with("--") && token.len() > 2 {
            let key = token.split('=').next().unwrap_or(token).to_string();
            flags.push(key);
        } else if token.starts_with('-') && token.len() > 1 {
            if token.chars().nth(1) == Some('-') {
                continue;
            }
            if token.len() > 2 && !token.contains('=') {
                for ch in token[1..].chars() {
                    flags.push(format!("-{ch}"));
                }
            } else {
                flags.push(token.to_string());
            }
        }
    }
    flags
}

fn command_has_flag(command: &str, short: char, long: &str) -> bool {
    let short = format!("-{short}");
    extract_flags(command)
        .iter()
        .any(|flag| flag == &short || flag == long)
}

pub fn revise_command(
    command: &str,
    prompt: &str,
    findings: &[ValidationFinding],
    suggestion: Option<&str>,
) -> Option<String> {
    let mut revised = command.trim().to_string();
    if revised.is_empty() {
        return None;
    }

    for finding in findings {
        match finding.kind.as_str() {
            "unknown_command" => {
                if let Some(s) = suggestion {
                    revised = replace_first_token(&revised, s);
                }
            }
            "unsupported_flag" => {
                revised = strip_flags_from_detail(&revised, &finding.detail);
            }
            "insufficient_for_prompt" => {
                let p = prompt.to_ascii_lowercase();
                if revised.starts_with("ls ") {
                    if (p.contains("modified") || p.contains("mtime") || p.contains("sorted"))
                        && !command_has_flag(&revised, 't', "--sort")
                    {
                        revised = ensure_compound_short_flag(&revised, 't');
                    }
                    if p.contains("all") && !command_has_flag(&revised, 'a', "--all") {
                        revised = ensure_compound_short_flag(&revised, 'a');
                    }
                }
            }
            "parse_ambiguous" => {
                if let Some(simplified) = simplify_ambiguous_command(&revised) {
                    revised = simplified;
                }
            }
            "parse_ambiguous_risky" => {
                return None;
            }
            _ => {}
        }
    }

    if revised == command.trim() {
        None
    } else {
        Some(revised)
    }
}

fn replace_first_token(command: &str, replacement: &str) -> String {
    let mut parts = command.split_whitespace();
    if parts.next().is_none() {
        return command.to_string();
    }
    let mut out = Vec::new();
    out.push(replacement.to_string());
    out.extend(parts.map(ToString::to_string));
    out.join(" ")
}

fn strip_flags_from_detail(command: &str, detail: &str) -> String {
    let mut remove = BTreeSet::new();
    for piece in detail.split(':').nth(1).unwrap_or_default().split(',') {
        let f = piece.trim();
        if f.starts_with('-') {
            remove.insert(f.to_string());
        }
    }
    if remove.is_empty() {
        return command.to_string();
    }

    command
        .split_whitespace()
        .filter(|tok| !remove.contains(*tok))
        .collect::<Vec<_>>()
        .join(" ")
}

fn ensure_compound_short_flag(command: &str, flag: char) -> String {
    let mut tokens = command
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if tokens.len() < 2 {
        return command.to_string();
    }
    if tokens[1].starts_with('-') && !tokens[1].starts_with("--") {
        if !tokens[1].contains(flag) {
            tokens[1].push(flag);
        }
        return tokens.join(" ");
    }

    tokens.insert(1, format!("-{flag}"));
    tokens.join(" ")
}

fn simplify_ambiguous_command(command: &str) -> Option<String> {
    let mut out = command.trim().to_string();
    if out.is_empty() {
        return None;
    }
    let original = out.clone();

    loop {
        let trimmed = out.trim_end();
        let shortened = trimmed
            .strip_suffix("&&")
            .or_else(|| trimmed.strip_suffix("||"))
            .or_else(|| trimmed.strip_suffix('|'))
            .or_else(|| trimmed.strip_suffix(';'))
            .or_else(|| trimmed.strip_suffix('&'));
        if let Some(next) = shortened {
            out = next.trim_end().to_string();
            continue;
        }
        break;
    }

    if out == original || out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_flags_parse_ambiguity() {
        let proposal = GroundedProposal {
            command: "ls -la |".to_string(),
            intent: "list files".to_string(),
            expected_effect: "show files".to_string(),
            commands_used: vec!["ls".to_string()],
            risk_level: "read_only".to_string(),
            destructive: false,
            requires_approval: true,
            grounding: vec!["docs#ls".to_string()],
            validation: Vec::new(),
        };
        let findings = validate_round(
            &proposal,
            &ValidationContext {
                prompt: "list files".to_string(),
                command_exists: true,
                docs_excerpt: "ls -a -l".to_string(),
                validate_command_flags: true,
                parse_ambiguous: true,
                parse_warnings: vec!["command ends with a shell operator".to_string()],
                parse_risky_constructs: true,
            },
        );
        assert!(findings.iter().any(|f| f.kind == "parse_ambiguous"));
        assert!(findings.iter().any(|f| f.kind == "parse_ambiguous_risky"));
    }

    #[test]
    fn validate_sort_request_accepts_compound_ls_time_flag() {
        let proposal = GroundedProposal {
            command: "ls -lt".to_string(),
            intent: "list newest files".to_string(),
            expected_effect: "show files by modification time".to_string(),
            commands_used: vec!["ls".to_string()],
            risk_level: "read_only".to_string(),
            destructive: false,
            requires_approval: true,
            grounding: vec!["docs#ls".to_string()],
            validation: Vec::new(),
        };
        let findings = validate_round(
            &proposal,
            &ValidationContext {
                prompt: "show files sorted newest first".to_string(),
                command_exists: true,
                docs_excerpt: "ls -l -t --sort".to_string(),
                validate_command_flags: true,
                parse_ambiguous: false,
                parse_warnings: Vec::new(),
                parse_risky_constructs: false,
            },
        );
        assert!(
            !findings
                .iter()
                .any(|finding| finding.kind == "insufficient_for_prompt"),
            "{findings:?}"
        );
    }

    #[test]
    fn revise_strips_trailing_operator_on_ambiguity() {
        let revised = revise_command(
            "ls -la |",
            "list files",
            &[ValidationFinding {
                kind: "parse_ambiguous".to_string(),
                detail: "command ends with a shell operator".to_string(),
            }],
            None,
        );
        assert_eq!(revised.as_deref(), Some("ls -la"));
    }
}
