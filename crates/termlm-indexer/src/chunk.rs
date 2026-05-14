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
        let document = with_usage_intent_section(command_name, raw);
        let mut sections = split_sections(&document);
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
        let hash = format!("{:x}", Sha256::digest(document.as_bytes()));
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

#[derive(Debug, Clone)]
struct OptionHint {
    display: String,
    flags: Vec<String>,
    description: String,
}

fn with_usage_intent_section(command_name: &str, raw: &str) -> String {
    if raw.contains("\nUSAGE INTENTS\n") {
        return raw.to_string();
    }
    let Some(section) = synthesize_usage_intents(command_name, raw) else {
        return raw.to_string();
    };
    let mut out = raw.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("USAGE INTENTS\n");
    out.push_str(&section);
    out.push('\n');
    out
}

fn synthesize_usage_intents(command_name: &str, raw: &str) -> Option<String> {
    let options = extract_option_hints(raw);
    let task_phrases = command_task_phrases(command_name);
    let related = related_command_notes(command_name);
    if options.is_empty() && task_phrases.is_empty() && related.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("Generated retrieval hints from local command documentation and common command task phrases. ");
    out.push_str(
        "These examples are bounded, not exhaustive; validate exact flags against OPTIONS.\n",
    );
    out.push_str("Command: ");
    out.push_str(command_name);
    out.push('\n');

    if !task_phrases.is_empty() {
        out.push_str("\nCommand task phrases:\n");
        for phrase in task_phrases {
            out.push_str("- ");
            out.push_str(phrase);
            out.push('\n');
        }
    }

    if !options.is_empty() {
        out.push_str("\nOption intent hints:\n");
        for hint in options.iter().take(24) {
            out.push_str("- `");
            out.push_str(command_name);
            out.push(' ');
            out.push_str(&hint.display);
            out.push_str("`: ");
            out.push_str(&trim_sentence(&hint.description, 220));
            let keywords = intent_keywords_for_option(command_name, hint);
            if !keywords.is_empty() {
                out.push_str(" Useful for requests about ");
                out.push_str(&keywords.join(", "));
                out.push('.');
            }
            out.push('\n');
        }
    }

    let combinations = command_option_combinations(command_name, &options);
    if !combinations.is_empty() {
        out.push_str("\nCommon bounded combinations:\n");
        for line in combinations.into_iter().take(10) {
            out.push_str("- ");
            out.push_str(&line);
            out.push('\n');
        }
    }

    if !related.is_empty() {
        out.push_str("\nSimilar command differences:\n");
        for line in related {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
    }

    Some(out.trim().to_string())
}

fn extract_option_hints(raw: &str) -> Vec<OptionHint> {
    let mut hints = Vec::<OptionHint>::new();
    let mut current: Option<OptionHint> = None;

    for line in raw.lines() {
        if let Some(next) = parse_option_hint_line(line) {
            if let Some(done) = current.take()
                && useful_option_hint(&done)
            {
                hints.push(done);
            }
            current = Some(next);
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() || looks_like_heading(line) {
            continue;
        }
        if let Some(active) = current.as_mut()
            && active.description.len() < 260
            && !trimmed.starts_with('-')
        {
            if !active.description.is_empty() {
                active.description.push(' ');
            }
            active.description.push_str(trimmed);
        }
    }
    if let Some(done) = current.take()
        && useful_option_hint(&done)
    {
        hints.push(done);
    }

    let mut seen = std::collections::BTreeSet::<String>::new();
    let mut out = Vec::new();
    for hint in hints {
        let key = if hint.flags.is_empty() {
            hint.display.clone()
        } else {
            hint.flags.join("|")
        };
        if seen.insert(key) {
            out.push(hint);
        }
        if out.len() >= 48 {
            break;
        }
    }
    out
}

fn parse_option_hint_line(line: &str) -> Option<OptionHint> {
    let normalized = line.replace('\t', "    ");
    let trimmed = normalized.trim();
    if !trimmed.starts_with('-') || trimmed.starts_with("---") || trimmed.len() < 2 {
        return None;
    }

    let (option_part, description) = split_option_line(trimmed);
    let flags = extract_flags(option_part);
    if flags.is_empty() || option_part.len() > 96 {
        return None;
    }

    Some(OptionHint {
        display: option_part.trim().trim_end_matches(',').to_string(),
        flags,
        description: description.trim().to_string(),
    })
}

fn split_option_line(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let mut last_was_space = false;
    for (idx, byte) in bytes.iter().enumerate().skip(2) {
        let is_space = byte.is_ascii_whitespace();
        if is_space && last_was_space {
            let option_end = idx.saturating_sub(1);
            return (line[..option_end].trim(), line[idx + 1..].trim());
        }
        last_was_space = is_space;
    }
    (line.trim(), "")
}

fn extract_flags(option_part: &str) -> Vec<String> {
    option_part
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter_map(|token| {
            let flag = token
                .trim()
                .trim_matches(|c: char| matches!(c, '[' | ']' | '(' | ')' | ':' | ';' | ','));
            if flag.starts_with('-') && flag.chars().any(|c| c.is_ascii_alphabetic()) {
                Some(flag.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn useful_option_hint(hint: &OptionHint) -> bool {
    !hint.flags.is_empty() && !hint.description.trim().is_empty()
}

fn looks_like_heading(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && !line.starts_with(' ')
        && trimmed.chars().all(|c| !c.is_ascii_lowercase())
}

fn trim_sentence(text: &str, max_chars: usize) -> String {
    let mut cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        cleaned = "See local documentation for exact behavior.".to_string();
    }
    if cleaned.len() <= max_chars {
        return cleaned;
    }
    let mut cut = max_chars;
    while cut > 0 && !cleaned.is_char_boundary(cut) {
        cut -= 1;
    }
    cleaned.truncate(cut);
    cleaned
        .trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string()
}

fn intent_keywords_for_option(command_name: &str, hint: &OptionHint) -> Vec<&'static str> {
    let mut out = Vec::<&'static str>::new();
    let lower = format!(
        "{} {} {}",
        command_name,
        hint.display.to_ascii_lowercase(),
        hint.description.to_ascii_lowercase()
    );
    for flag in &hint.flags {
        if let Some(intent) = known_option_intent(command_name, flag) {
            push_unique(&mut out, intent);
        }
    }

    let checks: &[(&str, &str)] = &[
        ("recursive", "recursive traversal"),
        ("ignore case", "case-insensitive matching"),
        ("case-insensitive", "case-insensitive matching"),
        ("hidden", "hidden files"),
        ("all files", "hidden files"),
        ("file name", "filename matching"),
        ("filename", "filename matching"),
        ("modification", "modified time filters"),
        ("human", "human-readable output"),
        ("summary", "total summary output"),
        ("line number", "line numbers"),
        ("parents", "parent directory creation"),
        ("empty", "empty files or directories"),
        ("size", "size filters"),
        ("larger", "larger-than size filters"),
        ("newer", "modified time filters"),
        ("older", "modified time filters"),
    ];
    for (needle, label) in checks {
        if lower.contains(needle) {
            push_unique(&mut out, label);
        }
    }
    out
}

fn known_option_intent(command_name: &str, flag: &str) -> Option<&'static str> {
    let flag = flag.trim_matches(|c: char| matches!(c, '[' | ']'));
    match (command_name, flag) {
        ("ls", "-a" | "--all" | "-A" | "--almost-all") => Some("hidden files and all entries"),
        ("ls", "-l" | "--long") => Some("long detailed listings"),
        ("grep" | "rg", "-i" | "--ignore-case") => Some("case-insensitive matching"),
        ("grep" | "rg", "-n" | "--line-number") => Some("line numbers"),
        ("grep" | "rg", "-v" | "--invert-match") => Some("excluding or inverted matches"),
        ("grep" | "rg", "-E" | "--extended-regexp") => Some("extended regular expressions"),
        ("grep" | "rg", "-F" | "--fixed-strings") => Some("literal string matching"),
        ("grep" | "rg", "-H" | "--with-filename") => Some("showing filenames with matches"),
        ("grep" | "rg", "-l" | "--files-with-matches") => Some("showing filenames with matches"),
        ("grep" | "rg", "-R" | "-r" | "--recursive") => Some("recursive traversal"),
        ("du" | "df" | "ls", "--human-readable") => Some("human-readable sizes"),
        ("du", "-s" | "--summarize") => Some("total summary output"),
        ("mkdir", "-p" | "--parents") => Some("creating parent directories"),
        ("tail", "-f" | "--follow") => Some("following appended output"),
        ("tail", "-n") => Some("line counts"),
        ("find", "-name" | "--name") => Some("filename matching"),
        ("find", "-type") => Some("files only, directories only, or file type filtering"),
        ("find", "-size") => Some("file size filters"),
        ("find", "-mtime" | "-mmin" | "-newer") => Some("modified time filters"),
        ("find", "-maxdepth" | "-mindepth") => Some("depth-limited searches"),
        ("find", "-empty") => Some("empty files or directories"),
        ("find", "-exec" | "-execdir") => Some("running a command for each match"),
        ("find", "-print" | "-print0") => Some("printing matching paths"),
        _ => None,
    }
}

fn command_option_combinations(command_name: &str, options: &[OptionHint]) -> Vec<String> {
    let has = |flag: &str| {
        options
            .iter()
            .any(|hint| hint.flags.iter().any(|candidate| candidate == flag))
    };
    let mut out = Vec::<String>::new();
    match command_name {
        "find" => {
            if has("-type") && has("-name") {
                out.push("`find PATH -type f -name PATTERN`: find files only, not directories, whose filenames match a pattern.".to_string());
            }
            if has("-type") && has("-empty") {
                out.push("`find PATH -type d -empty`: find empty directories; use `-type f -empty` for empty files.".to_string());
            }
            if has("-type") && (has("-mtime") || has("-mmin")) {
                out.push(
                    "`find PATH -type f -mtime N`: find files by modification age.".to_string(),
                );
            }
            if has("-maxdepth") && has("-type") {
                out.push("`find PATH -maxdepth N -type f`: limit recursive search depth while selecting files.".to_string());
            }
            if has("-type") && has("-size") {
                out.push("`find PATH -type f -size +N`: find files larger than a size threshold; use `-size -N` for smaller files.".to_string());
            }
        }
        "grep" | "rg" => {
            if has("-r") || has("-R") || has("--recursive") {
                out.push(format!(
                    "`{command_name} -R PATTERN PATH`: recursively search file contents for text."
                ));
            }
            if has("-i") || has("--ignore-case") {
                out.push(format!(
                    "`{command_name} -i PATTERN PATH`: search text case-insensitively."
                ));
            }
            if has("-l") || has("--files-with-matches") {
                out.push(format!(
                    "`{command_name} -l PATTERN PATH`: list filenames containing matches, not matching lines."
                ));
            }
        }
        "ls" => {
            if has("-a") || has("--all") {
                out.push(
                    "`ls -la PATH`: list all entries including hidden files with long details."
                        .to_string(),
                );
            }
            if has("-S") {
                out.push(
                    "`ls -lhS PATH`: list entries sorted by size with human-readable sizes."
                        .to_string(),
                );
            }
            if has("-t") {
                out.push("`ls -lt PATH`: list entries sorted by modification time.".to_string());
            }
        }
        "du" if has("-s") && (has("-h") || has("--human-readable")) => {
            out.push(
                "`du -sh PATH`: show total disk usage for a path in human-readable form."
                    .to_string(),
            );
        }
        "mkdir" if has("-p") || has("--parents") => {
            out.push("`mkdir -p PATH`: create a directory and any missing parent directories without error if it already exists.".to_string());
        }
        "tail" => {
            if has("-n") {
                out.push(
                    "`tail -n N FILE`: show the last N lines of a file or stream.".to_string(),
                );
            }
            if has("-f") {
                out.push(
                    "`tail -f FILE`: follow a growing log file as new lines arrive.".to_string(),
                );
            }
        }
        _ => {}
    }
    out
}

fn command_task_phrases(command_name: &str) -> Vec<&'static str> {
    match command_name {
        "pwd" => vec![
            "`pwd`: print the current working directory; answer where am I, what directory am I in, and show this folder path.",
        ],
        "ls" => vec![
            "`ls`: list directory contents, files in a folder, hidden files, long details, sorted listings, and entries in a path.",
        ],
        "find" => vec![
            "`find`: search for files or directories by name, type, size, age, emptiness, depth, or other filesystem metadata.",
            "`find -type f`: select files only, not directories.",
            "`find -type d`: select directories only, not files.",
        ],
        "grep" => vec![
            "`grep`: search file contents for text, patterns, matches, case-insensitive matches, and names of files whose contents match.",
        ],
        "rg" => vec![
            "`rg`: recursively search file contents for text, patterns, matches, case-insensitive matches, and names of files whose contents match.",
        ],
        "wc" => vec![
            "`wc`: count lines, words, bytes, characters, or records in files or command output.",
        ],
        "head" => vec!["`head`: show the first lines or first bytes of a file or command output."],
        "tail" => {
            vec!["`tail`: show the last lines, follow logs, or watch appended output from files."]
        }
        "du" => vec![
            "`du`: summarize disk usage for files or directories, including total size and human-readable size.",
        ],
        "df" => vec![
            "`df`: show filesystem capacity, mounted disk free space, available space, and usage percentages.",
        ],
        "mkdir" => vec![
            "`mkdir`: create directories, including missing parent directories when requested.",
        ],
        "touch" => {
            vec!["`touch`: create empty files or update file access and modification timestamps."]
        }
        "cp" => vec!["`cp`: copy files or directories, preserving the original source."],
        "mv" => vec!["`mv`: move files, move directories, or rename paths."],
        "rm" => {
            vec!["`rm`: remove files or directories; recursive directory removal is destructive."]
        }
        "cat" => vec!["`cat`: print, concatenate, or inspect complete file contents."],
        "sort" => vec![
            "`sort`: sort lines alphabetically, numerically, reverse order, or by selected keys.",
        ],
        "uniq" => vec![
            "`uniq`: collapse adjacent duplicate lines; usually pair with `sort` for global duplicate removal.",
        ],
        "date" => vec![
            "`date`: print or format the current date and time, including ISO dates like YYYY-MM-DD.",
        ],
        "chmod" => vec!["`chmod`: change file permission bits or executable permissions."],
        "chown" => vec!["`chown`: change file owner or group."],
        "tar" => {
            vec!["`tar`: create, extract, or list archive files such as .tar, .tar.gz, and .tgz."]
        }
        "curl" => vec![
            "`curl`: make HTTP requests, download URLs, send headers, inspect response codes, or post data.",
        ],
        "sed" => vec![
            "`sed`: transform text streams, especially substitutions, deletions, and line-based edits.",
        ],
        "awk" => vec![
            "`awk`: process structured text by fields and records, including filtering and calculations.",
        ],
        _ => Vec::new(),
    }
}

fn related_command_notes(command_name: &str) -> Vec<&'static str> {
    match command_name {
        "find" => vec![
            "`find` locates filesystem paths by metadata such as name, type, size, and time; use `grep` or `rg` to search file contents.",
            "`find` walks directories recursively by default; `ls` lists directory entries without predicate filtering.",
        ],
        "grep" => vec![
            "`grep` searches file contents; use `find` to search by filename or filesystem metadata.",
            "`rg` is a faster recursive grep-like searcher with smart defaults when installed.",
        ],
        "rg" => vec![
            "`rg` searches file contents recursively with ignore-file awareness; `grep` is more universally available.",
            "`find` searches paths and metadata, not text contents.",
        ],
        "ls" => vec![
            "`ls` lists directory entries; use `find` for recursive predicate-based filesystem searches.",
            "`du` reports disk usage; `ls -lh` reports apparent file sizes in listings.",
        ],
        "du" => vec![
            "`du` estimates disk usage of paths; `df` reports filesystem free space and capacity.",
            "`ls -lh` shows file sizes in listings but does not summarize directory disk usage.",
        ],
        "df" => vec![
            "`df` reports filesystem capacity and free space; `du` estimates usage for specific paths.",
        ],
        "head" => vec![
            "`head` shows the first lines; `tail` shows the last lines or follows appended log output.",
        ],
        "tail" => {
            vec!["`tail` shows the last lines or follows logs; `head` shows the first lines."]
        }
        "mkdir" => {
            vec!["`mkdir` creates directories; `touch` creates files or updates timestamps."]
        }
        "touch" => {
            vec!["`touch` creates files or updates timestamps; `mkdir` creates directories."]
        }
        "cp" => vec!["`cp` copies files; `mv` moves or renames files."],
        "mv" => vec!["`mv` moves or renames files; `cp` leaves the source in place."],
        "rm" => vec!["`rm` removes files; `rmdir` removes empty directories only."],
        "wc" => vec![
            "`wc` counts lines, words, and bytes; `grep -c` counts matching lines for a pattern.",
        ],
        "sort" => vec![
            "`sort` orders lines; `uniq` only collapses adjacent duplicate lines, usually after sorting.",
        ],
        "uniq" => vec![
            "`uniq` collapses adjacent duplicate lines; use `sort` first when duplicates are not already grouped.",
        ],
        "date" => vec!["`date` formats dates and times; `time` measures how long a command takes."],
        "cat" => {
            vec!["`cat` prints whole files; `head` and `tail` show only the beginning or end."]
        }
        "sed" => vec![
            "`sed` edits streams line by line, especially substitutions; `awk` is better for field-aware records and calculations.",
        ],
        "awk" => {
            vec!["`awk` processes fields and records; `sed` is lighter for simple substitutions."]
        }
        _ => Vec::new(),
    }
}

fn push_unique<'a>(out: &mut Vec<&'a str>, value: &'a str) {
    if !out.contains(&value) {
        out.push(value);
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
    fn usage_intents_are_synthesized_from_options() {
        let c = Chunker::new(2048);
        let raw = "NAME\nfind - walk a file hierarchy\n\nOPTIONS\n  -name pattern  Base of file name matches shell pattern.\n  -type t        File is of type t. Type f is a regular file and d is a directory.\n  -empty         True if the current file or directory is empty.\n";
        let chunks = c.chunk_document("find", "/usr/bin/find", "man", raw);
        let usage = chunks
            .iter()
            .find(|chunk| chunk.section_name == "USAGE INTENTS")
            .expect("usage intents chunk");
        assert!(usage.text.contains("files only, not directories"));
        assert!(usage.text.contains("filename"));
        assert!(usage.text.contains("Similar command differences"));
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
