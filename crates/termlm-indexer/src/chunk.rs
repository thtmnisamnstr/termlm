use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub command_name: String,
    pub path: String,
    pub extraction_method: String,
    pub section_name: String,
    pub chunk_index: usize,
    pub total_chunks: usize,
    pub doc_hash: String,
    pub extracted_at: DateTime<Utc>,
    pub text: String,
}

#[derive(Debug, Default, Clone)]
pub struct Chunker {
    pub max_section_chars: usize,
}

impl Chunker {
    pub fn new(max_section_chars: usize) -> Self {
        Self { max_section_chars }
    }

    pub fn chunk_document(
        &self,
        command_name: &str,
        source_path: &str,
        extraction_method: &str,
        raw: &str,
    ) -> Vec<Chunk> {
        let mut sections = split_sections(raw);
        if sections.is_empty() {
            sections.push((
                "NAME".to_string(),
                format!("{command_name} - no documentation available"),
            ));
        }

        // Ensure NAME appears first to keep a predictable cheat-sheet summary row.
        sections.sort_by_key(|(name, _)| {
            if name.eq_ignore_ascii_case("NAME") {
                0
            } else {
                1
            }
        });

        let mut outputs = Vec::new();
        for (section, body) in sections {
            if body.len() <= self.max_section_chars {
                outputs.push((section.clone(), body));
            } else {
                for part in split_large_text(&body, self.max_section_chars) {
                    outputs.push((section.clone(), part));
                }
            }
        }

        let total = outputs.len();
        let hash = format!("{:x}", Sha256::digest(raw.as_bytes()));
        let extracted_at = Utc::now();
        outputs
            .into_iter()
            .enumerate()
            .map(|(i, (section, text))| Chunk {
                command_name: command_name.to_string(),
                path: source_path.to_string(),
                extraction_method: extraction_method.to_string(),
                section_name: section,
                chunk_index: i,
                total_chunks: total,
                doc_hash: hash.clone(),
                extracted_at,
                text,
            })
            .collect()
    }
}

fn split_sections(raw: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut buf = String::new();

    for line in raw.lines() {
        let is_heading = !line.is_empty()
            && !line.starts_with(' ')
            && line.chars().all(|c| !c.is_ascii_lowercase());

        if is_heading {
            if let Some(name) = current_name.take() {
                sections.push((name, buf.trim().to_string()));
                buf.clear();
            }
            current_name = Some(line.trim().to_string());
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }

    if let Some(name) = current_name {
        sections.push((name, buf.trim().to_string()));
    }

    sections
}

fn split_large_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for para in text.split("\n\n") {
        let parts = if para.len() > max_chars {
            split_sentences(para)
        } else {
            vec![para.to_string()]
        };
        for part in parts {
            if current.len() + part.len() + 2 > max_chars && !current.is_empty() {
                out.push(current.trim().to_string());
                current.clear();
            }
            current.push_str(&part);
            current.push_str("\n\n");
        }
    }

    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }

    out
}

fn split_sentences(paragraph: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let chars = paragraph.chars().peekable();

    for ch in chars {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | ';') {
            out.push(current.trim().to_string());
            current.clear();
        }
    }

    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_by_headings() {
        let c = Chunker::new(128);
        let raw = "NAME\nfoo - test\n\nOPTIONS\n-a\n-b\n";
        let chunks = c.chunk_document("foo", "/bin/foo", "man", raw);
        assert!(chunks.iter().any(|x| x.section_name == "NAME"));
        assert!(chunks.iter().any(|x| x.section_name == "OPTIONS"));
        assert!(chunks.iter().all(|x| x.path == "/bin/foo"));
    }

    #[test]
    fn long_paragraph_splits_on_sentence_boundaries() {
        let c = Chunker::new(40);
        let raw = "DESCRIPTION\nfirst sentence. second sentence! third sentence?\n";
        let chunks = c.chunk_document("foo", "/bin/foo", "man", raw);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|x| x.section_name == "DESCRIPTION"));
    }
}
