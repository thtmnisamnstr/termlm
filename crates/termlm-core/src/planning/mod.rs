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

    findings.extend(validate_prompt_effects(cmd, &ctx.prompt));

    findings
}

pub fn validate_prompt_effects(command: &str, prompt: &str) -> Vec<ValidationFinding> {
    let mut findings = Vec::new();
    let p = prompt.to_ascii_lowercase();
    let c = command.to_ascii_lowercase();

    if prompt_requests_plain_listing(&p) {
        let overfiltered = c.contains(" | grep ")
            || c.contains(" | awk ")
            || c.contains(" | sed ")
            || c.contains(" | xargs ")
            || (c.starts_with("find ") && c.contains("-type f"));
        if overfiltered {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail:
                    "plain listing request should not add filtering, recursion, or transformation"
                        .to_string(),
            });
        }
    }

    if prompt_requests_largest_files(&p) && !command_satisfies_largest_files(command, &p) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "largest-file request needs size sorting or filtering and should limit results when a count is requested".to_string(),
        });
    }

    if prompt_requests_ranked_single_file(&p) && !command_selects_ranked_single_file(&c, &p) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "oldest/newest-file request should select one regular file path using mtime sorting, not dump a directory listing".to_string(),
        });
    }

    if prompt_requests_directories_only(&p) && !command_lists_directories_only(&c) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "directories-only request must select directories rather than all entries"
                .to_string(),
        });
    }

    if prompt_requests_delete_empty_directories(&p) && !command_deletes_empty_directories(&c) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail:
                "empty-directory deletion request must find empty directories and delete/rmdir them"
                    .to_string(),
        });
    }

    if prompt_requests_recent_file_per_subdir(&p) && !command_checks_recent_file_per_subdir(&c) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "per-subdirectory newest-file request needs per-directory traversal and modification-time sorting".to_string(),
        });
    }

    if prompt_requests_git_changed_files(&p) && !command_checks_git_changes(&c) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "git changed-files request should use git status or git diff".to_string(),
        });
    }

    if prompt_requests_filesystem_disk_usage(&p) && !command_checks_filesystem_disk_usage(&c) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "filesystem disk-usage request should use df, not a directory listing"
                .to_string(),
        });
    }

    if prompt_requests_dated_backup(&p) && !command_creates_dated_backup(&c) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "dated backup request should create a backup/archive and include today's date"
                .to_string(),
        });
    }

    if prompt_requests_create_and_cd(&p) && !command_creates_and_enters_directory(&c) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "create-and-cd request must both create the directory and cd into it"
                .to_string(),
        });
    }

    if (p.contains("how many") || p.contains("count")) && p.contains("file") && !c.contains("wc -l")
    {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "file-count request needs a count-producing command such as wc -l".to_string(),
        });
    }
    if (p.contains("how many") || p.contains("count")) && p.contains("file") {
        let counts_regular_files = c.starts_with("find ") && c.contains("-type f");
        let counts_regular_files_with_listing =
            c.contains(" | grep ") && (c.contains("grep -v '/$'") || c.contains("grep -v \"/$\""));
        if !counts_regular_files && !counts_regular_files_with_listing {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail:
                    "file-count request must count regular files, not directories or all entries"
                        .to_string(),
            });
        }
    }

    if prompt_requests_content_search(&p) {
        if !command_has_content_search(&c) {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail: "text-search request needs a content-search command such as grep or rg"
                    .to_string(),
            });
        }
        if prompt_requests_case_insensitive_search(&p) && !command_has_case_insensitive_search(&c) {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail: "case-insensitive text search needs an ignore-case flag".to_string(),
            });
        }
    }

    if prompt_mentions_image_files(&p) && prompt_requests_file_listing_or_transfer(&p) {
        if !command_has_image_filter(&c) {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail: "image-file request needs an image extension filter".to_string(),
            });
        }
        if c.starts_with("find ") && !c.contains("-type f") {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail: "image-file request must restrict results to regular files".to_string(),
            });
        }
    }

    if prompt_mentions_markdown_files(&p) && prompt_requests_file_listing_or_transfer(&p) {
        if !command_has_markdown_filter(&c) {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail: "Markdown-file request needs a .md or .markdown filter".to_string(),
            });
        }
        if c.starts_with("find ") && !c.contains("-type f") {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail: "Markdown-file request must restrict results to regular files".to_string(),
            });
        }
    }

    if prompt_requests_recursive_scope(&p)
        && prompt_requests_file_listing_or_transfer(&p)
        && !command_has_recursive_selection(&c)
    {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "recursive request has no recursive traversal or selection step".to_string(),
        });
    }

    if prompt_requests_compound_file_transfer(&p) {
        findings.extend(validate_compound_file_transfer_effects(&c, &p));
    }

    findings
}

fn validate_compound_file_transfer_effects(command: &str, prompt: &str) -> Vec<ValidationFinding> {
    let mut findings = Vec::new();
    let wants_copy =
        prompt_contains_word(prompt, "copy") || prompt_contains_word(prompt, "duplicate");
    let wants_move = prompt_contains_word(prompt, "move") || prompt_contains_word(prompt, "rename");
    let wants_created_destination = prompt_requests_destination_creation(prompt);
    let wants_recursive = prompt_requests_recursive_scope(prompt);

    if wants_created_destination && !command_contains_word(command, "mkdir") {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "compound request creates a destination folder but draft has no mkdir step"
                .to_string(),
        });
    }
    if wants_created_destination
        && command_contains_word(command, "mkdir")
        && (wants_copy || wants_move)
        && !command.contains("&&")
    {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "destination creation should be chained before the transfer with &&"
                .to_string(),
        });
    }
    if wants_copy && !command_has_copy_operation(command) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "compound copy request has no copy operation".to_string(),
        });
    }
    if wants_move && !command_has_move_operation(command) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "compound move request has no move operation".to_string(),
        });
    }
    if wants_recursive && !command_has_recursive_selection(command) {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "recursive request has no recursive traversal or selection step".to_string(),
        });
    }
    if prompt.contains("file") && command.contains("find ") && !command.contains("-type f") {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "file transfer should select regular files, not directories".to_string(),
        });
    }
    if command.contains("find ") && command.contains("xargs") {
        let null_safe = command.contains("-print0") && command.contains("xargs -0");
        if !null_safe && !command.contains("-exec ") {
            findings.push(ValidationFinding {
                kind: "insufficient_for_prompt".to_string(),
                detail: "find-to-xargs transfer should be null-delimited or use -exec".to_string(),
            });
        }
    }
    if prompt_mentions_standard_home_dir(prompt)
        && !command_mentions_requested_home_path(command, prompt)
    {
        findings.push(ValidationFinding {
            kind: "insufficient_for_prompt".to_string(),
            detail: "draft does not mention the requested standard home folder".to_string(),
        });
    }

    findings
}

fn prompt_requests_compound_file_transfer(prompt: &str) -> bool {
    (prompt_contains_word(prompt, "copy")
        || prompt_contains_word(prompt, "duplicate")
        || prompt_contains_word(prompt, "move")
        || prompt_contains_word(prompt, "rename"))
        && (prompt_requests_destination_creation(prompt)
            || prompt_requests_recursive_scope(prompt)
            || prompt.contains("subfolder")
            || prompt.contains("subdirector"))
}

fn prompt_requests_destination_creation(prompt: &str) -> bool {
    let mentions_folder = prompt_contains_word(prompt, "folder")
        || prompt_contains_word(prompt, "folders")
        || prompt_contains_word(prompt, "directory")
        || prompt_contains_word(prompt, "directories");
    if !mentions_folder {
        return false;
    }

    [
        "new folder",
        "new directory",
        "folder named",
        "folder called",
        "directory named",
        "directory called",
        "folder on",
        "directory on",
        "into a folder",
        "into the folder",
        "into a directory",
        "into the directory",
    ]
    .iter()
    .any(|needle| prompt.contains(needle))
        || ((prompt_contains_word(prompt, "create") || prompt_contains_word(prompt, "make"))
            && (prompt.contains(" named ") || prompt.contains(" called ")))
}

fn prompt_requests_recursive_scope(prompt: &str) -> bool {
    prompt.contains("recursive")
        || prompt.contains("recursively")
        || prompt.contains("subfolder")
        || prompt.contains("subdirectories")
        || prompt.contains("subdirector")
        || prompt.contains("under")
}

fn prompt_requests_file_listing_or_transfer(prompt: &str) -> bool {
    prompt.contains("list")
        || prompt.contains("show")
        || prompt.contains("find")
        || prompt.contains("search")
        || prompt.contains("copy")
        || prompt.contains("move")
        || prompt.contains("open")
}

fn prompt_requests_content_search(prompt: &str) -> bool {
    if prompt.contains("search web")
        || prompt.contains("search the web")
        || prompt.contains("web search")
        || prompt.contains("internet")
        || prompt.contains("online")
    {
        return false;
    }
    if (prompt_contains_word(prompt, "directory")
        || prompt_contains_word(prompt, "directories")
        || prompt_contains_word(prompt, "folder")
        || prompt_contains_word(prompt, "folders"))
        && prompt.contains("containing")
        && !(prompt.contains("content")
            || prompt.contains("contents")
            || prompt.contains("text")
            || prompt.contains("todo"))
    {
        return false;
    }
    if !(prompt.contains("search")
        || prompt.contains("grep")
        || prompt.contains("matches")
        || prompt.contains("containing")
        || prompt.contains("contains"))
    {
        return false;
    }
    if prompt.contains("file named")
        || prompt.contains("files named")
        || prompt.contains("filename")
        || prompt.contains("file name")
        || prompt.contains("named ")
        || prompt.contains("called ")
    {
        return false;
    }
    prompt.contains(" for ")
        || prompt.contains("todo")
        || prompt.contains("text")
        || prompt.contains("phrase")
        || prompt.contains("contents")
        || prompt.contains("content")
        || prompt.contains("case insensitive")
        || prompt.contains("case-insensitive")
}

fn prompt_requests_case_insensitive_search(prompt: &str) -> bool {
    prompt.contains("case insensitive") || prompt.contains("case-insensitive")
}

fn command_has_content_search(command: &str) -> bool {
    command_contains_word(command, "grep")
        || command_contains_word(command, "rg")
        || command_contains_word(command, "ag")
        || command_contains_word(command, "ack")
        || command.contains("git grep")
}

fn command_has_case_insensitive_search(command: &str) -> bool {
    command.split_whitespace().any(|token| {
        token == "--ignore-case"
            || (token.starts_with('-') && !token.starts_with("--") && token[1..].contains('i'))
    })
}

fn prompt_requests_largest_files(prompt: &str) -> bool {
    prompt.contains("largest") && prompt.contains("file")
}

fn command_satisfies_largest_files(command: &str, prompt: &str) -> bool {
    let wants_files = prompt.contains("file");
    let file_only = !wants_files
        || command.contains("-type f")
        || command.contains("*(.)")
        || command.contains("grep -v /")
        || command.contains("grep -v '/'");
    let ls_ranks_files =
        command_contains_word(command, "ls") && command_has_flag(command, 'S', "") && file_only;
    let du_ranks_files =
        command_contains_word(command, "du") && command_contains_word(command, "sort") && file_only;
    let has_size_sort = ls_ranks_files
        || (du_ranks_files && (!wants_files || command.contains("-type f")))
        || (command_contains_word(command, "find") && command.contains("-size"))
        || (command_contains_word(command, "find")
            && command.contains("stat")
            && command.contains("%z")
            && command_contains_word(command, "sort"));
    if !has_size_sort {
        return false;
    }
    if prompt.contains(" 3 largest")
        || prompt.contains(" three largest")
        || prompt.contains(" 10 largest")
        || prompt.contains(" ten largest")
        || prompt.contains("top ")
    {
        return command_contains_word(command, "head")
            || command.contains("-n 3")
            || command.contains("-n 10");
    }
    true
}

fn prompt_requests_ranked_single_file(prompt: &str) -> bool {
    (prompt.contains("oldest") || prompt.contains("newest") || prompt.contains("most recent"))
        && prompt_contains_word(prompt, "file")
        && !prompt.contains("subdirectories")
        && !prompt.contains("subdirectory")
}

fn command_selects_ranked_single_file(command: &str, prompt: &str) -> bool {
    let sort = if prompt.contains("oldest") {
        "sort -n"
    } else {
        "sort -nr"
    };
    command_contains_word(command, "find")
        && command.contains("-type f")
        && command.contains("stat -f")
        && command.contains("%m")
        && command.contains(sort)
        && command.contains("head -n 1")
        && command.contains("cut ")
        && command.contains("-f2-")
}

fn prompt_requests_directories_only(prompt: &str) -> bool {
    (prompt.contains("only directories")
        || prompt.contains("directories, not files")
        || prompt.contains("directories not files")
        || prompt.contains("folders, not files")
        || prompt.contains("folders not files"))
        && (prompt.contains("show") || prompt.contains("list") || prompt.contains("find"))
}

fn command_lists_directories_only(command: &str) -> bool {
    (command_contains_word(command, "find") && command.contains("-type d"))
        || (command_contains_word(command, "ls")
            && command_has_flag(command, 'd', "")
            && command.contains("*/"))
}

fn prompt_requests_delete_empty_directories(prompt: &str) -> bool {
    (prompt.contains("empty directory") || prompt.contains("empty directories"))
        && (prompt_contains_word(prompt, "delete")
            || prompt_contains_word(prompt, "remove")
            || prompt_contains_word(prompt, "rmdir"))
}

fn command_deletes_empty_directories(command: &str) -> bool {
    command_contains_word(command, "find")
        && command.contains("-type d")
        && command.contains("-empty")
        && (command.contains("-delete")
            || command_contains_word(command, "rmdir")
            || command_contains_word(command, "rm"))
}

fn prompt_requests_recent_file_per_subdir(prompt: &str) -> bool {
    prompt.contains("most recently modified file")
        && (prompt.contains("subdirectory") || prompt.contains("subdirectories"))
}

fn command_checks_recent_file_per_subdir(command: &str) -> bool {
    (command_contains_word(command, "for") && command_contains_word(command, "head"))
        || (command_contains_word(command, "find")
            && (command_contains_word(command, "sort") || command_contains_word(command, "stat")))
        || (command_contains_word(command, "ls")
            && command_has_flag(command, 't', "")
            && command_contains_word(command, "head"))
}

fn prompt_requests_git_changed_files(prompt: &str) -> bool {
    prompt.contains("file")
        && (prompt.contains("changed since the last commit")
            || prompt.contains("since the last commit")
            || prompt.contains("changed files")
            || prompt.contains("what files i've changed")
            || prompt.contains("what files i have changed"))
}

fn command_checks_git_changes(command: &str) -> bool {
    command_contains_word(command, "git")
        && (command_contains_word(command, "status") || command_contains_word(command, "diff"))
}

fn prompt_requests_filesystem_disk_usage(prompt: &str) -> bool {
    prompt.contains("disk usage by filesystem") || prompt.contains("disk space free")
}

fn command_checks_filesystem_disk_usage(command: &str) -> bool {
    command_contains_word(command, "df")
}

fn prompt_requests_dated_backup(prompt: &str) -> bool {
    prompt.contains("backup")
        && (prompt.contains("folder") || prompt.contains("directory"))
        && (prompt.contains("today") || prompt.contains("date"))
}

fn command_creates_dated_backup(command: &str) -> bool {
    (command_contains_word(command, "tar")
        || command_contains_word(command, "cp")
        || command_contains_word(command, "ditto"))
        && (command.contains("date") || command.contains("$(date"))
}

fn prompt_requests_create_and_cd(prompt: &str) -> bool {
    (prompt.contains("cd into")
        || prompt.contains("change into")
        || prompt.contains("enter it")
        || prompt.contains("go into"))
        && (prompt.contains("directory") || prompt.contains("folder"))
}

fn command_creates_and_enters_directory(command: &str) -> bool {
    (command_contains_word(command, "mkdir") && command_contains_word(command, "cd"))
        || command_contains_word(command, "mkcd")
}

fn prompt_mentions_image_files(prompt: &str) -> bool {
    prompt.contains("image") || prompt.contains("photo") || prompt.contains("picture")
}

fn prompt_mentions_markdown_files(prompt: &str) -> bool {
    prompt.contains("markdown") || prompt.contains(".md")
}

fn command_has_image_filter(command: &str) -> bool {
    [
        "jpg", "jpeg", "png", "gif", "webp", "heic", "tif", "tiff", "bmp",
    ]
    .iter()
    .any(|ext| command.contains(ext))
}

fn command_has_markdown_filter(command: &str) -> bool {
    command.contains(".md") || command.contains(".markdown")
}

fn command_has_copy_operation(command: &str) -> bool {
    command_contains_word(command, "cp")
        || command_contains_word(command, "rsync")
        || command_contains_word(command, "ditto")
}

fn command_has_move_operation(command: &str) -> bool {
    command_contains_word(command, "mv") || command.contains("--remove-source-files")
}

fn command_has_recursive_selection(command: &str) -> bool {
    (command_contains_word(command, "find") && !command.contains("-maxdepth 1"))
        || command.contains("**")
        || command_contains_word(command, "rsync")
        || command_contains_word(command, "rg")
        || (command_contains_word(command, "grep") && command_has_recursive_grep_flag(command))
        || command.contains(" -R ")
        || command.contains(" -r ")
}

fn command_has_recursive_grep_flag(command: &str) -> bool {
    command.split_whitespace().any(|token| {
        token == "-R"
            || token == "-r"
            || token == "--recursive"
            || (token.starts_with('-')
                && !token.starts_with("--")
                && (token[1..].contains('R') || token[1..].contains('r')))
    })
}

fn prompt_requests_plain_listing(prompt: &str) -> bool {
    if !(prompt.contains("list") || prompt.contains("show")) {
        return false;
    }
    if !(prompt.contains("files")
        || prompt.contains("contents")
        || prompt.contains("everything")
        || prompt.contains("entries"))
    {
        return false;
    }
    ![
        "not directories",
        "not files",
        "no directories",
        "no files",
        "exclude directories",
        "exclude files",
        "without directories",
        "without files",
        "only directories",
        "directories only",
        "directory only",
        "files only",
        "only files",
        "recursive",
        "subfolder",
        "subdirectories",
        "image",
        "photo",
        "picture",
        "markdown",
        ".md",
        "count",
        "how many",
        "oldest",
        "newest",
        "recent",
        "open",
        "copy",
        "move",
        "largest",
        "biggest",
        "larger",
        "smaller",
        "modified",
        "created",
        "sort",
        "by size",
    ]
    .iter()
    .any(|needle| prompt.contains(needle))
}

fn prompt_mentions_standard_home_dir(prompt: &str) -> bool {
    prompt.contains("download")
        || prompt.contains("desktop")
        || prompt.contains("document")
        || prompt.contains("picture")
        || prompt.contains("movie")
        || prompt.contains("music")
}

fn command_mentions_requested_home_path(command: &str, prompt: &str) -> bool {
    let target = if prompt.contains("download") {
        Some("$home/downloads")
    } else if prompt.contains("desktop") {
        Some("$home/desktop")
    } else if prompt.contains("document") {
        Some("$home/documents")
    } else if prompt.contains("picture") {
        Some("$home/pictures")
    } else if prompt.contains("movie") {
        Some("$home/movies")
    } else if prompt.contains("music") {
        Some("$home/music")
    } else {
        None
    };
    target
        .map(|target| command.contains(target) || command.contains(&target.replace("$home", "~")))
        .unwrap_or(true)
}

fn prompt_contains_word(prompt: &str, word: &str) -> bool {
    prompt
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == word)
}

fn command_contains_word(command: &str, word: &str) -> bool {
    command
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')))
        .any(|token| token == word)
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
            if is_single_dash_word_option(token) {
                flags.push(token.to_string());
            } else if token.len() > 2 && !token.contains('=') {
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

fn is_single_dash_word_option(token: &str) -> bool {
    matches!(
        token,
        "-maxdepth"
            | "-mindepth"
            | "-type"
            | "-name"
            | "-iname"
            | "-regex"
            | "-iregex"
            | "-exec"
            | "-path"
            | "-prune"
            | "-print"
            | "-print0"
            | "-mtime"
            | "-mmin"
            | "-size"
            | "-empty"
            | "-newer"
            | "-perm"
            | "-user"
            | "-group"
            | "-delete"
    )
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
    fn validate_find_word_options_as_single_flags() {
        let proposal = GroundedProposal {
            command: "find $HOME/Downloads -maxdepth 1 -type f -print | wc -l".to_string(),
            intent: "count files".to_string(),
            expected_effect: "show count".to_string(),
            commands_used: vec!["find".to_string(), "wc".to_string()],
            risk_level: "read_only".to_string(),
            destructive: false,
            requires_approval: true,
            grounding: vec!["docs#find".to_string()],
            validation: Vec::new(),
        };
        let findings = validate_round(
            &proposal,
            &ValidationContext {
                prompt: "how many files do I have in downloads".to_string(),
                command_exists: true,
                docs_excerpt: "find -maxdepth -type -print wc -l".to_string(),
                validate_command_flags: true,
                parse_ambiguous: false,
                parse_warnings: Vec::new(),
                parse_risky_constructs: false,
            },
        );
        assert!(
            !findings
                .iter()
                .any(|finding| finding.kind == "unsupported_flag"),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_entry_count_for_file_count() {
        let findings = validate_prompt_effects(
            "ls -A $HOME/Downloads | wc -l",
            "how many files do I have in downloads",
        );
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt"
                    && finding.detail.contains("regular files")
            }),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_partial_compound_copy() {
        let findings = validate_prompt_effects(
            "mkdir -p $HOME/Desktop/md",
            "make a folder on my desktop named md and copy all markdown files from my desktop and all of its subfolders recursive to the new md folder",
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.kind == "insufficient_for_prompt"),
            "{findings:?}"
        );

        let complete = validate_prompt_effects(
            "mkdir -p $HOME/Desktop/md && find $HOME/Desktop -path $HOME/Desktop/md -prune -o -type f \\( -iname '*.md' -o -iname '*.markdown' \\) -print0 | xargs -0 -I{} cp -p {} $HOME/Desktop/md/",
            "make a folder on my desktop named md and copy all markdown files from my desktop and all of its subfolders recursive to the new md folder",
        );
        assert!(complete.is_empty(), "{complete:?}");
    }

    #[test]
    fn validate_prompt_effects_rejects_overfiltered_plain_listing() {
        let findings = validate_prompt_effects(
            "ls -A $HOME/Downloads | grep '/$' | awk '{print $NF}'",
            "list all the files in my downloads",
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.kind == "insufficient_for_prompt"),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_unfiltered_image_listing() {
        let findings = validate_prompt_effects(
            "find $HOME/Desktop -type f -print",
            "list all of the image files on my desktop and in all of my desktop subfolders",
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.kind == "insufficient_for_prompt"),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_file_listing_for_content_search() {
        let findings = validate_prompt_effects(
            "find . -type f -print",
            "search recursively for TODO case insensitively in this directory",
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.kind == "insufficient_for_prompt"),
            "{findings:?}"
        );

        let complete = validate_prompt_effects(
            "grep -Ri TODO .",
            "search recursively for TODO case insensitively in this directory",
        );
        assert!(complete.is_empty(), "{complete:?}");
    }

    #[test]
    fn validate_prompt_effects_does_not_treat_remove_as_move() {
        let complete = validate_prompt_effects(
            "find . -type d -empty -delete",
            "remove all empty directories under here",
        );
        assert!(complete.is_empty(), "{complete:?}");
    }

    #[test]
    fn validate_prompt_effects_rejects_missing_empty_directory_delete() {
        let findings = validate_prompt_effects(
            "find . -type d -empty",
            "find every empty directory under here and delete them",
        );
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt" && finding.detail.contains("delete/rmdir")
            }),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_does_not_invent_destination_for_bak_copy() {
        let complete = validate_prompt_effects(
            "for f in *.conf; do cp \"$f\" \"$f.bak\"; done",
            "make a .bak copy of every .conf file in this directory",
        );
        assert!(complete.is_empty(), "{complete:?}");
    }

    #[test]
    fn validate_prompt_effects_rejects_plain_ls_for_ranked_size_request() {
        let findings =
            validate_prompt_effects("ls", "show me the 10 largest files in this directory");
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt" && finding.detail.contains("largest-file")
            }),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_directory_size_for_largest_files() {
        let findings = validate_prompt_effects(
            "du -ah --max-depth=1 | sort -rh | head -n 4",
            "show me the 3 largest files here, then I'll decide what to delete",
        );
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt" && finding.detail.contains("largest-file")
            }),
            "{findings:?}"
        );

        let plain_ls = validate_prompt_effects(
            "ls -lS | head -n 3",
            "show me the 3 largest files here, then I'll decide what to delete",
        );
        assert!(
            plain_ls.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt" && finding.detail.contains("largest-file")
            }),
            "{plain_ls:?}"
        );

        let complete = validate_prompt_effects(
            "find . -maxdepth 1 -type f -exec stat -f '%z %N' {} + | sort -nr | head -n 3 | cut -d' ' -f2-",
            "show me the 3 largest files here, then I'll decide what to delete",
        );
        assert!(complete.is_empty(), "{complete:?}");
    }

    #[test]
    fn validate_prompt_effects_rejects_plain_ls_for_directories_only() {
        let findings = validate_prompt_effects("ls", "show only directories, not files");
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt"
                    && finding.detail.contains("directories-only")
            }),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_directory_dump_for_oldest_file() {
        let findings =
            validate_prompt_effects("ls -lt", "what is the oldest file in this directory?");
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt"
                    && finding.detail.contains("oldest/newest-file")
            }),
            "{findings:?}"
        );

        let complete = validate_prompt_effects(
            "find . -maxdepth 1 -type f -exec stat -f '%m %N' {} + | sort -n | head -n 1 | cut -d' ' -f2-",
            "what is the oldest file in this directory?",
        );
        assert!(complete.is_empty(), "{complete:?}");
    }

    #[test]
    fn validate_prompt_effects_does_not_treat_directory_containing_as_text_search() {
        let complete = validate_prompt_effects(
            "mkdir -p 2026-Q1/month-1 2026-Q1/month-2 2026-Q1/month-3",
            "create directories for 2026-Q1 through 2026-Q4 each containing month-1 month-2 month-3 subdirs",
        );
        assert!(
            !complete
                .iter()
                .any(|finding| finding.detail.contains("text-search")),
            "{complete:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_missing_git_context_for_changed_files() {
        let findings = validate_prompt_effects(
            "ls",
            "show me what files I've changed since the last commit",
        );
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt" && finding.detail.contains("git")
            }),
            "{findings:?}"
        );
    }

    #[test]
    fn validate_prompt_effects_rejects_missing_cd_step() {
        let findings = validate_prompt_effects(
            "mkdir -p workspace",
            "make a directory called workspace and cd into it",
        );
        assert!(
            findings.iter().any(|finding| {
                finding.kind == "insufficient_for_prompt" && finding.detail.contains("cd into")
            }),
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
