use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteConfig {
    pub suite: SuiteMeta,
    #[serde(default)]
    pub shell_context: ShellContextConfig,
    #[serde(default)]
    pub test: Vec<TestCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteMeta {
    pub version: String,
    pub total_tests: usize,
    pub default_approval_mode: String,
    pub default_timeout_secs: u64,
    pub sandbox_root_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellContextConfig {
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub functions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub id: String,
    pub category: String,
    pub prompt: String,
    #[serde(default)]
    pub setup: Vec<String>,
    pub mode: String,
    #[serde(default)]
    pub expected: Expected,
    #[serde(default)]
    pub relevant_commands: Vec<String>,
    #[serde(default)]
    pub approval_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Expected {
    #[serde(default)]
    pub command_regex: Vec<String>,
    #[serde(default)]
    pub must_succeed: Option<bool>,
    #[serde(default)]
    pub stdout_contains: Vec<String>,
    #[serde(default)]
    pub stdout_order: Vec<String>,
    #[serde(default)]
    pub filesystem_state_after: Option<FilesystemStateAfter>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub forbid_proposed_command: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilesystemStateAfter {
    #[serde(default)]
    pub exists: Vec<String>,
    #[serde(default)]
    pub not_exists: Vec<String>,
}

pub fn load_suite(path: &Path) -> Result<SuiteConfig> {
    let raw = std::fs::read_to_string(path)?;
    let cfg: SuiteConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_suite() {
        let raw = r#"
[suite]
version = "1.0.0"
total_tests = 1
default_approval_mode = "auto"
default_timeout_secs = 30
sandbox_root_template = "/tmp/x"

[shell_context]
aliases = { ll = "ls -lah" }
functions = { mkcd = "mkcd () { mkdir -p \"$1\" && cd \"$1\"; }" }

[[test]]
id = "T-1"
category = "listing"
prompt = "list files"
mode = "execute"
expected = { must_succeed = true, stdout_contains = ["foo"] }
"#;
        let cfg: SuiteConfig = toml::from_str(raw).expect("suite parse");
        assert_eq!(cfg.test.len(), 1);
        assert_eq!(cfg.test[0].expected.must_succeed, Some(true));
        assert_eq!(
            cfg.test[0].expected.stdout_contains,
            vec!["foo".to_string()]
        );
        assert_eq!(
            cfg.shell_context.aliases.get("ll").map(String::as_str),
            Some("ls -lah")
        );
    }
}
