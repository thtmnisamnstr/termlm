use crate::ToolCall;
use anyhow::{Result, anyhow};
use regex::Regex;
use serde_json::Value;

pub fn parse_tagged_tool_calls(input: &str) -> Result<Vec<ToolCall>> {
    let mut out = Vec::new();
    let mut rest = input;
    let key_re = Regex::new(r#"([A-Za-z_][A-Za-z0-9_]*)\s*:"#).expect("regex");

    while let Some(start) = rest.find("<|tool_call>") {
        let after_start = &rest[start + "<|tool_call>".len()..];
        let Some(end) = after_start.find("<tool_call|>") else {
            break;
        };
        let block = after_start[..end].trim();
        rest = &after_start[end + "<tool_call|>".len()..];

        let block = block
            .strip_prefix("call:")
            .ok_or_else(|| anyhow!("missing call: prefix"))?;
        let brace = block
            .find('{')
            .ok_or_else(|| anyhow!("missing args object"))?;
        let name = block[..brace].trim().to_string();
        let args_blob = block[brace + 1..].trim();
        let body = args_blob
            .strip_suffix('}')
            .ok_or_else(|| anyhow!("missing closing }}"))?;

        let normalized = body
            .replace("<|\"|>", "\"")
            .replace("\n", " ")
            .trim()
            .to_string();

        let quoted_keys = key_re.replace_all(&normalized, "\"$1\":");
        let as_json = format!("{{{quoted_keys}}}");
        let value: Value =
            serde_json::from_str(&as_json).or_else(|_| repair_and_parse_json_object(&as_json))?;

        out.push(ToolCall {
            name,
            arguments: value,
        });
    }

    Ok(out)
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
