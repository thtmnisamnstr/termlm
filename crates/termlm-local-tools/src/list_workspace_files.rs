use anyhow::Result;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListWorkspaceFilesResult {
    pub root: String,
    pub entries: Vec<WorkspaceEntry>,
    pub truncated: bool,
}

#[derive(Debug)]
struct CandidateEntry {
    path: PathBuf,
    kind: String,
    priority: u8,
    depth: usize,
    mtime_secs: u64,
}

pub fn list_workspace_files(
    root: &Path,
    max_entries: usize,
    max_depth: usize,
    include_hidden: bool,
) -> Result<ListWorkspaceFilesResult> {
    let mut candidates = Vec::<CandidateEntry>::new();
    let mut skipped = false;

    let mut walker = WalkBuilder::new(root);
    walker.hidden(!include_hidden);
    walker.max_depth(Some(max_depth));
    walker.standard_filters(true);
    walker.filter_entry(|entry| {
        let Some(name) = entry.file_name().to_str() else {
            return true;
        };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            return true;
        }
        !is_noise_dir(name)
    });

    for entry in walker.build().flatten() {
        if entry.path() == root {
            continue;
        }
        if candidates.len() >= max_entries.saturating_mul(4) {
            skipped = true;
            break;
        }
        let kind = if entry.file_type().map(|f| f.is_dir()).unwrap_or(false) {
            "dir"
        } else {
            "file"
        };
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let depth = rel.components().count();
        let mtime_secs = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|ts| ts.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        candidates.push(CandidateEntry {
            path: entry.path().to_path_buf(),
            kind: kind.to_string(),
            priority: entry_priority(rel, kind),
            depth,
            mtime_secs,
        });
    }

    candidates.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(a.depth.cmp(&b.depth))
            .then(b.mtime_secs.cmp(&a.mtime_secs))
            .then(a.path.cmp(&b.path))
    });
    if candidates.len() > max_entries {
        candidates.truncate(max_entries);
        skipped = true;
    }

    let entries = candidates
        .into_iter()
        .map(|entry| WorkspaceEntry {
            path: entry.path.display().to_string(),
            kind: entry.kind,
        })
        .collect::<Vec<_>>();

    Ok(ListWorkspaceFilesResult {
        root: root.display().to_string(),
        entries,
        truncated: skipped,
    })
}

fn entry_priority(rel: &Path, kind: &str) -> u8 {
    let name = rel
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let depth = rel.components().count();
    if depth == 1 && is_manifest_or_top_doc(&name) {
        return 0;
    }
    if kind == "dir" && depth <= 2 && is_source_or_config_dir(&name) {
        return 1;
    }
    if depth <= 2 {
        return 2;
    }
    3
}

fn is_manifest_or_top_doc(name: &str) -> bool {
    matches!(
        name,
        "cargo.toml"
            | "package.json"
            | "pyproject.toml"
            | "go.mod"
            | "makefile"
            | "justfile"
            | "taskfile.yml"
            | "taskfile.yaml"
            | "dockerfile"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | ".tool-versions"
            | ".nvmrc"
            | ".python-version"
            | ".env.example"
    ) || name.starts_with("readme")
        || name.starts_with("license")
}

fn is_source_or_config_dir(name: &str) -> bool {
    matches!(
        name,
        "src"
            | "lib"
            | "app"
            | "cmd"
            | "scripts"
            | "config"
            | "configs"
            | "docs"
            | "test"
            | "tests"
    )
}

fn is_noise_dir(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "vendor"
            | ".cache"
            | "__pycache__"
            | ".venv"
            | "venv"
            | "tmp"
            | ".next"
            | ".turbo"
    )
}
