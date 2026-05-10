use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMetadata {
    pub root: String,
    pub languages: Vec<String>,
    pub package_managers: Vec<String>,
    pub scripts: Vec<String>,
    pub ci_files: Vec<String>,
    pub files_read: usize,
    pub files_scanned: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ProjectMetadataOptions {
    pub max_files_read: usize,
    pub max_bytes_per_file: usize,
    pub detect_scripts: bool,
    pub detect_package_managers: bool,
    pub detect_ci: bool,
}

impl Default for ProjectMetadataOptions {
    fn default() -> Self {
        Self {
            max_files_read: 50,
            max_bytes_per_file: 65_536,
            detect_scripts: true,
            detect_package_managers: true,
            detect_ci: true,
        }
    }
}

pub fn project_metadata(root: &Path, opts: ProjectMetadataOptions) -> Result<ProjectMetadata> {
    let mut files_read = 0usize;
    let mut files_scanned = 0usize;
    let mut truncated = false;

    let mut languages = BTreeSet::<String>::new();
    let mut package_managers = BTreeSet::<String>::new();
    let mut scripts = BTreeSet::<String>::new();
    let mut ci_files = BTreeSet::<String>::new();

    let mut consume_file = |path: &Path| -> Option<String> {
        files_scanned = files_scanned.saturating_add(1);
        if !path.exists() || !path.is_file() {
            return None;
        }
        if files_read >= opts.max_files_read {
            truncated = true;
            return None;
        }
        let Ok(meta) = path.metadata() else {
            return None;
        };
        if meta.len() > opts.max_bytes_per_file as u64 {
            truncated = true;
            return None;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return None;
        };
        files_read = files_read.saturating_add(1);
        Some(text)
    };

    let cargo_toml = root.join("Cargo.toml");
    if cargo_toml.exists() {
        languages.insert("rust".to_string());
        if opts.detect_package_managers {
            package_managers.insert("cargo".to_string());
        }
        if opts.detect_scripts {
            scripts.insert("cargo build".to_string());
            scripts.insert("cargo test".to_string());
        }
        let _ = consume_file(&cargo_toml);
    }

    let package_json = root.join("package.json");
    if let Some(text) = consume_file(&package_json) {
        languages.insert("javascript".to_string());
        if opts.detect_package_managers {
            package_managers.insert("npm".to_string());
        }
        if opts.detect_scripts
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(map) = json.get("scripts").and_then(|v| v.as_object())
        {
            for name in map.keys().take(50) {
                scripts.insert(format!("npm run {name}"));
            }
        }
    }

    let pyproject_toml = root.join("pyproject.toml");
    if let Some(text) = consume_file(&pyproject_toml) {
        languages.insert("python".to_string());
        if opts.detect_package_managers {
            package_managers.insert("pip".to_string());
        }
        if opts.detect_scripts
            && let Ok(parsed) = text.parse::<toml::Value>()
        {
            if let Some(tbl) = parsed
                .get("project")
                .and_then(|v| v.get("scripts"))
                .and_then(|v| v.as_table())
            {
                for key in tbl.keys().take(50) {
                    scripts.insert(format!("python -m {key}"));
                }
            }
            if let Some(tbl) = parsed
                .get("tool")
                .and_then(|v| v.get("poetry"))
                .and_then(|v| v.get("scripts"))
                .and_then(|v| v.as_table())
            {
                for key in tbl.keys().take(50) {
                    scripts.insert(format!("poetry run {key}"));
                }
            }
        }
    }

    let go_mod = root.join("go.mod");
    if go_mod.exists() {
        languages.insert("go".to_string());
        if opts.detect_package_managers {
            package_managers.insert("go".to_string());
        }
        if opts.detect_scripts {
            scripts.insert("go test ./...".to_string());
        }
    }

    let lockfiles = [
        ("package-lock.json", "npm"),
        ("yarn.lock", "yarn"),
        ("pnpm-lock.yaml", "pnpm"),
        ("bun.lockb", "bun"),
        ("Cargo.lock", "cargo"),
        ("poetry.lock", "poetry"),
        ("Pipfile.lock", "pipenv"),
        ("go.sum", "go"),
    ];
    if opts.detect_package_managers {
        for (file, manager) in lockfiles {
            if root.join(file).exists() {
                package_managers.insert(manager.to_string());
            }
        }
    }

    if opts.detect_ci {
        let ci_paths = [
            ".github/workflows",
            ".gitlab-ci.yml",
            ".circleci/config.yml",
            "azure-pipelines.yml",
            "buildkite.yml",
        ];
        for rel in ci_paths {
            let p = root.join(rel);
            if p.exists() {
                ci_files.insert(rel.to_string());
            }
        }
    }

    if scripts.is_empty() && opts.detect_scripts {
        if root.join("Makefile").exists() {
            scripts.insert("make".to_string());
        }
        if root.join("justfile").exists() || root.join("Justfile").exists() {
            scripts.insert("just".to_string());
        }
        if root.join("Taskfile.yml").exists() {
            scripts.insert("task".to_string());
        }
    }

    Ok(ProjectMetadata {
        root: root.display().to_string(),
        languages: languages.into_iter().collect(),
        package_managers: package_managers.into_iter().collect(),
        scripts: scripts.into_iter().collect(),
        ci_files: ci_files.into_iter().collect(),
        files_read,
        files_scanned,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_scripts_from_package_json() {
        let root = std::env::temp_dir().join(format!(
            "termlm-project-meta-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts":{"test":"vitest","build":"vite build"}}"#,
        )
        .expect("write");

        let out = project_metadata(
            &root,
            ProjectMetadataOptions {
                max_files_read: 10,
                max_bytes_per_file: 65536,
                detect_scripts: true,
                detect_package_managers: true,
                detect_ci: true,
            },
        )
        .expect("metadata");

        assert!(out.package_managers.iter().any(|m| m == "npm"));
        assert!(out.scripts.iter().any(|s| s == "npm run test"));
        assert!(out.scripts.iter().any(|s| s == "npm run build"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enforces_file_read_budget() {
        let root = std::env::temp_dir().join(format!(
            "termlm-project-meta-budget-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("package.json"), r#"{"scripts":{"a":"x"}}"#).expect("write");
        std::fs::write(root.join("pyproject.toml"), "[project]\nname='x'\n").expect("write");

        let out = project_metadata(
            &root,
            ProjectMetadataOptions {
                max_files_read: 1,
                max_bytes_per_file: 65536,
                detect_scripts: true,
                detect_package_managers: true,
                detect_ci: true,
            },
        )
        .expect("metadata");

        assert!(out.files_read <= 1);
        assert!(out.truncated);

        let _ = std::fs::remove_dir_all(&root);
    }
}
