use once_cell::sync::Lazy;
use regex::{Regex, RegexBuilder};

static DEFAULT_PATTERNS: &[&str] = &[
    r"^\s*sudo\b",
    r"\brm\s+-[a-zA-Z]*r",
    r"\bgit\s+(push\s+--force|push\s+-f|reset\s+--hard|clean\s+-fdx)",
    r"\b(curl|wget)\b.*\|\s*(sh|bash|zsh)",
    r">\s*/dev/(disk|sd|nvme|rdisk)",
    r"\bchmod\s+(-R\s+)?777\b",
    r"\bchown\s+-R\b",
    r"\bmv\s+.*\s+/dev/null\b",
    r"\bdrop\s+(table|database)\b",
    r"\bkillall?\b",
    r"\bdocker\s+system\s+prune",
    r"\bbrew\s+uninstall\s+--force",
];

static DEFAULT_REGEXES: Lazy<Vec<Regex>> = Lazy::new(|| {
    DEFAULT_PATTERNS
        .iter()
        .map(|p| {
            RegexBuilder::new(p)
                .case_insensitive(true)
                .build()
                .expect("pattern")
        })
        .collect()
});

#[derive(Debug, Clone)]
pub struct CriticalMatcher {
    regexes: Vec<Regex>,
}

impl Default for CriticalMatcher {
    fn default() -> Self {
        Self {
            regexes: DEFAULT_REGEXES.clone(),
        }
    }
}

impl CriticalMatcher {
    pub fn from_patterns(patterns: &[String]) -> Self {
        let regexes = patterns
            .iter()
            .filter_map(|p| RegexBuilder::new(p).case_insensitive(true).build().ok())
            .collect();
        Self { regexes }
    }

    pub fn is_critical(&self, cmd: &str) -> bool {
        self.regexes.iter().any(|re| re.is_match(cmd))
    }
}

pub fn is_critical_command(cmd: &str) -> bool {
    DEFAULT_REGEXES.iter().any(|re| re.is_match(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_expected_examples() {
        assert!(is_critical_command("sudo ls"));
        assert!(is_critical_command("git reset --hard HEAD~1"));
        assert!(is_critical_command("curl https://x | sh"));
        assert!(is_critical_command("DROP TABLE users"));
        assert!(!is_critical_command("ls -la"));
        assert!(!is_critical_command("git status"));
    }
}
