use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceResolution {
    pub root: Option<PathBuf>,
    pub reason: String,
}

const MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "yarn.lock",
    "npm-shrinkwrap.json",
    "pyproject.toml",
    "poetry.lock",
    "Pipfile",
    "Pipfile.lock",
    "requirements.txt",
    "requirements-dev.txt",
    "setup.py",
    "setup.cfg",
    "tox.ini",
    "go.mod",
    "go.sum",
    "go.work",
    "go.work.sum",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "Gemfile",
    "Gemfile.lock",
    "Rakefile",
    "composer.json",
    "mix.exs",
    "mix.lock",
    "deno.json",
    "deno.jsonc",
    "Makefile",
    "justfile",
    "Taskfile.yml",
    "Taskfile.yaml",
    "CMakeLists.txt",
    "meson.build",
    "flake.nix",
    "shell.nix",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yaml",
    "compose.yml",
    ".github/workflows",
    ".gitlab-ci.yml",
    ".circleci/config.yml",
    ".buildkite/pipeline.yml",
    "azure-pipelines.yml",
    ".terraform.lock.hcl",
];

const BLOCKED_ROOTS: &[&str] = &[
    "/",
    "/usr",
    "/usr/bin",
    "/bin",
    "/sbin",
    "/etc",
    "/System",
    "/Library",
    "/Applications",
    "/opt/homebrew/bin",
];

pub fn resolve_workspace_root(cwd: &Path, explicit: Option<&Path>) -> WorkspaceResolution {
    resolve_workspace_root_with_markers(cwd, explicit, false, false, &[])
}

pub fn resolve_workspace_root_with_policy(
    cwd: &Path,
    explicit: Option<&Path>,
    allow_home_as_workspace: bool,
    allow_system_dirs: bool,
) -> WorkspaceResolution {
    resolve_workspace_root_with_markers(
        cwd,
        explicit,
        allow_home_as_workspace,
        allow_system_dirs,
        &[],
    )
}

pub fn resolve_workspace_root_with_markers(
    cwd: &Path,
    explicit: Option<&Path>,
    allow_home_as_workspace: bool,
    allow_system_dirs: bool,
    extra_markers: &[String],
) -> WorkspaceResolution {
    let home = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .map(|p| normalize(&p));
    let cwd = normalize(cwd);

    if let Some(root) = explicit {
        let normalized = normalize(root);
        if (!allow_system_dirs && is_blocked(&normalized))
            || (!allow_home_as_workspace && home.as_deref() == Some(normalized.as_path()))
        {
            return WorkspaceResolution {
                root: None,
                reason: "access_denied_sensitive_path".to_string(),
            };
        }
        return WorkspaceResolution {
            root: Some(normalized),
            reason: "explicit_root".to_string(),
        };
    }

    let mut cur = cwd.clone();
    let mut at_start = true;
    loop {
        if at_start {
            if (!allow_system_dirs && is_blocked(&cur))
                || (!allow_home_as_workspace && home.as_deref() == Some(&cur))
            {
                return WorkspaceResolution {
                    root: None,
                    reason: "no_workspace_detected_system_directory".to_string(),
                };
            }
            at_start = false;
        }

        if has_markers(&cur, extra_markers) {
            return WorkspaceResolution {
                root: Some(cur),
                reason: "marker_detected".to_string(),
            };
        }

        let Some(parent) = cur.parent() else {
            break;
        };
        if parent == cur {
            break;
        }
        cur = parent.to_path_buf();
    }

    WorkspaceResolution {
        root: Some(cwd),
        reason: "ad_hoc_workspace".to_string(),
    }
}

fn is_blocked(path: &Path) -> bool {
    BLOCKED_ROOTS.iter().any(|blocked| {
        let blocked = Path::new(blocked);
        if blocked == Path::new("/") {
            path == blocked
        } else {
            path == blocked || path.starts_with(blocked)
        }
    })
}

fn has_markers(root: &Path, extra_markers: &[String]) -> bool {
    MARKERS.iter().any(|m| root.join(m).exists())
        || extra_markers
            .iter()
            .any(|m| !m.trim().is_empty() && root.join(m.trim()).exists())
}

fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn explicit_blocked_root_returns_access_denied() {
        let cwd = Path::new("/tmp");
        let res =
            resolve_workspace_root_with_policy(cwd, Some(Path::new("/usr/bin")), false, false);
        assert!(res.root.is_none());
        assert_eq!(res.reason, "access_denied_sensitive_path");
    }

    #[test]
    fn explicit_subpath_of_blocked_root_is_denied() {
        let cwd = Path::new("/tmp");
        let res = resolve_workspace_root_with_policy(
            cwd,
            Some(Path::new("/usr/local/bin")),
            false,
            false,
        );
        assert!(res.root.is_none());
        assert_eq!(res.reason, "access_denied_sensitive_path");
    }

    #[test]
    fn implicit_system_directory_returns_no_workspace_detected() {
        let res = resolve_workspace_root_with_policy(Path::new("/usr/bin"), None, false, false);
        assert!(res.root.is_none());
        assert_eq!(res.reason, "no_workspace_detected_system_directory");
    }

    #[test]
    fn detects_extended_marker_while_walking_up() {
        let root = unique_temp_dir("termlm-workspace-marker");
        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).expect("create nested");
        std::fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'").expect("marker");
        let expected_root = super::normalize(&root);

        let res = resolve_workspace_root_with_policy(&nested, None, false, false);
        assert_eq!(res.root.as_deref(), Some(expected_root.as_path()));
        assert_eq!(res.reason, "marker_detected");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_configured_markers_while_walking_up() {
        let root = unique_temp_dir("termlm-workspace-extra-marker");
        let nested = root.join("x/y/z");
        std::fs::create_dir_all(&nested).expect("create nested");
        std::fs::create_dir_all(root.join(".config/workspace")).expect("marker dir");
        let expected_root = super::normalize(&root);

        let markers = vec![".config/workspace".to_string()];
        let res = resolve_workspace_root_with_markers(&nested, None, false, false, &markers);
        assert_eq!(res.root.as_deref(), Some(expected_root.as_path()));
        assert_eq!(res.reason, "marker_detected");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn falls_back_to_ad_hoc_workspace_without_markers() {
        let root = unique_temp_dir("termlm-workspace-adhoc");
        std::fs::create_dir_all(&root).expect("mkdir");
        let expected_root = super::normalize(&root);

        let res = resolve_workspace_root_with_policy(&root, None, false, false);
        assert_eq!(res.root.as_deref(), Some(expected_root.as_path()));
        assert_eq!(res.reason, "ad_hoc_workspace");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn home_root_is_blocked_unless_allowed() {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .expect("HOME must be set for test");
        let home = super::normalize(&home);

        let denied = resolve_workspace_root_with_policy(&home, None, false, false);
        assert!(denied.root.is_none());
        assert_eq!(denied.reason, "no_workspace_detected_system_directory");

        let allowed = resolve_workspace_root_with_policy(&home, None, true, false);
        assert_eq!(allowed.root.as_deref(), Some(home.as_path()));
    }
}
