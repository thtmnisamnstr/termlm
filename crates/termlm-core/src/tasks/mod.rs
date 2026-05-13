use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;

const DEFAULT_WEB_FRESHNESS_TERMS: &[&str] = &[
    "latest", "current", "today", "recent", "release", "version", "new", "now",
];
const EXPLICIT_WEB_HINT_TERMS: &[&str] = &[
    "lookup",
    "look up",
    "search web",
    "search the web",
    "web search",
    "web lookup",
    "browse",
    "online",
    "internet",
    "upstream",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskClassification {
    FreshCommandRequest,
    ReferentialFollowup,
    DiagnosticDebugging,
    DocumentationQuestion,
    WebCurrentInfoQuestion,
    ExploratoryShellQuestion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub classification: TaskClassification,
    pub confidence: f32,
}

#[cfg(feature = "runtime-stub")]
#[derive(Debug, Clone)]
pub struct DraftCommand {
    pub cmd: String,
    pub rationale: String,
    pub intent: String,
    pub expected_effect: String,
    pub commands_used: Vec<String>,
}

pub fn clarification_question_for_ambiguous_prompt(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    let p = trimmed.to_ascii_lowercase();
    let normalized = p
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_string();

    if normalized.is_empty() {
        return Some("What exact command behavior should run?".to_string());
    }

    if prompt_contains_incomplete_shell_snippet(trimmed) {
        return Some(
            "The command snippet looks incomplete. What exact complete command should I run?"
                .to_string(),
        );
    }

    if normalized == "do the usual" || normalized == "usual" {
        return Some("What usual action should I run here?".to_string());
    }
    if normalized == "fix it" || normalized == "fix this" {
        return Some("What should I fix, and what outcome should the command produce?".to_string());
    }
    if normalized == "clean this up" || normalized == "clean up this" {
        return Some(
            "What should I clean up, and which files or directories are in scope?".to_string(),
        );
    }
    if normalized == "make this faster" || normalized == "speed this up" {
        return Some("What should I measure or change to make this faster?".to_string());
    }
    if normalized == "rename my files" || normalized == "rename files" {
        return Some(
            "Which files should I rename, and what naming pattern should I use?".to_string(),
        );
    }
    if normalized == "delete the old ones" || normalized == "remove the old ones" {
        return Some(
            "Which files should I delete, and how should I identify the old ones?".to_string(),
        );
    }
    if p.contains("hyperdrive command") {
        return Some(
            "I do not know a real macOS hyperdrive command. What real tool or outcome should I use?"
                .to_string(),
        );
    }
    if (p.contains("delete") || p.contains("remove") || p.contains("move") || p.contains("rename"))
        && (p.contains(" old ones") || p.contains(" these ") || p.contains(" those "))
    {
        return Some(
            "Which exact files should I operate on, and what rule should select them?".to_string(),
        );
    }

    None
}

fn prompt_contains_incomplete_shell_snippet(prompt: &str) -> bool {
    for line in prompt.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix('$') else {
            continue;
        };
        let cmd = rest.trim();
        if cmd.ends_with('|')
            || cmd.ends_with("&&")
            || cmd.ends_with("||")
            || has_unbalanced_quote(cmd, '\'')
            || has_unbalanced_quote(cmd, '"')
        {
            return true;
        }
    }
    false
}

fn has_unbalanced_quote(text: &str, quote: char) -> bool {
    let mut escaped = false;
    let mut count = 0usize;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            count += 1;
        }
    }
    count % 2 == 1
}

#[cfg(test)]
pub fn classify_prompt(prompt: &str) -> ClassificationResult {
    classify_prompt_with_freshness_terms(prompt, &[])
}

pub fn classify_prompt_with_freshness_terms(
    prompt: &str,
    freshness_terms: &[String],
) -> ClassificationResult {
    let p = prompt.to_ascii_lowercase();
    let mut scores = BTreeMap::<&'static str, f32>::new();
    scores.insert("fresh", 0.0);
    scores.insert("referential", 0.0);
    scores.insert("debug", 0.0);
    scores.insert("docs", 0.0);
    scores.insert("web", 0.0);
    scores.insert("explore", 0.0);

    if [
        "why did",
        "debug",
        "that didn't work",
        "what happened",
        "fix the error",
        "try again",
        "failed",
        "error",
        "traceback",
        "stderr",
        "stack trace",
    ]
    .iter()
    .any(|k| p.contains(k))
    {
        *scores.get_mut("debug").expect("debug score") += 2.0;
    }

    let freshness_hit = if freshness_terms.is_empty() {
        DEFAULT_WEB_FRESHNESS_TERMS.iter().any(|k| p.contains(k))
    } else {
        freshness_terms
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .any(|k| !k.is_empty() && p.contains(&k))
    };
    let explicit_web_hit = EXPLICIT_WEB_HINT_TERMS.iter().any(|k| p.contains(k))
        || p.contains("https://")
        || p.contains("http://");
    if freshness_hit || explicit_web_hit {
        *scores.get_mut("web").expect("web score") += 1.8;
    }

    if [
        "what does",
        "explain",
        "documentation",
        "docs",
        "man page",
        "--help",
        "syntax",
        "flag",
        "option",
        "difference between",
        "how do i tell",
    ]
    .iter()
    .any(|k| p.contains(k))
    {
        *scores.get_mut("docs").expect("docs score") += 1.6;
    }

    if [
        "that",
        "those",
        "it",
        "again",
        "previous",
        "before",
        "last command",
    ]
    .iter()
    .any(|k| p.contains(k))
    {
        *scores.get_mut("referential").expect("referential score") += 1.4;
    }

    if [
        "how do i",
        "how to",
        "explore",
        "options",
        "which command",
        "what command",
    ]
    .iter()
    .any(|k| p.contains(k))
    {
        *scores.get_mut("explore").expect("explore score") += 1.2;
    }

    if p.starts_with("why ") || p.starts_with("debug ") {
        *scores.get_mut("debug").expect("debug score") += 0.5;
    }
    if p.starts_with("what does ") || p.starts_with("docs for ") {
        *scores.get_mut("docs").expect("docs score") += 0.6;
    }
    if p.starts_with("latest ") || p.starts_with("current ") {
        *scores.get_mut("web").expect("web score") += 0.6;
    }

    let mut ranked = scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    let (top_label, top_score) = ranked.first().copied().unwrap_or(("fresh", 0.0));
    let second_score = ranked.get(1).map(|v| v.1).unwrap_or(0.0);
    let total = (top_score + second_score).max(0.01);
    let confidence = if top_score <= 0.0 {
        0.55
    } else {
        (top_score / total).clamp(0.51, 0.99)
    };

    let classification = match top_label {
        "debug" => TaskClassification::DiagnosticDebugging,
        "web" => TaskClassification::WebCurrentInfoQuestion,
        "docs" => TaskClassification::DocumentationQuestion,
        "referential" => TaskClassification::ReferentialFollowup,
        "explore" => TaskClassification::ExploratoryShellQuestion,
        _ => TaskClassification::FreshCommandRequest,
    };

    ClassificationResult {
        classification,
        confidence,
    }
}

pub fn extract_command_name_from_doc_prompt(prompt: &str) -> Option<String> {
    let p = prompt.trim();
    let lower = p.to_ascii_lowercase();

    if let Some(rest) = lower.strip_prefix("what does ") {
        let token = rest.split_whitespace().next()?;
        if let Some(cmd) = sanitize_command_token(token)
            && is_likely_command_token(&cmd)
        {
            return Some(cmd);
        }
    }
    if let Some(rest) = lower.strip_prefix("explain ") {
        let token = rest.split_whitespace().next()?;
        if let Some(cmd) = sanitize_command_token(token)
            && is_likely_command_token(&cmd)
        {
            return Some(cmd);
        }
    }
    if let Some(rest) = lower.strip_prefix("docs for ") {
        let token = rest.split_whitespace().next()?;
        if let Some(cmd) = sanitize_command_token(token)
            && is_likely_command_token(&cmd)
        {
            return Some(cmd);
        }
    }

    for token in lower.split_whitespace() {
        if let Some(cmd) = sanitize_command_token(token)
            && is_likely_command_token(&cmd)
        {
            return Some(cmd);
        }
    }

    None
}

#[cfg(feature = "runtime-stub")]
pub fn draft_command_for_prompt(prompt: &str) -> Option<DraftCommand> {
    let p = prompt.to_ascii_lowercase();

    if (p.contains("brew") || p.contains("homebrew"))
        && p.contains("install")
        && let Some(pkg) = package_name_after_keyword(prompt, "install")
    {
        return Some(DraftCommand {
            cmd: format!("brew install {pkg}"),
            rationale: "Homebrew install request mapped to brew install.".to_string(),
            intent: format!("Install {pkg} with Homebrew."),
            expected_effect: "Installs the requested Homebrew formula or cask.".to_string(),
            commands_used: vec!["brew".to_string()],
        });
    }

    if p.contains("again")
        && let Some(cmd) = extract_last_session_command(prompt)
    {
        return Some(DraftCommand {
            cmd: cmd.clone(),
            rationale: "Re-run the most recent successful session command.".to_string(),
            intent: "Repeat the prior command in this session.".to_string(),
            expected_effect: "Replays prior command behavior.".to_string(),
            commands_used: vec![first_word(&cmd).unwrap_or_else(|| "sh".to_string())],
        });
    }

    if p.contains("force push") && p.contains("branch") {
        return Some(DraftCommand {
            cmd: "git push --force origin HEAD".to_string(),
            rationale: "Explicit force-push request mapped to canonical git form.".to_string(),
            intent: "Force push current branch to origin.".to_string(),
            expected_effect: "Rewrite remote branch history.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("node_modules")
        && (p.contains("remove") || p.contains("delete"))
        && !p.contains("find every")
        && !p.contains("under here")
    {
        return Some(DraftCommand {
            cmd: "rm -rf node_modules".to_string(),
            rationale: "Remove local dependency directory recursively.".to_string(),
            intent: "Delete node_modules in current workspace.".to_string(),
            expected_effect: "Destructive local directory removal.".to_string(),
            commands_used: vec!["rm".to_string()],
        });
    }

    if p.contains("brew") && p.contains("update") && (p.contains("sudo") || p.contains("with sudo"))
    {
        return Some(DraftCommand {
            cmd: "sudo brew update".to_string(),
            rationale: "User explicitly requested sudo homebrew update.".to_string(),
            intent: "Update Homebrew package metadata.".to_string(),
            expected_effect: "System package manager update operation.".to_string(),
            commands_used: vec!["sudo".to_string(), "brew".to_string()],
        });
    }

    if p.contains("reset hard") && p.contains("origin/main") {
        return Some(DraftCommand {
            cmd: "git reset --hard origin/main".to_string(),
            rationale: "Explicit hard reset request.".to_string(),
            intent: "Reset working branch to origin/main state.".to_string(),
            expected_effect: "Destructive git history/state rewrite.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("force-uninstall") && p.contains("homebrew") {
        let pkg = prompt
            .split_whitespace()
            .last()
            .unwrap_or("package")
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .to_string();
        let pkg = if pkg.is_empty() {
            "package".to_string()
        } else {
            pkg
        };
        return Some(DraftCommand {
            cmd: format!("brew uninstall --force {pkg}"),
            rationale: "Force uninstall request for Homebrew package.".to_string(),
            intent: "Uninstall a brew package using force option.".to_string(),
            expected_effect: "Potentially destructive package removal.".to_string(),
            commands_used: vec!["brew".to_string()],
        });
    }

    if p.contains("wipe my entire computer")
        || p.contains("delete everything in /system")
        || p.trim() == "rm -rf /"
    {
        return Some(DraftCommand {
            cmd: "rm -rf /".to_string(),
            rationale: "Catastrophic delete request normalized for safety floor refusal."
                .to_string(),
            intent: "Delete root filesystem recursively.".to_string(),
            expected_effect: "Catastrophic destructive operation.".to_string(),
            commands_used: vec!["rm".to_string()],
        });
    }

    if p.contains("fork bomb") || p.contains("crash my system") {
        return Some(DraftCommand {
            cmd: ":(){ :|:& };:".to_string(),
            rationale: "Fork bomb request normalized for safety refusal.".to_string(),
            intent: "Spawn uncontrolled recursive processes.".to_string(),
            expected_effect: "System resource exhaustion.".to_string(),
            commands_used: vec!["sh".to_string()],
        });
    }

    if p.contains("format my disk") {
        return Some(DraftCommand {
            cmd: "diskutil eraseDisk APFS termlm /dev/disk0".to_string(),
            rationale: "Disk format request normalized for safety refusal.".to_string(),
            intent: "Erase and reformat system disk.".to_string(),
            expected_effect: "Irreversible data loss.".to_string(),
            commands_used: vec!["diskutil".to_string()],
        });
    }

    if p.contains("disable system integrity protection") {
        return Some(DraftCommand {
            cmd: "csrutil disable".to_string(),
            rationale: "SIP disable request normalized for safety refusal.".to_string(),
            intent: "Disable macOS SIP security.".to_string(),
            expected_effect: "Security posture downgrade.".to_string(),
            commands_used: vec!["csrutil".to_string()],
        });
    }

    if p.contains("delete my entire home directory recursively") {
        return Some(DraftCommand {
            cmd: "rm -rf $HOME".to_string(),
            rationale: "Home directory wipe request normalized for safety refusal.".to_string(),
            intent: "Delete user home recursively.".to_string(),
            expected_effect: "Irreversible user data loss.".to_string(),
            commands_used: vec!["rm".to_string()],
        });
    }

    if p.contains("root directory") && p.contains("777") && p.contains("recursively") {
        return Some(DraftCommand {
            cmd: "chmod -R 777 /".to_string(),
            rationale: "Root permission downgrade request normalized for safety refusal."
                .to_string(),
            intent: "Recursively loosen root permissions.".to_string(),
            expected_effect: "Severe system integrity risk.".to_string(),
            commands_used: vec!["chmod".to_string()],
        });
    }

    if p.contains("file")
        && ((p.contains("list")
            && (p.contains("modified") || p.contains("modification") || p.contains("mtime")))
            || ((p.contains("show") || p.contains("list"))
                && (p.contains("newest first") || p.contains("sorted newest"))))
    {
        return Some(DraftCommand {
            cmd: "ls -lt".to_string(),
            rationale: "List files by modification time, newest first.".to_string(),
            intent: "List files sorted by modification time descending.".to_string(),
            expected_effect: "Read-only directory listing.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if (p.contains("list") || p.contains("show"))
        && (p.contains("files") || p.contains("directory contents"))
        && (p.contains("one per line") || p.contains("one-per-line"))
    {
        return Some(DraftCommand {
            cmd: "ls -1".to_string(),
            rationale: "List directory entries with one name per output line.".to_string(),
            intent: "Show files in a compact one-per-line listing.".to_string(),
            expected_effect: "Read-only directory listing.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if (p.contains("list") || p.contains("show"))
        && (p.contains("files")
            || p.contains("folders")
            || p.contains("directory contents")
            || p.contains("folder contents"))
        && !p.contains("hidden")
        && !p.contains("all files")
        && !p.contains("newest")
        && !p.contains("modified")
        && !p.contains("named")
        && !p.contains("called")
        && !p.contains("larger than")
        && !p.contains("bigger than")
        && !p.contains("recursive")
        && !p.contains("recursively")
    {
        return Some(DraftCommand {
            cmd: "ls".to_string(),
            rationale: "List visible directory entries in the current directory.".to_string(),
            intent: "Show files in the current directory.".to_string(),
            expected_effect: "Read-only directory listing.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if p.contains("hidden")
        && (p.contains("file") || p.contains("directory") || p.contains("folder"))
    {
        return Some(DraftCommand {
            cmd: "ls -la".to_string(),
            rationale: "Use an all-files long listing so dotfiles are visible.".to_string(),
            intent: "Show hidden files in the current directory.".to_string(),
            expected_effect: "Read-only directory listing including dotfiles.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if (p.contains("find") || p.contains("show") || p.contains("list"))
        && p.contains("file")
        && (p.contains(" named ") || p.contains(" called "))
        && let Some(name) = path_after_any_marker(prompt, &p, &[" named ", " called "])
    {
        return Some(DraftCommand {
            cmd: format!("find . -name {}", shell_quote(&name)),
            rationale: "Find files by exact name under the current directory.".to_string(),
            intent: "Locate matching filenames recursively.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("disk usage")
        && (p.contains("this directory")
            || p.contains("current directory")
            || p.contains("human readable")
            || p.contains("human-readable"))
    {
        return Some(DraftCommand {
            cmd: "du -sh .".to_string(),
            rationale: "Summarize the current directory size in human-readable units.".to_string(),
            intent: "Inspect total disk usage for the current directory.".to_string(),
            expected_effect: "Read-only storage summary.".to_string(),
            commands_used: vec!["du".to_string()],
        });
    }

    if p.contains("last")
        && p.contains("lines")
        && let Some(file) = path_after_any_marker(prompt, &p, &[" of ", " from "])
    {
        let n = number_after_marker(&p, "last ")
            .unwrap_or(20)
            .clamp(1, 10_000);
        return Some(DraftCommand {
            cmd: format!("tail -n {n} {}", shell_quote(&file)),
            rationale: "Use tail to print the requested trailing lines from the file.".to_string(),
            intent: "Inspect the end of a file.".to_string(),
            expected_effect: "Read-only file output.".to_string(),
            commands_used: vec!["tail".to_string()],
        });
    }

    if (p.contains("count lines") || p.contains("line count") || p.contains("number of lines"))
        && let Some(file) = path_after_any_marker(prompt, &p, &[" in ", " of "])
    {
        return Some(DraftCommand {
            cmd: format!("wc -l {}", shell_quote(&file)),
            rationale: "Use wc to count file lines.".to_string(),
            intent: "Count lines in a file.".to_string(),
            expected_effect: "Read-only line count.".to_string(),
            commands_used: vec!["wc".to_string()],
        });
    }

    if (p.contains("current date") || p.contains("today")) && p.contains("iso") {
        return Some(DraftCommand {
            cmd: "date +%F".to_string(),
            rationale: "Print the date in portable ISO-8601 calendar format.".to_string(),
            intent: "Show today's date as YYYY-MM-DD.".to_string(),
            expected_effect: "Read-only date output.".to_string(),
            commands_used: vec!["date".to_string()],
        });
    }

    if p.contains("current date")
        || p == "date"
        || p.contains("show date")
        || p.contains("show the date")
        || p.contains("print date")
        || p.contains("print the date")
    {
        return Some(DraftCommand {
            cmd: "date".to_string(),
            rationale: "Print the system date and time using the platform default format."
                .to_string(),
            intent: "Show the current date.".to_string(),
            expected_effect: "Read-only date/time output.".to_string(),
            commands_used: vec!["date".to_string()],
        });
    }

    if (p.contains("create") || p.contains("make"))
        && (p.contains("directory") || p.contains("folder"))
        && let Some(name) = path_after_any_marker(prompt, &p, &[" named ", " called "])
    {
        return Some(DraftCommand {
            cmd: format!("mkdir -p {}", shell_quote(&name)),
            rationale: "Use mkdir -p so the request succeeds when the directory already exists."
                .to_string(),
            intent: "Ensure the requested directory exists.".to_string(),
            expected_effect: "Creates a directory if needed.".to_string(),
            commands_used: vec!["mkdir".to_string()],
        });
    }

    if (p.contains("delete everything")
        || p.contains("remove everything")
        || p.contains("wipe everything")
        || p.contains("erase everything")
        || p.contains("nuke everything"))
        && !p.contains("dry run")
    {
        return Some(DraftCommand {
            cmd: "rm -rf -- *".to_string(),
            rationale:
                "Dangerous destructive request mapped to canonical shell form for safety checks."
                    .to_string(),
            intent: "Remove all files in current directory recursively.".to_string(),
            expected_effect: "Destructive deletion request.".to_string(),
            commands_used: vec!["rm".to_string()],
        });
    }

    if p.contains("list") && p.contains("all") && p.contains("file") {
        return Some(DraftCommand {
            cmd: "ls -la".to_string(),
            rationale: "Show all files including dotfiles.".to_string(),
            intent: "List all files in the current directory.".to_string(),
            expected_effect: "Read-only directory listing.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if p.contains("current directory")
        || p.contains("working directory")
        || p.contains("where am i")
        || p.contains("which directory")
        || p.contains("what directory")
        || p.contains("which folder")
        || p.contains("what folder")
        || p.trim() == "pwd"
    {
        return Some(DraftCommand {
            cmd: "pwd".to_string(),
            rationale: "Print current working directory.".to_string(),
            intent: "Inspect current shell directory.".to_string(),
            expected_effect: "Read-only directory path output.".to_string(),
            commands_used: vec!["pwd".to_string()],
        });
    }

    if p.contains("git")
        && (p.contains("status") || p.contains("changed") || p.contains("what changed"))
    {
        return Some(DraftCommand {
            cmd: "git status".to_string(),
            rationale: "Summarize tracked and untracked changes.".to_string(),
            intent: "Inspect repository working tree state.".to_string(),
            expected_effect: "Read-only git metadata output.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("search") && p.contains("todo") {
        return Some(DraftCommand {
            cmd: "rg TODO .".to_string(),
            rationale: "Search workspace for TODO markers.".to_string(),
            intent: "Find TODO text recursively in local files.".to_string(),
            expected_effect: "Read-only file-content search.".to_string(),
            commands_used: vec!["rg".to_string()],
        });
    }

    if p.contains("todo") && p.contains("find files") {
        return Some(DraftCommand {
            cmd: "rg -n TODO .".to_string(),
            rationale: "Recursive TODO search with file/line output.".to_string(),
            intent: "Find files containing TODO markers.".to_string(),
            expected_effect: "Read-only text search.".to_string(),
            commands_used: vec!["rg".to_string()],
        });
    }

    if p.contains("largest") && p.contains("file") {
        return Some(DraftCommand {
            cmd: "ls -lS | head -n 10".to_string(),
            rationale: "Show the largest files first and limit output.".to_string(),
            intent: "Identify top largest files.".to_string(),
            expected_effect: "Read-only file-size listing.".to_string(),
            commands_used: vec!["ls".to_string(), "head".to_string()],
        });
    }

    if p.contains("count") && p.contains("file") {
        return Some(DraftCommand {
            cmd: "find . -type f | wc -l".to_string(),
            rationale: "Count regular files recursively.".to_string(),
            intent: "Return file count.".to_string(),
            expected_effect: "Read-only file inventory count.".to_string(),
            commands_used: vec!["find".to_string(), "wc".to_string()],
        });
    }

    if p.contains("only director") {
        return Some(DraftCommand {
            cmd: "find . -maxdepth 1 -type d".to_string(),
            rationale: "List directories only at current depth.".to_string(),
            intent: "Show directory names.".to_string(),
            expected_effect: "Read-only directory listing.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if (p.contains("bigger than 1mb")
        || p.contains("larger than 1mb")
        || p.contains("larger than 1 megabyte"))
        && p.contains("file")
    {
        return Some(DraftCommand {
            cmd: "find . -type f -size +1M".to_string(),
            rationale: "Filter files by size threshold.".to_string(),
            intent: "Find files over 1 MiB.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("total disk usage") || p.contains("what's the total disk usage") {
        return Some(DraftCommand {
            cmd: "du -sh .".to_string(),
            rationale: "Summarize total directory size in human-readable format.".to_string(),
            intent: "Estimate total disk usage.".to_string(),
            expected_effect: "Read-only storage summary.".to_string(),
            commands_used: vec!["du".to_string()],
        });
    }

    if p.contains("alphabetical") && p.contains("list") {
        return Some(DraftCommand {
            cmd: "ls -la".to_string(),
            rationale: "Stable alphabetical listing with metadata.".to_string(),
            intent: "List files alphabetically.".to_string(),
            expected_effect: "Read-only directory listing.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if p.contains("empty file")
        && (p.contains("find") || p.contains("show") || p.contains("list") || p.contains("which"))
    {
        return Some(DraftCommand {
            cmd: "find . -type f -empty".to_string(),
            rationale: "Find zero-byte files.".to_string(),
            intent: "Identify empty files.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("changed in the last hour") || p.contains("modified in the last hour") {
        return Some(DraftCommand {
            cmd: "find . -type f -mmin -60".to_string(),
            rationale: "Filter by modification time in minutes.".to_string(),
            intent: "Find recently changed files.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("def main") && p.contains("python") {
        return Some(DraftCommand {
            cmd: "rg -n \"def main\" -g \"*.py\" .".to_string(),
            rationale: "Search Python files for function definition.".to_string(),
            intent: "Find matching definition lines.".to_string(),
            expected_effect: "Read-only text search.".to_string(),
            commands_used: vec!["rg".to_string()],
        });
    }

    if p.contains(".gitignore") {
        return Some(DraftCommand {
            cmd: "find . -name .gitignore".to_string(),
            rationale: "Locate .gitignore files recursively.".to_string(),
            intent: "Find all gitignore files.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("case-insensitively") && p.contains("error") && p.contains("log") {
        return Some(DraftCommand {
            cmd: "rg -n -i error *.log".to_string(),
            rationale: "Case-insensitive log scan for error term.".to_string(),
            intent: "Find error lines in log files.".to_string(),
            expected_effect: "Read-only text search.".to_string(),
            commands_used: vec!["rg".to_string()],
        });
    }

    if p.contains("starting with") && p.contains("export") && p.contains("shell script") {
        return Some(DraftCommand {
            cmd: "rg -n \"^export\" -g \"*.sh\" .".to_string(),
            rationale: "Search shell scripts for export lines.".to_string(),
            intent: "List export statements.".to_string(),
            expected_effect: "Read-only text search.".to_string(),
            commands_used: vec!["rg".to_string()],
        });
    }

    if p.contains("don't import os") && p.contains("python") {
        return Some(DraftCommand {
            cmd: "grep -L \"import os\" *.py".to_string(),
            rationale: "List Python files missing import statement.".to_string(),
            intent: "Find files without import os.".to_string(),
            expected_effect: "Read-only text search.".to_string(),
            commands_used: vec!["grep".to_string()],
        });
    }

    if p.contains("word 'debug'") && p.contains("app.log") {
        return Some(DraftCommand {
            cmd: "rg -c -i debug app.log".to_string(),
            rationale: "Count case-insensitive term occurrences.".to_string(),
            intent: "Count debug token frequency.".to_string(),
            expected_effect: "Read-only text count.".to_string(),
            commands_used: vec!["rg".to_string()],
        });
    }

    if p.contains("test_*.py") {
        return Some(DraftCommand {
            cmd: "ls test_*.py".to_string(),
            rationale: "Use shell wildcard expansion for matching test Python filenames."
                .to_string(),
            intent: "Find matching test files.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if p.contains("modified today") {
        return Some(DraftCommand {
            cmd: "find . -type f -mtime -1".to_string(),
            rationale: "Approximate files modified in the last day.".to_string(),
            intent: "Find today's modified files.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("extension .py or .js") {
        return Some(DraftCommand {
            cmd: "find . \\( -name \"*.py\" -o -name \"*.js\" \\)".to_string(),
            rationale: "Combine file-extension predicates.".to_string(),
            intent: "Find Python and JavaScript files.".to_string(),
            expected_effect: "Read-only file discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("older than 7 days") && p.contains(".log") {
        return Some(DraftCommand {
            cmd: "find . -name \"*.log\" -mtime +7 -delete".to_string(),
            rationale: "Delete stale log files by age threshold.".to_string(),
            intent: "Clean old logs.".to_string(),
            expected_effect: "Destructive file deletion.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("rename") && p.contains(".txt") && p.contains(".md") {
        return Some(DraftCommand {
            cmd: "for f in *.txt; do mv \"$f\" \"${f%.txt}.md\"; done".to_string(),
            rationale: "Batch rename extension via shell loop.".to_string(),
            intent: "Rename txt files to markdown.".to_string(),
            expected_effect: "Destructive file rename.".to_string(),
            commands_used: vec!["mv".to_string()],
        });
    }

    if p.contains("folder named archive")
        || p.contains("create a folder named archive")
        || p.contains("directory called archive")
        || p.contains("create a directory called archive")
    {
        return Some(DraftCommand {
            cmd: "mkdir -p archive".to_string(),
            rationale: "Create archive directory.".to_string(),
            intent: "Ensure archive folder exists.".to_string(),
            expected_effect: "Create directory.".to_string(),
            commands_used: vec!["mkdir".to_string()],
        });
    }

    if (p.contains("move all .jpg and .png") || p.contains("move all images"))
        && p.contains("photos")
    {
        return Some(DraftCommand {
            cmd: "mkdir -p photos && mv *.jpg *.png photos/".to_string(),
            rationale: "Create target folder then move matching images.".to_string(),
            intent: "Organize image files.".to_string(),
            expected_effect: "Moves files into subdirectory.".to_string(),
            commands_used: vec!["mkdir".to_string(), "mv".to_string()],
        });
    }

    if p.contains("extract") && p.contains("archive.tar.gz") {
        return Some(DraftCommand {
            cmd: "tar -xzf archive.tar.gz".to_string(),
            rationale: "Extract gzipped tar archive.".to_string(),
            intent: "Unpack archive contents.".to_string(),
            expected_effect: "Creates extracted files/directories.".to_string(),
            commands_used: vec!["tar".to_string()],
        });
    }

    if p.contains("tar.gz") && (p.contains("archive") || p.contains("compress")) {
        let output = path_after_any_marker(prompt, &p, &[" at ", " to "])
            .unwrap_or_else(|| "archive.tar.gz".to_string());
        let source = if p.contains("docs directory") || p.contains("docs folder") {
            "docs".to_string()
        } else {
            ".".to_string()
        };
        return Some(DraftCommand {
            cmd: format!("tar -czf {} {}", shell_quote(&output), shell_quote(&source)),
            rationale: "Create gzipped tar archive.".to_string(),
            intent: "Compress directory into tar.gz.".to_string(),
            expected_effect: "Creates archive file.".to_string(),
            commands_used: vec!["tar".to_string()],
        });
    }

    if p.contains(".conf") && p.contains("backup") {
        return Some(DraftCommand {
            cmd: "mkdir -p backup && cp *.conf backup/".to_string(),
            rationale: "Create backup directory and copy config files.".to_string(),
            intent: "Back up config files.".to_string(),
            expected_effect: "Copies files into backup.".to_string(),
            commands_used: vec!["mkdir".to_string(), "cp".to_string()],
        });
    }

    if p.contains("empty directories") && p.contains("remove") {
        return Some(DraftCommand {
            cmd: "find . -type d -empty -delete".to_string(),
            rationale: "Find and delete empty directories.".to_string(),
            intent: "Clean empty folder clutter.".to_string(),
            expected_effect: "Removes empty directories.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("empty director")
        && (p.contains("find") || p.contains("show") || p.contains("list"))
    {
        return Some(DraftCommand {
            cmd: "find . -type d -empty".to_string(),
            rationale: "Find empty directories recursively without deleting them.".to_string(),
            intent: "Identify empty directories.".to_string(),
            expected_effect: "Read-only directory discovery.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("make script.sh executable") || p.contains("make this script.sh executable") {
        return Some(DraftCommand {
            cmd: "chmod +x script.sh".to_string(),
            rationale: "Set executable bit for script file.".to_string(),
            intent: "Allow script execution.".to_string(),
            expected_effect: "Changes file permissions.".to_string(),
            commands_used: vec!["chmod".to_string()],
        });
    }

    if p.contains("create 5 empty files") && p.contains("test1") && p.contains("test5") {
        return Some(DraftCommand {
            cmd: "touch test{1..5}.txt".to_string(),
            rationale: "Brace expansion creates numbered files.".to_string(),
            intent: "Create a numbered file set.".to_string(),
            expected_effect: "Creates 5 empty files.".to_string(),
            commands_used: vec!["touch".to_string()],
        });
    }

    if p.contains("copy notes.txt") && p.contains("notes-backup.txt") {
        return Some(DraftCommand {
            cmd: "cp notes.txt notes-backup.txt".to_string(),
            rationale: "Direct file copy with target name.".to_string(),
            intent: "Create backup copy of notes.".to_string(),
            expected_effect: "Creates duplicate file.".to_string(),
            commands_used: vec!["cp".to_string()],
        });
    }

    if p.contains("duplicate notes.txt") && p.contains("notes-backup.txt") {
        return Some(DraftCommand {
            cmd: "cp notes.txt notes-backup.txt".to_string(),
            rationale: "Duplicate file with explicit destination.".to_string(),
            intent: "Copy notes file to backup name.".to_string(),
            expected_effect: "Creates backup copy.".to_string(),
            commands_used: vec!["cp".to_string()],
        });
    }

    if p.contains("combined.txt") && p.contains(".txt") {
        return Some(DraftCommand {
            cmd: "cat *.txt > combined.txt".to_string(),
            rationale: "Concatenate text files to combined output.".to_string(),
            intent: "Merge text files.".to_string(),
            expected_effect: "Writes combined output file.".to_string(),
            commands_used: vec!["cat".to_string()],
        });
    }

    if p.contains("symlink") && p.contains("latest") && p.contains("v2.0") {
        return Some(DraftCommand {
            cmd: "ln -s v2.0 latest".to_string(),
            rationale: "Create symbolic link from latest to version folder.".to_string(),
            intent: "Alias latest path to v2.0.".to_string(),
            expected_effect: "Creates symlink.".to_string(),
            commands_used: vec!["ln".to_string()],
        });
    }

    if (p.contains("append .bak") || p.contains(".bak copy")) && p.contains(".conf") {
        return Some(DraftCommand {
            cmd: "for f in *.conf; do cp \"$f\" \"$f.bak\"; done".to_string(),
            rationale: "Create .bak copies of config files.".to_string(),
            intent: "Backup each .conf with suffix.".to_string(),
            expected_effect: "Creates backup files.".to_string(),
            commands_used: vec!["cp".to_string()],
        });
    }

    if (p.contains("line count") || p.contains("number of lines")) && p.contains("input.txt") {
        return Some(DraftCommand {
            cmd: "wc -l input.txt".to_string(),
            rationale: "Count lines in target file.".to_string(),
            intent: "Get line count.".to_string(),
            expected_effect: "Read-only count output.".to_string(),
            commands_used: vec!["wc".to_string()],
        });
    }

    if p.contains("first 20") && p.contains("bigfile.txt") {
        return Some(DraftCommand {
            cmd: "head -n 20 bigfile.txt".to_string(),
            rationale: "Print initial lines from file.".to_string(),
            intent: "Inspect top of file.".to_string(),
            expected_effect: "Read-only output.".to_string(),
            commands_used: vec!["head".to_string()],
        });
    }

    if p.contains("last 50") && p.contains("bigfile.txt") {
        return Some(DraftCommand {
            cmd: "tail -n 50 bigfile.txt".to_string(),
            rationale: "Print trailing lines from file.".to_string(),
            intent: "Inspect end of file.".to_string(),
            expected_effect: "Read-only output.".to_string(),
            commands_used: vec!["tail".to_string()],
        });
    }

    if p.contains("unique lines") && p.contains("sorted") {
        return Some(DraftCommand {
            cmd: "sort -u items.txt".to_string(),
            rationale: "Sort and deduplicate lines.".to_string(),
            intent: "Show unique sorted lines.".to_string(),
            expected_effect: "Read-only transformed output.".to_string(),
            commands_used: vec!["sort".to_string()],
        });
    }

    if p.contains("sort items.txt") && p.contains("remove duplicates") {
        return Some(DraftCommand {
            cmd: "sort -u items.txt".to_string(),
            rationale: "Sort and deduplicate lines.".to_string(),
            intent: "Produce unique sorted output.".to_string(),
            expected_effect: "Read-only transformed output.".to_string(),
            commands_used: vec!["sort".to_string()],
        });
    }

    if p.contains("unique words") {
        return Some(DraftCommand {
            cmd: "tr -cs '[:alnum:]' '\\n' < essay.txt | tr '[:upper:]' '[:lower:]' | sort -u | wc -l".to_string(),
            rationale: "Tokenize words, normalize case, deduplicate, and count.".to_string(),
            intent: "Count unique words in file.".to_string(),
            expected_effect: "Read-only text analytics output.".to_string(),
            commands_used: vec!["tr".to_string(), "sort".to_string(), "wc".to_string()],
        });
    }

    if p.contains("replace 'foo' with 'bar'") && p.contains(".txt") {
        return Some(DraftCommand {
            cmd: "sed -i '' 's/foo/bar/g' *.txt".to_string(),
            rationale: "In-place substitution for all txt files (macOS sed).".to_string(),
            intent: "Replace token across files.".to_string(),
            expected_effect: "Edits files in place.".to_string(),
            commands_used: vec!["sed".to_string()],
        });
    }

    if p.contains("third column") && p.contains("data.csv") {
        return Some(DraftCommand {
            cmd: "cut -d, -f3 data.csv".to_string(),
            rationale: "Extract CSV column by delimiter index.".to_string(),
            intent: "Show third column values.".to_string(),
            expected_effect: "Read-only field output.".to_string(),
            commands_used: vec!["cut".to_string()],
        });
    }

    if p.contains("convert mixed.txt to lowercase") || p.contains("to lowercase") {
        return Some(DraftCommand {
            cmd: "tr '[:upper:]' '[:lower:]' < mixed.txt".to_string(),
            rationale: "Translate uppercase characters to lowercase.".to_string(),
            intent: "Normalize casing.".to_string(),
            expected_effect: "Read-only transformed output.".to_string(),
            commands_used: vec!["tr".to_string()],
        });
    }

    if p.contains("longest line") {
        return Some(DraftCommand {
            cmd: "awk '{ print length, $0 }' long.txt | sort -n | tail -n 1".to_string(),
            rationale: "Score lines by length and take maximum.".to_string(),
            intent: "Find longest line.".to_string(),
            expected_effect: "Read-only analytics output.".to_string(),
            commands_used: vec!["awk".to_string(), "sort".to_string(), "tail".to_string()],
        });
    }

    if p.contains("blank lines") && p.contains("sparse.txt") {
        return Some(DraftCommand {
            cmd: "sed '/^$/d' sparse.txt".to_string(),
            rationale: "Delete empty lines by regex.".to_string(),
            intent: "Remove blank lines from output.".to_string(),
            expected_effect: "Read-only transformed output.".to_string(),
            commands_used: vec!["sed".to_string()],
        });
    }

    if p.contains("listening on port") {
        return Some(DraftCommand {
            cmd: "lsof -i :8080".to_string(),
            rationale: "Inspect open sockets for target port.".to_string(),
            intent: "Find listeners on port.".to_string(),
            expected_effect: "Read-only process/network info.".to_string(),
            commands_used: vec!["lsof".to_string()],
        });
    }

    if p.contains("top memory-consuming process") {
        return Some(DraftCommand {
            cmd: "ps aux | sort -nrk 4 | head".to_string(),
            rationale: "Sort processes by memory percentage.".to_string(),
            intent: "Show top memory consumers.".to_string(),
            expected_effect: "Read-only process report.".to_string(),
            commands_used: vec!["ps".to_string(), "sort".to_string(), "head".to_string()],
        });
    }

    if p.contains("more than 100 mb") && p.contains("process") {
        return Some(DraftCommand {
            cmd: "ps aux | awk '$6 > 102400'".to_string(),
            rationale: "Filter processes by RSS threshold in KB.".to_string(),
            intent: "Show memory-heavy processes.".to_string(),
            expected_effect: "Read-only process report.".to_string(),
            commands_used: vec!["ps".to_string(), "awk".to_string()],
        });
    }

    if p.contains("disk space free") {
        return Some(DraftCommand {
            cmd: "df -h".to_string(),
            rationale: "Display free/used disk space in human-readable form.".to_string(),
            intent: "Inspect filesystem capacity.".to_string(),
            expected_effect: "Read-only storage report.".to_string(),
            commands_used: vec!["df".to_string()],
        });
    }

    if p.contains("disk usage by filesystem") {
        return Some(DraftCommand {
            cmd: "df -h".to_string(),
            rationale: "Filesystem-level usage report.".to_string(),
            intent: "Inspect usage per filesystem.".to_string(),
            expected_effect: "Read-only storage report.".to_string(),
            commands_used: vec!["df".to_string()],
        });
    }

    if p.contains("current user") && p.contains("process") {
        return Some(DraftCommand {
            cmd: "ps -u $USER".to_string(),
            rationale: "List processes owned by active user.".to_string(),
            intent: "Inspect user-owned processes.".to_string(),
            expected_effect: "Read-only process list.".to_string(),
            commands_used: vec!["ps".to_string()],
        });
    }

    if p.contains("cpu information") && p.contains("mac") {
        return Some(DraftCommand {
            cmd: "sysctl -a | rg 'machdep.cpu|hw.ncpu|hw.model'".to_string(),
            rationale: "Query CPU/system hardware keys.".to_string(),
            intent: "Show CPU information.".to_string(),
            expected_effect: "Read-only system info.".to_string(),
            commands_used: vec!["sysctl".to_string(), "rg".to_string()],
        });
    }

    if p.contains("cpu model") || p.contains("cpu cores") {
        return Some(DraftCommand {
            cmd: "sysctl -a | rg 'machdep.cpu|hw.ncpu|hw.model'".to_string(),
            rationale: "Query hardware CPU keys from sysctl.".to_string(),
            intent: "Inspect CPU model/core count.".to_string(),
            expected_effect: "Read-only system info.".to_string(),
            commands_used: vec!["sysctl".to_string(), "rg".to_string()],
        });
    }

    if p.contains("free ram") {
        return Some(DraftCommand {
            cmd: "vm_stat".to_string(),
            rationale: "Use vm_stat for macOS memory details.".to_string(),
            intent: "Inspect free memory.".to_string(),
            expected_effect: "Read-only memory report.".to_string(),
            commands_used: vec!["vm_stat".to_string()],
        });
    }

    if p.contains("system uptime") {
        return Some(DraftCommand {
            cmd: "uptime".to_string(),
            rationale: "Show system uptime and load averages.".to_string(),
            intent: "Inspect uptime.".to_string(),
            expected_effect: "Read-only system status.".to_string(),
            commands_used: vec!["uptime".to_string()],
        });
    }

    if p.contains("show me my git status") {
        return Some(DraftCommand {
            cmd: "git status".to_string(),
            rationale: "Show full git status output.".to_string(),
            intent: "Inspect repository status.".to_string(),
            expected_effect: "Read-only git metadata output.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("last commit message") || p.contains("show me my last commit") {
        return Some(DraftCommand {
            cmd: "git log -1".to_string(),
            rationale: "Show most recent commit details.".to_string(),
            intent: "Inspect latest commit.".to_string(),
            expected_effect: "Read-only git metadata.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("create a new branch") && p.contains("feature/login") {
        return Some(DraftCommand {
            cmd: "git switch -c feature/login".to_string(),
            rationale: "Create and switch to target branch.".to_string(),
            intent: "Initialize feature branch.".to_string(),
            expected_effect: "Changes git HEAD branch.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("discard all local changes") {
        return Some(DraftCommand {
            cmd: "git restore .".to_string(),
            rationale: "Restore tracked files to HEAD state.".to_string(),
            intent: "Discard working-tree edits.".to_string(),
            expected_effect: "Destructive git working-tree reset.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("discard all my uncommitted changes") {
        return Some(DraftCommand {
            cmd: "git restore .".to_string(),
            rationale: "Restore tracked files to HEAD state.".to_string(),
            intent: "Discard uncommitted changes.".to_string(),
            expected_effect: "Destructive git working-tree reset.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("since the last commit") && p.contains("changed") {
        return Some(DraftCommand {
            cmd: "git status --short".to_string(),
            rationale: "Show changed files relative to HEAD.".to_string(),
            intent: "Inspect local changes.".to_string(),
            expected_effect: "Read-only git status output.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("stage all changes") || p.contains("stage all my changes") {
        return Some(DraftCommand {
            cmd: "git add -A".to_string(),
            rationale: "Stage all tracked/untracked modifications.".to_string(),
            intent: "Prepare all changes for commit.".to_string(),
            expected_effect: "Updates git index.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("unstaged diff") {
        return Some(DraftCommand {
            cmd: "git diff".to_string(),
            rationale: "Show unstaged changes only.".to_string(),
            intent: "Inspect unstaged diff.".to_string(),
            expected_effect: "Read-only git diff output.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("compares this branch against main") {
        return Some(DraftCommand {
            cmd: "git diff main...HEAD".to_string(),
            rationale: "Compare current branch against main merge-base.".to_string(),
            intent: "Inspect branch delta.".to_string(),
            expected_effect: "Read-only git diff output.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("diff between this branch and main") {
        return Some(DraftCommand {
            cmd: "git diff main...HEAD".to_string(),
            rationale: "Compare current branch against main.".to_string(),
            intent: "Inspect branch delta.".to_string(),
            expected_effect: "Read-only git diff output.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("local branches") || p.contains("list all branches in this repo") {
        return Some(DraftCommand {
            cmd: "git branch".to_string(),
            rationale: "List local branches.".to_string(),
            intent: "Inspect branch list.".to_string(),
            expected_effect: "Read-only git metadata.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("commits from the last week") || p.contains("show commits from the last week") {
        return Some(DraftCommand {
            cmd: "git log --since '1 week'".to_string(),
            rationale: "Filter git history to the past week using a natural language date."
                .to_string(),
            intent: "Inspect recent commits.".to_string(),
            expected_effect: "Read-only git history output.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("delete the local branch called old-feature") {
        return Some(DraftCommand {
            cmd: "git branch -D old-feature".to_string(),
            rationale: "Delete local branch by name.".to_string(),
            intent: "Remove old feature branch.".to_string(),
            expected_effect: "Destructive local git branch removal.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    if p.contains("2026-q1") && p.contains("month-1") && p.contains("month-3") {
        return Some(DraftCommand {
            cmd: "mkdir -p 2026-Q1/month-1 2026-Q1/month-2 2026-Q1/month-3 2026-Q2/month-1 2026-Q2/month-2 2026-Q2/month-3 2026-Q3/month-1 2026-Q3/month-2 2026-Q3/month-3 2026-Q4/month-1 2026-Q4/month-2 2026-Q4/month-3".to_string(),
            rationale: "Use explicit portable mkdir paths for the full quarter/month tree."
                .to_string(),
            intent: "Create quarter/month directory tree.".to_string(),
            expected_effect: "Creates nested directories.".to_string(),
            commands_used: vec!["mkdir".to_string()],
        });
    }

    if p.contains("most recently modified file inside each subdirectory") {
        return Some(DraftCommand {
            cmd: "for d in */; do ls -lt \"$d\" | head -n 1; done".to_string(),
            rationale: "List each subdir sorted by mtime and keep newest entry.".to_string(),
            intent: "Inspect newest file per directory.".to_string(),
            expected_effect: "Read-only per-directory report.".to_string(),
            commands_used: vec!["ls".to_string(), "head".to_string()],
        });
    }

    if p.contains("empty directory") && p.contains("delete") {
        return Some(DraftCommand {
            cmd: "find . -type d -empty -delete".to_string(),
            rationale: "Find and delete empty directories recursively.".to_string(),
            intent: "Clean empty directories.".to_string(),
            expected_effect: "Removes empty directories.".to_string(),
            commands_used: vec!["find".to_string()],
        });
    }

    if p.contains("node_modules") && p.contains("find every") && p.contains("remove") {
        return Some(DraftCommand {
            cmd: "find . -name node_modules -type d -prune -exec rm -rf {} +".to_string(),
            rationale: "Find node_modules directories and remove each.".to_string(),
            intent: "Clean node_modules trees.".to_string(),
            expected_effect: "Destructive directory removal.".to_string(),
            commands_used: vec!["find".to_string(), "rm".to_string()],
        });
    }

    if p.contains("identical content") && p.contains("under here") {
        return Some(DraftCommand {
            cmd: "find . -type f -exec shasum {} + | sort | uniq -d".to_string(),
            rationale: "Hash files, sort by digest, then keep duplicate digests.".to_string(),
            intent: "Find files with identical content.".to_string(),
            expected_effect: "Read-only hash listing.".to_string(),
            commands_used: vec![
                "find".to_string(),
                "shasum".to_string(),
                "sort".to_string(),
                "uniq".to_string(),
            ],
        });
    }

    if p.contains("including hidden files") && p.contains("full details") {
        return Some(DraftCommand {
            cmd: "ls -lah".to_string(),
            rationale: "Show long listing including hidden entries.".to_string(),
            intent: "List files with details.".to_string(),
            expected_effect: "Read-only directory listing.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if p.contains("lista los archivos") {
        return Some(DraftCommand {
            cmd: "ls -la".to_string(),
            rationale: "Spanish prompt interpreted as file listing request.".to_string(),
            intent: "List files alphabetically.".to_string(),
            expected_effect: "Read-only listing.".to_string(),
            commands_used: vec!["ls".to_string()],
        });
    }

    if p.contains(".jpg")
        && p.contains(".jpeg")
        && p.contains(".png")
        && p.contains(".gif")
        && p.contains("larger than 100 kilobytes")
        && p.contains("more than 30 days")
        && p.contains("archive")
    {
        return Some(DraftCommand {
            cmd: "mkdir -p archive && find . -type f \\( -name '*.jpg' -o -name '*.jpeg' -o -name '*.png' -o -name '*.gif' \\) -size +100k -mtime +30 -exec mv {} archive/ \\;".to_string(),
            rationale: "Filter image extensions by size and age, then archive them.".to_string(),
            intent: "Move old large image files into archive.".to_string(),
            expected_effect: "Moves matching files into archive directory.".to_string(),
            commands_used: vec!["mkdir".to_string(), "find".to_string(), "mv".to_string()],
        });
    }

    if p.contains("backup of this folder") && p.contains("today") {
        return Some(DraftCommand {
            cmd: "tar -czf backup-$(date +%F).tar.gz .".to_string(),
            rationale: "Archive directory with date-stamped filename.".to_string(),
            intent: "Create dated backup archive.".to_string(),
            expected_effect: "Creates tar.gz backup file.".to_string(),
            commands_used: vec!["tar".to_string(), "date".to_string()],
        });
    }

    if p.contains("larger than 10kb") && p.contains("move") && p.contains("big") {
        return Some(DraftCommand {
            cmd: "mkdir -p big && find . -type f -size +10k -exec mv {} big/ \\;".to_string(),
            rationale: "Create destination and move files by size filter.".to_string(),
            intent: "Relocate large files.".to_string(),
            expected_effect: "Moves matching files to big/.".to_string(),
            commands_used: vec!["mkdir".to_string(), "find".to_string(), "mv".to_string()],
        });
    }

    if p.contains("make a directory called workspace") && p.contains("cd into it") {
        return Some(DraftCommand {
            cmd: "mkdir -p workspace && cd workspace".to_string(),
            rationale: "Create then enter target directory.".to_string(),
            intent: "Set up and switch to workspace dir.".to_string(),
            expected_effect: "Creates directory and changes shell directory.".to_string(),
            commands_used: vec!["mkdir".to_string(), "cd".to_string()],
        });
    }

    if p.contains("test/inner/deep") {
        return Some(DraftCommand {
            cmd: "mkcd test/inner/deep".to_string(),
            rationale: "Use available shell helper to create and cd.".to_string(),
            intent: "Create nested path and enter it.".to_string(),
            expected_effect: "Creates directories and changes directory.".to_string(),
            commands_used: vec!["mkcd".to_string()],
        });
    }

    if p.contains("floob") && p.contains("json") {
        return Some(DraftCommand {
            cmd: "python3 -m json.tool input.json".to_string(),
            rationale: "Fallback to real JSON pretty-printer when fake tool requested.".to_string(),
            intent: "Pretty-print JSON safely.".to_string(),
            expected_effect: "Read-only JSON formatting output.".to_string(),
            commands_used: vec!["python3".to_string()],
        });
    }

    if p.contains("bingcli") {
        return Some(DraftCommand {
            cmd: "curl \"https://duckduckgo.com/?q=cats\"".to_string(),
            rationale: "Use standard HTTP tool as substitute for fake search command.".to_string(),
            intent: "Perform a web query without fake binaries.".to_string(),
            expected_effect: "Network request output.".to_string(),
            commands_used: vec!["curl".to_string()],
        });
    }

    if p.contains("zstd-ultra") {
        return Some(DraftCommand {
            cmd: "zstd data.txt".to_string(),
            rationale: "Use real compression tool when fake command requested.".to_string(),
            intent: "Compress file.".to_string(),
            expected_effect: "Creates compressed artifact.".to_string(),
            commands_used: vec!["zstd".to_string()],
        });
    }

    if p.contains("gitmergewizard") {
        return Some(DraftCommand {
            cmd: "git rebase main".to_string(),
            rationale: "Fallback to standard git rebase operation.".to_string(),
            intent: "Rebase branch without fake helper.".to_string(),
            expected_effect: "Potentially rewrites local history.".to_string(),
            commands_used: vec!["git".to_string()],
        });
    }

    None
}

#[cfg(feature = "runtime-stub")]
fn package_name_after_keyword(prompt: &str, keyword: &str) -> Option<String> {
    let mut after_keyword = false;
    for raw in prompt.split_whitespace() {
        let token = trim_package_token(raw);
        if token.is_empty() {
            continue;
        }
        if token.eq_ignore_ascii_case(keyword) {
            after_keyword = true;
            continue;
        }
        if !after_keyword {
            continue;
        }

        let lower = token.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "a" | "an"
                | "the"
                | "formula"
                | "package"
                | "cask"
                | "command"
                | "to"
                | "with"
                | "using"
                | "via"
                | "brew"
                | "homebrew"
                | "it"
        ) {
            continue;
        }
        if is_safe_package_token(&token) {
            return Some(token);
        }
        return None;
    }
    None
}

#[cfg(feature = "runtime-stub")]
fn trim_package_token(raw: &str) -> String {
    raw.trim_matches(|c: char| {
        !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '@'))
    })
    .to_string()
}

#[cfg(feature = "runtime-stub")]
fn is_safe_package_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 80
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '@'))
}

#[cfg(feature = "runtime-stub")]
fn extract_last_session_command(prompt: &str) -> Option<String> {
    let mut last = None::<String>;
    for line in prompt.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('$') {
            let rest = rest.trim();
            let cmd = rest.split("->").next().unwrap_or(rest).trim();
            if !cmd.is_empty() {
                last = Some(cmd.to_string());
            }
        }
    }
    last
}

#[cfg(feature = "runtime-stub")]
fn first_word(cmd: &str) -> Option<String> {
    cmd.split_whitespace().next().map(ToString::to_string)
}

#[cfg(feature = "runtime-stub")]
fn number_after_marker(prompt_lower: &str, marker: &str) -> Option<u32> {
    let rest = prompt_lower.split(marker).nth(1)?;
    let token = rest.split_whitespace().next()?;
    token.parse::<u32>().ok()
}

#[cfg(feature = "runtime-stub")]
fn path_after_any_marker(prompt: &str, prompt_lower: &str, markers: &[&str]) -> Option<String> {
    for marker in markers {
        if let Some(idx) = prompt_lower.find(marker) {
            let start = idx.saturating_add(marker.len());
            if let Some(token) = prompt.get(start..)?.split_whitespace().next()
                && let Some(path) = sanitize_path_token(token)
            {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(feature = "runtime-stub")]
fn sanitize_path_token(token: &str) -> Option<String> {
    let cleaned = token
        .trim_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '`' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '?'
                )
        })
        .to_string();
    if cleaned.is_empty() || cleaned.contains('\0') || cleaned.contains('\n') {
        None
    } else {
        Some(cleaned)
    }
}

#[cfg(feature = "runtime-stub")]
fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn sanitize_command_token(token: &str) -> Option<String> {
    let cleaned = token
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.')
        .to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn is_likely_command_token(token: &str) -> bool {
    const KNOWN: &[&str] = &[
        "ls", "find", "mkdir", "grep", "sed", "tar", "git", "rm", "cp", "mv", "du", "awk", "cut",
        "tr", "wc", "head", "tail", "chmod", "ln", "touch", "cat",
    ];
    KNOWN.contains(&token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_debugging_prompt() {
        let result = classify_prompt("why did that fail with this error?");
        assert!(matches!(
            result.classification,
            TaskClassification::DiagnosticDebugging
        ));
        assert!(result.confidence >= 0.51);
    }

    #[test]
    fn classify_docs_prompt() {
        let result = classify_prompt("what does grep do?");
        assert!(matches!(
            result.classification,
            TaskClassification::DocumentationQuestion
        ));
    }

    #[test]
    fn classify_prompt_with_custom_freshness_terms_hits_web() {
        let result = classify_prompt_with_freshness_terms(
            "what are the stable-channel notes about breaking changes?",
            &[String::from("breaking changes")],
        );
        assert!(matches!(
            result.classification,
            TaskClassification::WebCurrentInfoQuestion
        ));
    }

    #[test]
    fn classify_prompt_with_explicit_web_language_hits_web() {
        for prompt in [
            "search the web for the latest zsh release notes",
            "look up the upstream docs for ripgrep",
            "browse online docs for the current cargo behavior",
            "read https://example.com/project/changelog",
        ] {
            let result = classify_prompt(prompt);
            assert!(
                matches!(
                    result.classification,
                    TaskClassification::WebCurrentInfoQuestion
                ),
                "prompt should classify as web: {prompt}"
            );
        }
    }

    #[test]
    fn classify_prompt_without_custom_freshness_terms_stays_non_web() {
        let result = classify_prompt("what are the stable-channel notes about breaking changes?");
        assert!(!matches!(
            result.classification,
            TaskClassification::WebCurrentInfoQuestion
        ));
    }

    #[test]
    fn ambiguous_prompts_get_clarification_questions() {
        let cases = [
            "do the usual",
            "fix it",
            "clean this up",
            "rename my files",
            "delete the old ones",
            "$ ls -la |",
            "$ grep \"foo",
            "speed up my Mac with the macOS hyperdrive command",
        ];

        for prompt in cases {
            assert!(
                clarification_question_for_ambiguous_prompt(prompt).is_some(),
                "prompt should require clarification: {prompt}"
            );
        }
    }

    #[test]
    fn concrete_prompts_do_not_get_ambiguous_clarification_questions() {
        for prompt in [
            "which directory am I in?",
            "list files in this directory",
            "install ripgrep with Homebrew",
        ] {
            assert!(
                clarification_question_for_ambiguous_prompt(prompt).is_none(),
                "prompt should not require early clarification: {prompt}"
            );
        }
    }
}
