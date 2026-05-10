use crate::redaction::redact_secrets;
use crate::text_detection::{TextDetectionOptions, detect_plaintext_like_with_options};
use anyhow::Result;
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFilesResult {
    pub cwd: String,
    pub root: String,
    pub workspace_root: String,
    pub matches: Vec<FileMatch>,
    pub scanned_files: usize,
    pub plaintext_files_scanned: usize,
    pub skipped_binary_files: usize,
    pub scanned_bytes: usize,
    pub files_truncated_by_size: usize,
    pub truncated: bool,
    pub detector: String,
    pub encoding: String,
}

#[derive(Debug, Clone)]
pub struct SearchFilesOptions<'a> {
    pub glob: Option<&'a str>,
    pub regex_mode: bool,
    pub max_results: usize,
    pub max_files_scanned: usize,
    pub max_bytes_per_file: usize,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    pub text_detection: TextDetectionOptions,
}

impl Default for SearchFilesOptions<'_> {
    fn default() -> Self {
        Self {
            glob: None,
            regex_mode: false,
            max_results: 100,
            max_files_scanned: 1000,
            max_bytes_per_file: 65_536,
            include_hidden: false,
            respect_gitignore: true,
            text_detection: TextDetectionOptions::default(),
        }
    }
}

pub fn search_files(
    root: &Path,
    query: &str,
    opts: SearchFilesOptions<'_>,
) -> Result<SearchFilesResult> {
    let mut matches = Vec::new();
    let mut scanned_files = 0usize;
    let mut plaintext_files_scanned = 0usize;
    let mut skipped_binary_files = 0usize;
    let mut scanned_bytes = 0usize;
    let mut files_truncated_by_size = 0usize;
    let mut encodings = BTreeSet::<String>::new();
    let query_regex = if opts.regex_mode {
        Some(Regex::new(query)?)
    } else {
        None
    };
    let glob_regex = opts.glob.map(glob_to_regex).transpose()?;

    let mut walker = WalkBuilder::new(root);
    walker.hidden(!opts.include_hidden);
    walker.ignore(true);
    walker.parents(true);
    walker.git_ignore(opts.respect_gitignore);
    walker.git_global(opts.respect_gitignore);
    walker.git_exclude(opts.respect_gitignore);
    walker.require_git(false);

    for entry in walker.build().flatten() {
        if !entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
            continue;
        }

        if scanned_files >= opts.max_files_scanned {
            return Ok(SearchFilesResult {
                cwd: std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| String::new()),
                root: root.display().to_string(),
                workspace_root: root.display().to_string(),
                matches,
                scanned_files,
                plaintext_files_scanned,
                skipped_binary_files,
                scanned_bytes,
                files_truncated_by_size,
                truncated: true,
                detector: "content_sniff_v1".to_string(),
                encoding: summarize_encodings(&encodings),
            });
        }

        scanned_files += 1;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        if let Some(glob_re) = &glob_regex
            && !glob_re.is_match(&rel)
        {
            continue;
        }
        let Ok(mut file) = File::open(path) else {
            continue;
        };
        let max_bytes = opts.max_bytes_per_file.max(1);
        let mut data = Vec::with_capacity(max_bytes.min(8192).saturating_add(1));
        let Ok(read_len) = file
            .by_ref()
            .take((max_bytes as u64).saturating_add(1))
            .read_to_end(&mut data)
        else {
            continue;
        };
        if read_len > max_bytes {
            data.truncate(max_bytes);
            files_truncated_by_size = files_truncated_by_size.saturating_add(1);
        }
        scanned_bytes = scanned_bytes.saturating_add(data.len());

        let detection = detect_plaintext_like_with_options(&data, &opts.text_detection);
        if !detection.plaintext_like {
            skipped_binary_files = skipped_binary_files.saturating_add(1);
            continue;
        }
        plaintext_files_scanned = plaintext_files_scanned.saturating_add(1);
        encodings.insert(detection.encoding);

        let text = String::from_utf8_lossy(&data);
        for (idx, line) in text.lines().enumerate() {
            let is_match = if let Some(re) = &query_regex {
                re.is_match(line)
            } else {
                line.contains(query)
            };
            if is_match {
                matches.push(FileMatch {
                    path: path.display().to_string(),
                    line: idx + 1,
                    text: redact_secrets(line),
                });
                if matches.len() >= opts.max_results {
                    return Ok(SearchFilesResult {
                        cwd: std::env::current_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| String::new()),
                        root: root.display().to_string(),
                        workspace_root: root.display().to_string(),
                        matches,
                        scanned_files,
                        plaintext_files_scanned,
                        skipped_binary_files,
                        scanned_bytes,
                        files_truncated_by_size,
                        truncated: true,
                        detector: "content_sniff_v1".to_string(),
                        encoding: summarize_encodings(&encodings),
                    });
                }
            }
        }
    }

    Ok(SearchFilesResult {
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| String::new()),
        root: root.display().to_string(),
        workspace_root: root.display().to_string(),
        matches,
        scanned_files,
        plaintext_files_scanned,
        skipped_binary_files,
        scanned_bytes,
        files_truncated_by_size,
        truncated: files_truncated_by_size > 0,
        detector: "content_sniff_v1".to_string(),
        encoding: summarize_encodings(&encodings),
    })
}

fn summarize_encodings(encodings: &BTreeSet<String>) -> String {
    if encodings.is_empty() {
        return "none".to_string();
    }
    if encodings.len() == 1 {
        return encodings.iter().next().cloned().unwrap_or_default();
    }
    "mixed".to_string()
}

fn glob_to_regex(glob: &str) -> Result<Regex> {
    let mut pattern = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => pattern.push_str(".*"),
            '?' => pattern.push('.'),
            '.' => pattern.push_str("\\."),
            '\\' => pattern.push_str("\\\\"),
            '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' => {
                pattern.push('\\');
                pattern.push(ch);
            }
            _ => pattern.push(ch),
        }
    }
    pattern.push('$');
    Ok(Regex::new(&pattern)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_glob_and_regex_query() {
        let root = std::env::temp_dir().join(format!(
            "termlm-search-files-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(
            root.join("src/main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .expect("write");
        std::fs::write(root.join("README.md"), "hello docs\n").expect("write");

        let result = search_files(
            &root,
            r#"println!\("hello"\)"#,
            SearchFilesOptions {
                glob: Some("src/*.rs"),
                regex_mode: true,
                max_results: 20,
                max_files_scanned: 100,
                max_bytes_per_file: 8192,
                include_hidden: false,
                respect_gitignore: true,
                text_detection: TextDetectionOptions::default(),
            },
        )
        .expect("search");
        assert_eq!(result.matches.len(), 1);
        assert!(result.matches[0].path.ends_with("src/main.rs"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn can_ignore_gitignore_when_requested() {
        let root = std::env::temp_dir().join(format!(
            "termlm-search-files-gitignore-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ignored")).expect("mkdir");
        std::fs::write(root.join(".gitignore"), "ignored/\n").expect("write");
        std::fs::write(root.join("ignored/secret.txt"), "needle\n").expect("write");

        let respected = search_files(
            &root,
            "needle",
            SearchFilesOptions {
                max_results: 20,
                max_files_scanned: 100,
                max_bytes_per_file: 8192,
                include_hidden: false,
                respect_gitignore: true,
                ..SearchFilesOptions::default()
            },
        )
        .expect("search respected");
        assert!(respected.matches.is_empty());

        let ignored = search_files(
            &root,
            "needle",
            SearchFilesOptions {
                max_results: 20,
                max_files_scanned: 100,
                max_bytes_per_file: 8192,
                include_hidden: false,
                respect_gitignore: false,
                ..SearchFilesOptions::default()
            },
        )
        .expect("search ignored");
        assert_eq!(ignored.matches.len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn truncates_large_files_per_file_budget() {
        let root = std::env::temp_dir().join(format!(
            "termlm-search-files-budget-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let large = "x".repeat(20_000) + "\nneedle\n";
        std::fs::write(root.join("large.txt"), large).expect("write");

        let out = search_files(
            &root,
            "needle",
            SearchFilesOptions {
                max_results: 20,
                max_files_scanned: 100,
                max_bytes_per_file: 128,
                include_hidden: false,
                respect_gitignore: true,
                ..SearchFilesOptions::default()
            },
        )
        .expect("search");
        assert!(out.truncated);
        assert!(out.files_truncated_by_size >= 1);
        assert!(out.scanned_bytes <= 128);
        assert!(out.matches.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
