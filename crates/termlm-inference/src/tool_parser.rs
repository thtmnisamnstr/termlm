use crate::ToolCall;
use anyhow::{Result, anyhow};
use regex::Regex;
use serde_json::Value;

pub fn parse_tagged_tool_calls(input: &str) -> Result<Vec<ToolCall>> {
    let mut out = Vec::new();
    let sanitized = sanitize_tagged_input(input);
    let mut rest = sanitized.as_str();
    let key_re = Regex::new(r#"([A-Za-z_][A-Za-z0-9_]*)\s*:"#).expect("regex");

    while let Some((start, start_tag, end_tag)) = find_next_tag_bounds(rest) {
        let after_start = &rest[start + start_tag.len()..];
        let (block, consumed) = if let Some(end) = after_start.find(end_tag) {
            (&after_start[..end], end + end_tag.len())
        } else if let Some((fallback, consumed)) = extract_balanced_call_block(after_start) {
            (fallback, consumed)
        } else {
            break;
        };
        rest = &after_start[consumed..];

        if let Some(call) = parse_tagged_tool_call_block(block, &key_re) {
            out.push(call);
        }
    }

    Ok(out)
}

pub fn extract_partial_execute_shell_command(input: &str) -> Option<ToolCall> {
    let start = input.find("call:execute_shell_command")?;
    let tail = &input[start..];
    let cmd_pos = tail.find("cmd:")?;
    let after_cmd = &tail[cmd_pos + 4..];

    for marker in ["<|\"|>", "<|\\\"|>"] {
        if let Some(open) = after_cmd.find(marker) {
            let rest = &after_cmd[open + marker.len()..];
            let candidate = if let Some(close) = rest.find(marker) {
                rest[..close].trim()
            } else {
                let mut cutoff = rest.len();
                for delim in [
                    ",commands_used",
                    ",expected_effect",
                    ",intent",
                    "<tool_call",
                    "<|/tool_call|>",
                    "\n",
                    "}",
                    ",",
                ] {
                    if let Some(pos) = rest.find(delim) {
                        cutoff = cutoff.min(pos);
                    }
                }
                rest[..cutoff].trim()
            };
            if !candidate.is_empty() {
                return Some(ToolCall {
                    name: "execute_shell_command".to_string(),
                    arguments: serde_json::json!({ "cmd": candidate }),
                });
            }
        }
    }

    None
}

pub fn parse_json_tool_call(input: &str) -> Result<ToolCall> {
    let candidate = extract_best_json_candidate(input).unwrap_or_else(|| input.trim().to_string());
    let value: Value = serde_json::from_str(&candidate)
        .or_else(|_| repair_and_parse_json_object(&candidate))
        .or_else(|_| repair_and_parse_json_object(input))?;

    let name = value
        .get("tool")
        .or_else(|| value.get("name"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing tool/name field"))?
        .to_string();

    let arguments = value
        .get("arguments")
        .or_else(|| value.get("args"))
        .cloned()
        .unwrap_or(Value::Object(Default::default()));

    Ok(ToolCall { name, arguments })
}

fn extract_best_json_candidate(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_string());
    }

    if let Some(start) = trimmed.find("```") {
        let rest = &trimmed[start + 3..];
        if let Some(end) = rest.find("```") {
            let block = &rest[..end];
            let block = block
                .strip_prefix("json")
                .map(str::trim_start)
                .unwrap_or(block)
                .trim();
            if block.starts_with('{') && block.ends_with('}') {
                return Some(block.to_string());
            }
        }
    }

    let mut depth = 0usize;
    let mut start_idx = None;
    for (i, c) in trimmed.char_indices() {
        if c == '{' {
            if start_idx.is_none() {
                start_idx = Some(i);
            }
            depth += 1;
        } else if c == '}' && depth > 0 {
            depth -= 1;
            if depth == 0
                && let Some(s) = start_idx
            {
                return Some(trimmed[s..=i].to_string());
            }
        }
    }
    None
}

fn repair_and_parse_json_object(input: &str) -> Result<Value> {
    let mut repaired = input.trim().to_string();
    if !repaired.starts_with('{') {
        repaired = format!("{{{repaired}}}");
    }

    repaired = repaired
        .replace("'", "\"")
        .replace(": True", ": true")
        .replace(": False", ": false");

    let v: Value = serde_json::from_str(&repaired)?;
    Ok(v)
}

fn sanitize_tagged_input(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect()
}

fn find_next_tag_bounds(input: &str) -> Option<(usize, &'static str, &'static str)> {
    const TAGS: &[(&str, &str)] = &[
        ("<|tool_call>", "<tool_call|>"),
        ("<tool_call>", "</tool_call>"),
        ("<|tool_call|>", "<|/tool_call|>"),
    ];

    let mut best: Option<(usize, &'static str, &'static str)> = None;
    for (start, end) in TAGS {
        if let Some(idx) = input.find(start) {
            match best {
                Some((best_idx, _, _)) if best_idx <= idx => {}
                _ => best = Some((idx, *start, *end)),
            }
        }
    }
    best
}

fn extract_balanced_call_block(input: &str) -> Option<(&str, usize)> {
    let block = input.trim_start();
    if !block.starts_with("call:") {
        return None;
    }
    let start_offset = input.len().saturating_sub(block.len());
    let brace_start = block.find('{')?;
    let mut depth = 0usize;
    for (idx, ch) in block.char_indices().skip(brace_start) {
        match ch {
            '{' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = idx + 1;
                    let consumed = start_offset + end;
                    return Some((&block[..end], consumed));
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_tagged_tool_call_block(block: &str, key_re: &Regex) -> Option<ToolCall> {
    let block = block.strip_prefix("call:")?;
    let brace = block.find('{')?;
    let name = block[..brace].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let args_blob = block[brace + 1..].trim();
    let body = args_blob.strip_suffix('}')?;

    let normalized = body
        .replace("<|\\\"|>", "\"")
        .replace("<|\"|>", "\"")
        .replace('\n', " ")
        .trim()
        .to_string();

    let quoted_keys = key_re.replace_all(&normalized, "\"$1\":");
    let as_json = format!("{{{quoted_keys}}}");
    let value: Value = serde_json::from_str(&as_json)
        .or_else(|_| repair_and_parse_json_object(&as_json))
        .ok()?;

    Some(ToolCall {
        name,
        arguments: value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tagged_calls() {
        let text = r#"<|tool_call>call:execute_shell_command{cmd:<|"|>ls -la<|"|>}<tool_call|>"#;
        let calls = parse_tagged_tool_calls(text).expect("parse");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "execute_shell_command");
        assert_eq!(calls[0].arguments["cmd"], "ls -la");
    }

    #[test]
    fn parse_tagged_calls_supports_alternate_tags() {
        let text = r#"<tool_call>call:lookup_command_docs{name:<|"|>git<|"|>}</tool_call>"#;
        let calls = parse_tagged_tool_calls(text).expect("parse alt tags");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "lookup_command_docs");
        assert_eq!(calls[0].arguments["name"], "git");
    }

    #[test]
    fn parse_tagged_calls_tolerates_control_bytes() {
        let text = format!(
            "{}<|tool_call>call:execute_shell_command{{cmd:<|\"|>pwd<|\"|>}}<tool_call|>{}",
            '\u{8}', '\u{7}'
        );
        let calls = parse_tagged_tool_calls(&text).expect("parse with controls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["cmd"], "pwd");
    }

    #[test]
    fn parse_tagged_calls_handles_command_metadata_fields() {
        let text = r#"<|tool_call>call:execute_shell_command{cmd:<|"|>ls -1 | head -n 5<|"|>,commands_used:[<|"|>ls -1 | head -n 5<|"|>],expected_effect:<|"|>List the names of the first five files/directories in the current working directory, one per line.<|"|>,intent:<|"|>List the first five files in the repository<|"|>}<tool_call|>"#;
        let calls = parse_tagged_tool_calls(text).expect("parse metadata fields");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "execute_shell_command");
        assert_eq!(calls[0].arguments["cmd"], "ls -1 | head -n 5");
    }

    #[test]
    fn parse_tagged_calls_without_closing_tag() {
        let text = r#"<|tool_call>call:execute_shell_command{cmd:<|"|>pwd<|"|>,intent:<|"|>print working directory<|"|>}"#;
        let calls = parse_tagged_tool_calls(text).expect("parse missing closing tag");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "execute_shell_command");
        assert_eq!(calls[0].arguments["cmd"], "pwd");
    }

    #[test]
    fn extract_partial_execute_shell_command_handles_unterminated_payload() {
        let text = r#"<|tool_call>call:execute_shell_command{cmd:<|"|>ls -1 | head -n 5"#;
        let call = extract_partial_execute_shell_command(text).expect("extract partial command");
        assert_eq!(call.name, "execute_shell_command");
        assert_eq!(call.arguments["cmd"], "ls -1 | head -n 5");
    }

    #[test]
    fn parse_json_call() {
        let call = parse_json_tool_call(
            r#"{"tool":"execute_shell_command","arguments":{"cmd":"echo hi"}}"#,
        )
        .expect("json");
        assert_eq!(call.name, "execute_shell_command");
        assert_eq!(call.arguments["cmd"], "echo hi");
    }

    #[test]
    fn parse_json_call_from_fenced_text() {
        let call = parse_json_tool_call(
            r#"Here is the tool call:
```json
{"name":"lookup_command_docs","arguments":{"name":"git"}}
```"#,
        )
        .expect("fenced json");
        assert_eq!(call.name, "lookup_command_docs");
        assert_eq!(call.arguments["name"], "git");
    }
}
