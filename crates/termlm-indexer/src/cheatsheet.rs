use crate::chunk::Chunk;

pub const STATIC_PRIORITY_COMMANDS: &[&str] = &[
    "ls", "cd", "pwd", "cat", "less", "head", "tail", "cp", "mv", "rm", "mkdir", "rmdir", "touch",
    "find", "grep", "sed", "awk", "sort", "uniq", "cut", "xargs", "tar", "gzip", "zip", "unzip",
    "curl", "wget", "ssh", "scp", "rsync", "ps", "top", "kill", "du", "df", "which", "man", "git",
    "rg", "fd", "brew", "docker", "kubectl", "python", "node", "npm", "pnpm", "cargo", "rustc",
    "make", "cmake",
];

pub fn build_cheatsheet(
    chunks: &[Chunk],
    max_commands: usize,
    aliases: &[(String, String)],
    functions: &[String],
    max_tokens: usize,
) -> String {
    let mut out = String::from("## Available commands (subset; full docs available on request)\n");
    let mut seen = std::collections::BTreeSet::new();
    let name_rows = map_name_rows(chunks);

    for cmd in STATIC_PRIORITY_COMMANDS {
        if seen.len() >= max_commands {
            break;
        }
        if let Some(synopsis) = name_rows.get(*cmd)
            && seen.insert((*cmd).to_string())
        {
            out.push_str(&format!("{cmd} — {synopsis}\n"));
        }
    }

    for chunk in chunks {
        if seen.len() >= max_commands {
            break;
        }
        if chunk.section_name != "NAME" {
            continue;
        }
        if seen.insert(chunk.command_name.clone()) {
            let synopsis = name_rows
                .get(chunk.command_name.as_str())
                .cloned()
                .unwrap_or_else(|| "no documentation available".to_string());
            out.push_str(&format!("{} — {}\n", chunk.command_name, synopsis));
        }
    }

    let mut sorted_aliases = aliases.to_vec();
    sorted_aliases.sort_by(|a, b| a.0.cmp(&b.0));

    let alias_lines = sorted_aliases
        .iter()
        .map(|(name, expansion)| {
            let mut trimmed = expansion.chars().take(80).collect::<String>();
            if expansion.chars().count() > 80 {
                trimmed.push('…');
            }
            format!("{name} = {trimmed}")
        })
        .collect::<Vec<_>>();
    let function_lines = wrap_functions(functions, 100);
    if !alias_lines.is_empty() {
        out.push_str("\n## Your aliases\n");
        out.push_str(&alias_lines.join("\n"));
        out.push('\n');
    }
    if !function_lines.is_empty() {
        out.push_str("\n## Your functions\n");
        out.push_str(&function_lines.join("\n"));
        out.push('\n');
    }

    enforce_token_budget(&out, max_tokens, &sorted_aliases, functions)
}

fn wrap_functions(functions: &[String], max_line_chars: usize) -> Vec<String> {
    if functions.is_empty() {
        return Vec::new();
    }
    let mut names = functions.to_vec();
    names.sort();
    names.dedup();

    let mut lines = Vec::new();
    let mut current = String::new();
    for name in names {
        if current.is_empty() {
            current.push_str(&name);
            continue;
        }
        if current.len() + 2 + name.len() > max_line_chars {
            lines.push(current);
            current = name;
        } else {
            current.push_str(", ");
            current.push_str(&name);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn enforce_token_budget(
    baseline: &str,
    max_tokens: usize,
    aliases: &[(String, String)],
    functions: &[String],
) -> String {
    if estimate_tokens(baseline) <= max_tokens {
        return baseline.to_string();
    }
    let commands_block = baseline
        .split("\n## Your aliases")
        .next()
        .unwrap_or(baseline)
        .split("\n## Your functions")
        .next()
        .unwrap_or(baseline)
        .to_string();

    // Truncate last sub-block first: functions, then aliases.
    let mut alias_count = aliases.len();
    let mut function_count = functions.len();

    while function_count > 0 {
        function_count -= 1;
        let candidate = build_with_counts(
            &commands_block,
            aliases,
            alias_count,
            functions,
            function_count,
        );
        if estimate_tokens(&candidate) <= max_tokens {
            return candidate;
        }
    }

    while alias_count > 0 {
        alias_count -= 1;
        let candidate = build_with_counts(&commands_block, aliases, alias_count, functions, 0);
        if estimate_tokens(&candidate) <= max_tokens {
            return candidate;
        }
    }

    // Last resort: hard trim keeps deterministic output.
    let max_chars = max_tokens.saturating_mul(4);
    let mut out = baseline.chars().take(max_chars).collect::<String>();
    if baseline.chars().count() > max_chars {
        out.push('\n');
        out.push_str("… (truncated for context budget)");
    }
    out
}

fn map_name_rows(chunks: &[Chunk]) -> std::collections::BTreeMap<&str, String> {
    let mut rows = std::collections::BTreeMap::new();
    for chunk in chunks {
        if chunk.section_name != "NAME" {
            continue;
        }
        rows.entry(chunk.command_name.as_str()).or_insert_with(|| {
            chunk
                .text
                .lines()
                .next()
                .unwrap_or("no documentation available")
                .chars()
                .take(90)
                .collect::<String>()
        });
    }
    rows
}

fn build_with_counts(
    commands_block: &str,
    aliases: &[(String, String)],
    alias_count: usize,
    functions: &[String],
    function_count: usize,
) -> String {
    let mut out = commands_block.to_string();
    if alias_count > 0 {
        out.push_str("\n## Your aliases\n");
        for (idx, (name, expansion)) in aliases.iter().take(alias_count).enumerate() {
            let mut trimmed = expansion.chars().take(80).collect::<String>();
            if expansion.chars().count() > 80 {
                trimmed.push('…');
            }
            if idx > 0 {
                out.push('\n');
            }
            out.push_str(&format!("{name} = {trimmed}"));
        }
        if alias_count < aliases.len() {
            out.push_str(&format!(
                "\n… (and {} more aliases)",
                aliases.len() - alias_count
            ));
        }
        out.push('\n');
    }
    if function_count > 0 {
        let function_lines = wrap_functions(&functions[..function_count], 100);
        out.push_str("\n## Your functions\n");
        out.push_str(&function_lines.join("\n"));
        if function_count < functions.len() {
            out.push_str(&format!(
                "\n… (and {} more functions)",
                functions.len() - function_count
            ));
        }
        out.push('\n');
    }
    out
}

fn estimate_tokens(text: &str) -> usize {
    text.chars().count().saturating_add(3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn includes_aliases_and_functions_blocks() {
        let chunks = vec![Chunk {
            command_name: "ls".to_string(),
            path: "/bin/ls".to_string(),
            extraction_method: "man".to_string(),
            section_name: "NAME".to_string(),
            chunk_index: 0,
            total_chunks: 1,
            doc_hash: "h".to_string(),
            extracted_at: Utc::now(),
            text: "ls - list directory contents".to_string(),
        }];

        let out = build_cheatsheet(
            &chunks,
            10,
            &[("ll".to_string(), "ls -lah".to_string())],
            &["mkcd".to_string()],
            5500,
        );
        assert!(out.contains("## Your aliases"));
        assert!(out.contains("ll = ls -lah"));
        assert!(out.contains("## Your functions"));
        assert!(out.contains("mkcd"));
    }
}
