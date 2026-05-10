use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyFloorMatch {
    pub pattern: &'static str,
    pub command: String,
}

static FLOOR_PATTERNS: &[&str] = &[
    r"^\s*rm\s+-[a-zA-Z]*r[a-zA-Z]*\s+/(\s|$)",
    r"^\s*rm\s+-[a-zA-Z]*r[a-zA-Z]*\s+(\$HOME|~)(/|\s|$)",
    r"^\s*rm\s+-[a-zA-Z]*r[a-zA-Z]*\s+/\*",
    r"^\s*:\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:",
    r"\bdd\s+.*of=/dev/(disk|rdisk|sd|nvme)",
    r">\s*/dev/(disk|rdisk|sd|nvme)",
    r"\bmkfs(\.\w+)?\s+/dev/",
    r"^\s*sudo\s+rm\s+-[a-zA-Z]*r",
    r"\brm\s+-[a-zA-Z]*r[a-zA-Z]*\s+/(System|Library|usr|bin|sbin|etc|var)(/|\s|$)",
    r"\b(chmod|chown)\s+-R\s+\S+\s+/(\s|$)",
    r">\s*/(System|Library|usr|bin|sbin|etc)/",
    r"\bdiskutil\s+(eraseDisk|eraseVolume|secureErase)",
    r"\bcsrutil\s+disable",
    r"\bspctl\s+--master-disable",
    r"\bnvram\s+-c",
];

static FLOOR_REGEXES: Lazy<Vec<Regex>> = Lazy::new(|| {
    FLOOR_PATTERNS
        .iter()
        .map(|p| Regex::new(p).expect("invalid floor regex"))
        .collect()
});

pub fn matches_safety_floor(cmd: &str) -> Option<SafetyFloorMatch> {
    FLOOR_REGEXES
        .iter()
        .zip(FLOOR_PATTERNS)
        .find(|(re, _)| re.is_match(cmd))
        .map(|(_, pat)| SafetyFloorMatch {
            pattern: pat,
            command: cmd.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_catastrophic_samples() {
        let blocked = [
            "rm -rf /",
            "sudo rm -rf /",
            "rm -rf $HOME/projects",
            "rm -rf ~",
            ":(){ :|:& };:",
            "dd if=/dev/zero of=/dev/disk0",
            "mkfs.ext4 /dev/sdb1",
            "diskutil eraseDisk JHFS+ X disk2",
            "spctl --master-disable",
            "nvram -c",
        ];

        for cmd in blocked {
            assert!(matches_safety_floor(cmd).is_some(), "should block: {cmd}");
        }
    }

    #[test]
    fn allows_common_non_catastrophic_samples() {
        let allowed = [
            "rm -rf ./build",
            "dd if=/dev/zero of=./bigfile bs=1M count=10",
            "chmod -R 755 ./mydir",
            "echo \"rm -rf /\" > note.txt",
        ];

        for cmd in allowed {
            assert!(matches_safety_floor(cmd).is_none(), "should allow: {cmd}");
        }
    }
}
