use crate::chunk::Chunk;

#[derive(Debug, Clone)]
pub struct LookupResult {
    pub name: String,
    pub section: Option<String>,
    pub text: String,
    pub truncated: bool,
}

pub fn lookup_command_docs(
    chunks: &[Chunk],
    name: &str,
    section: Option<&str>,
    max_bytes: usize,
) -> Result<LookupResult, Vec<String>> {
    let mut matching: Vec<&Chunk> = chunks.iter().filter(|c| c.command_name == name).collect();
    if matching.is_empty() {
        let suggestions = fuzzy_suggestions(chunks, name, 5);
        return Err(suggestions);
    }
    matching.sort_by_key(|c| c.chunk_index);

    let selected: Vec<&Chunk> = if let Some(section_name) = section {
        let section_hits = matching
            .iter()
            .copied()
            .filter(|c| c.section_name.eq_ignore_ascii_case(section_name))
            .collect::<Vec<_>>();
        if section_hits.is_empty() {
            first_chunk_per_section(&matching)
        } else {
            section_hits
        }
    } else {
        first_chunk_per_section(&matching)
    };

    let mut out = String::new();
    for chunk in selected {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("### ");
        out.push_str(&chunk.section_name);
        out.push('\n');
        out.push_str(&chunk.text);
    }

    let mut truncated = false;
    if out.len() > max_bytes {
        out.truncate(max_bytes);
        out.push_str("\n… [truncated; specify section= for more]");
        truncated = true;
    }

    Ok(LookupResult {
        name: name.to_string(),
        section: section.map(|s| s.to_string()),
        text: out,
        truncated,
    })
}

fn first_chunk_per_section<'a>(chunks: &[&'a Chunk]) -> Vec<&'a Chunk> {
    let mut seen = std::collections::BTreeSet::<String>::new();
    let mut selected = Vec::new();
    for chunk in chunks {
        let key = chunk.section_name.to_ascii_lowercase();
        if seen.insert(key) {
            selected.push(*chunk);
        }
    }
    selected
}

fn fuzzy_suggestions(chunks: &[Chunk], needle: &str, limit: usize) -> Vec<String> {
    let mut names: Vec<String> = chunks.iter().map(|c| c.command_name.clone()).collect();
    names.sort();
    names.dedup();

    names.sort_by_key(|name| levenshtein(name, needle));
    names.into_iter().take(limit).collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let mut costs: Vec<usize> = (0..=b.chars().count()).collect();

    for (i, ca) in a.chars().enumerate() {
        let mut prev_diag = i;
        costs[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let temp = costs[j + 1];
            let replace_cost = if ca == cb { prev_diag } else { prev_diag + 1 };
            let insert_cost = costs[j + 1] + 1;
            let delete_cost = costs[j] + 1;
            costs[j + 1] = replace_cost.min(insert_cost).min(delete_cost);
            prev_diag = temp;
        }
    }

    costs[b.chars().count()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn falls_back_to_first_chunk_per_section_when_section_missing() {
        let chunks = vec![
            Chunk {
                command_name: "git".to_string(),
                path: "/usr/bin/git".to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 3,
                doc_hash: "h1".to_string(),
                extracted_at: Utc::now(),
                text: "git - distributed version control".to_string(),
            },
            Chunk {
                command_name: "git".to_string(),
                path: "/usr/bin/git".to_string(),
                extraction_method: "man".to_string(),
                section_name: "OPTIONS".to_string(),
                chunk_index: 1,
                total_chunks: 3,
                doc_hash: "h1".to_string(),
                extracted_at: Utc::now(),
                text: "--help show help".to_string(),
            },
            Chunk {
                command_name: "git".to_string(),
                path: "/usr/bin/git".to_string(),
                extraction_method: "man".to_string(),
                section_name: "OPTIONS".to_string(),
                chunk_index: 2,
                total_chunks: 3,
                doc_hash: "h1".to_string(),
                extracted_at: Utc::now(),
                text: "--version show version".to_string(),
            },
        ];

        let found = lookup_command_docs(&chunks, "git", Some("MISSING"), 8192).expect("found");
        assert!(found.text.contains("### NAME"));
        assert!(found.text.contains("### OPTIONS"));
        assert!(!found.text.contains("--version show version"));
    }
}
