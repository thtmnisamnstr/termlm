use crate::redaction::redact_secrets;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitContextResult {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub staged_files: Vec<String>,
    pub unstaged_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub conflict_files: Vec<String>,
    pub stash_count: usize,
    pub recent_commits: Vec<String>,
    pub diff_summary: Option<String>,
    pub changed_files_truncated: bool,
    pub diff_truncated: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct GitContextOptions {
    pub max_changed_files: usize,
    pub max_recent_commits: usize,
    pub include_diff_summary: bool,
    pub max_diff_bytes: usize,
}

impl Default for GitContextOptions {
    fn default() -> Self {
        Self {
            max_changed_files: 200,
            max_recent_commits: 10,
            include_diff_summary: true,
            max_diff_bytes: 12_000,
        }
    }
}

pub fn git_context(root: &Path, opts: GitContextOptions) -> Result<GitContextResult> {
    let root_out = match run_git(root, ["rev-parse", "--show-toplevel"]) {
        Ok(v) => v.trim().to_string(),
        Err(e) if is_not_a_git_repository_error(&e.to_string()) => {
            return Ok(not_a_git_repository(root));
        }
        Err(e) => return Err(e),
    };
    let branch = run_git(root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    let upstream = run_git(
        root,
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());
    let (ahead, behind) = if upstream.is_some() {
        upstream_ahead_behind(root).unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    let status = run_git(root, ["status", "--porcelain"])?;
    let mut staged_files = Vec::new();
    let mut unstaged_files = Vec::new();
    let mut untracked_files = Vec::new();
    let mut conflict_files = Vec::new();
    let mut changed_files_truncated = false;

    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let x = line.chars().next().unwrap_or(' ');
        let y = line.chars().nth(1).unwrap_or(' ');
        let path = line[3..].to_string();

        if x != ' ' && x != '?' {
            if staged_files.len() < opts.max_changed_files {
                staged_files.push(path.clone());
            } else {
                changed_files_truncated = true;
            }
        }
        if y != ' ' {
            if unstaged_files.len() < opts.max_changed_files {
                unstaged_files.push(path.clone());
            } else {
                changed_files_truncated = true;
            }
        }
        if x == '?' && y == '?' {
            if untracked_files.len() < opts.max_changed_files {
                untracked_files.push(path.clone());
            } else {
                changed_files_truncated = true;
            }
        }
        if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
            if conflict_files.len() < opts.max_changed_files {
                conflict_files.push(path);
            } else {
                changed_files_truncated = true;
            }
        }
    }

    let conflict_names =
        run_git(root, ["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    for line in conflict_names.lines() {
        let path = line.trim();
        if path.is_empty() || conflict_files.iter().any(|p| p == path) {
            continue;
        }
        if conflict_files.len() < opts.max_changed_files {
            conflict_files.push(path.to_string());
        } else {
            changed_files_truncated = true;
            break;
        }
    }

    let stash_count = run_git(root, ["stash", "list"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);

    let commits_raw = run_git(
        root,
        [
            "log",
            "--oneline",
            "-n",
            &opts.max_recent_commits.max(1).to_string(),
        ],
    )?;
    let recent_commits = commits_raw
        .lines()
        .map(|l| l.to_string())
        .collect::<Vec<_>>();

    let mut diff_truncated = false;
    let diff_summary = if opts.include_diff_summary {
        let unstaged_stat = run_git(root, ["diff", "--stat", "--no-color"]).unwrap_or_default();
        let staged_stat =
            run_git(root, ["diff", "--cached", "--stat", "--no-color"]).unwrap_or_default();
        let mut sections = Vec::new();
        if !unstaged_stat.trim().is_empty() {
            sections.push(format!("unstaged:\n{}", unstaged_stat.trim_end()));
        }
        if !staged_stat.trim().is_empty() {
            sections.push(format!("staged:\n{}", staged_stat.trim_end()));
        }
        let combined = sections.join("\n\n");
        if combined.trim().is_empty() {
            None
        } else {
            let mut text = redact_secrets(&combined);
            if text.len() > opts.max_diff_bytes {
                text.truncate(opts.max_diff_bytes);
                diff_truncated = true;
            }
            Some(text)
        }
    } else {
        None
    };

    Ok(GitContextResult {
        status: "ok".to_string(),
        root: Some(root_out),
        branch: Some(branch.trim().to_string()),
        upstream,
        ahead,
        behind,
        dirty: !(staged_files.is_empty()
            && unstaged_files.is_empty()
            && untracked_files.is_empty()
            && conflict_files.is_empty()),
        staged_files,
        unstaged_files,
        untracked_files,
        conflict_files,
        stash_count,
        recent_commits,
        diff_summary,
        changed_files_truncated,
        diff_truncated,
    })
}

fn is_not_a_git_repository_error(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains("not a git repository")
}

fn not_a_git_repository(path: &Path) -> GitContextResult {
    GitContextResult {
        status: "not_a_git_repository".to_string(),
        root: Some(path.display().to_string()),
        branch: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        dirty: false,
        staged_files: Vec::new(),
        unstaged_files: Vec::new(),
        untracked_files: Vec::new(),
        conflict_files: Vec::new(),
        stash_count: 0,
        recent_commits: Vec::new(),
        diff_summary: None,
        changed_files_truncated: false,
        diff_truncated: false,
    }
}

fn upstream_ahead_behind(root: &Path) -> Result<(u32, u32)> {
    let raw = run_git(root, ["rev-list", "--left-right", "--count", "@{u}...HEAD"])?;
    let mut parts = raw.split_whitespace();
    let behind = parts.next().unwrap_or("0").parse::<u32>().unwrap_or(0);
    let ahead = parts.next().unwrap_or("0").parse::<u32>().unwrap_or(0);
    Ok((ahead, behind))
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .with_context(|| format!("git failed in {}", cwd.display()))?;

    if !out.status.success() {
        anyhow::bail!(
            "git command failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_not_a_git_repository_status() {
        let root = std::env::temp_dir().join(format!(
            "termlm-git-context-not-repo-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");

        let out = git_context(&root, GitContextOptions::default()).expect("git_context");
        assert_eq!(out.status, "not_a_git_repository");
        assert!(out.branch.is_none());
        assert_eq!(out.ahead, 0);
        assert_eq!(out.behind, 0);
        assert_eq!(out.stash_count, 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_ahead_behind() {
        let root = std::env::temp_dir().join(format!(
            "termlm-git-context-ahead-behind-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let remote = root.join("remote.git");
        let other = root.join("other");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");

        run_cmd(&root, ["init", "repo"]).expect("init repo");
        let repo = root.join("repo");
        run_cmd(&repo, ["config", "user.email", "test@example.com"]).expect("email");
        run_cmd(&repo, ["config", "user.name", "Test User"]).expect("name");
        std::fs::write(repo.join("a.txt"), "base\n").expect("write");
        run_cmd(&repo, ["add", "a.txt"]).expect("add");
        run_cmd(&repo, ["commit", "-m", "base"]).expect("commit");

        run_cmd(
            &root,
            ["init", "--bare", remote.to_str().unwrap_or("remote.git")],
        )
        .expect("init bare");
        run_cmd(
            &repo,
            ["remote", "add", "origin", remote.to_str().unwrap_or("")],
        )
        .expect("add remote");
        run_cmd(&repo, ["push", "-u", "origin", "HEAD"]).expect("push base");

        std::fs::write(repo.join("a.txt"), "local\n").expect("write local");
        run_cmd(&repo, ["commit", "-am", "local"]).expect("commit local");

        run_cmd(
            &root,
            [
                "clone",
                remote.to_str().unwrap_or(""),
                other.to_str().unwrap_or(""),
            ],
        )
        .expect("clone");
        run_cmd(&other, ["config", "user.email", "test@example.com"]).expect("email other");
        run_cmd(&other, ["config", "user.name", "Test User"]).expect("name other");
        std::fs::write(other.join("b.txt"), "remote\n").expect("write remote");
        run_cmd(&other, ["add", "b.txt"]).expect("add remote");
        run_cmd(&other, ["commit", "-m", "remote"]).expect("commit remote");
        run_cmd(&other, ["push"]).expect("push remote");

        run_cmd(&repo, ["fetch", "origin"]).expect("fetch origin");
        let out = git_context(&repo, GitContextOptions::default()).expect("git_context");
        assert_eq!(out.status, "ok");
        assert!(out.ahead >= 1, "expected ahead >=1, got {}", out.ahead);
        assert!(out.behind >= 1, "expected behind >=1, got {}", out.behind);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_conflicts_and_stash_count() {
        let root = std::env::temp_dir().join(format!(
            "termlm-git-context-conflicts-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");

        run_cmd(&root, ["init"]).expect("git init");
        run_cmd(&root, ["config", "user.email", "test@example.com"]).expect("email");
        run_cmd(&root, ["config", "user.name", "Test User"]).expect("name");
        std::fs::write(root.join("conflict.txt"), "base\n").expect("write");
        run_cmd(&root, ["add", "conflict.txt"]).expect("add");
        run_cmd(&root, ["commit", "-m", "base"]).expect("commit");
        let default_branch =
            run_git(&root, ["rev-parse", "--abbrev-ref", "HEAD"]).expect("default branch");
        let default_branch = default_branch.trim().to_string();

        std::fs::write(root.join("scratch.txt"), "stash me\n").expect("write scratch");
        run_cmd(&root, ["add", "scratch.txt"]).expect("add scratch");
        let stash_status =
            run_cmd_allow_failure(&root, ["stash", "push", "-m", "tmp-stash"]).expect("stash");
        assert_eq!(stash_status, 0, "expected successful stash");

        run_cmd(&root, ["checkout", "-b", "feature"]).expect("branch feature");
        std::fs::write(root.join("conflict.txt"), "feature\n").expect("write");
        run_cmd(&root, ["commit", "-am", "feature change"]).expect("commit");
        run_cmd(&root, ["checkout", default_branch.as_str()]).expect("checkout default branch");
        std::fs::write(root.join("conflict.txt"), "master\n").expect("write");
        run_cmd(&root, ["commit", "-am", "master change"]).expect("commit");
        let merge_status = run_cmd_allow_failure(&root, ["merge", "feature"]).expect("merge run");
        assert_ne!(merge_status, 0, "merge should conflict");

        let out = git_context(&root, GitContextOptions::default()).expect("git_context");
        assert_eq!(out.status, "ok");
        assert!(
            out.conflict_files.iter().any(|p| p == "conflict.txt"),
            "expected conflict file in {:?}",
            out.conflict_files
        );
        assert!(
            out.stash_count >= 1,
            "expected stash_count >= 1, got {}",
            out.stash_count
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn git_context_limits_diff_and_files() {
        let root = std::env::temp_dir().join(format!(
            "termlm-git-context-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");

        run_cmd(&root, ["init"]).expect("git init");
        run_cmd(&root, ["config", "user.email", "test@example.com"]).expect("email");
        run_cmd(&root, ["config", "user.name", "Test User"]).expect("name");
        std::fs::write(root.join("a.txt"), "one\n").expect("write");
        run_cmd(&root, ["add", "a.txt"]).expect("add");
        run_cmd(&root, ["commit", "-m", "init"]).expect("commit");

        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").expect("write");
        std::fs::write(root.join("b.txt"), "new\n").expect("write");

        let out = git_context(
            &root,
            GitContextOptions {
                max_changed_files: 1,
                max_recent_commits: 1,
                include_diff_summary: true,
                max_diff_bytes: 32,
            },
        )
        .expect("git_context");

        assert_eq!(out.status, "ok");
        assert!(out.dirty);
        assert!(out.changed_files_truncated || out.staged_files.len() <= 1);
        assert_eq!(out.recent_commits.len(), 1);
        assert!(out.diff_summary.is_some());
        assert!(out.diff_truncated || out.diff_summary.unwrap_or_default().len() <= 32);

        let _ = std::fs::remove_dir_all(&root);
    }

    fn run_cmd<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<()> {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .with_context(|| format!("git {:?} failed", args))?;
        if !out.status.success() {
            anyhow::bail!("git failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        Ok(())
    }

    fn run_cmd_allow_failure<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<i32> {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .with_context(|| format!("git {:?} failed", args))?;
        Ok(out.status.code().unwrap_or(1))
    }
}
