#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub first_token: Option<String>,
    pub tokens: Vec<String>,
    pub has_pipeline: bool,
    pub has_control_operators: bool,
    pub has_redirection: bool,
    pub has_command_substitution: bool,
    pub has_grouping: bool,
    pub trailing_operator: bool,
    pub unbalanced_quotes: bool,
    pub unbalanced_grouping: bool,
    pub ambiguous: bool,
    pub warnings: Vec<String>,
}

impl ParsedCommand {
    pub fn has_risky_constructs(&self) -> bool {
        self.has_pipeline
            || self.has_control_operators
            || self.has_redirection
            || self.has_command_substitution
    }
}

#[derive(Debug, Default)]
struct ScanResult {
    tokens: Vec<String>,
    has_pipeline: bool,
    has_control_operators: bool,
    has_redirection: bool,
    has_command_substitution: bool,
    has_grouping: bool,
    trailing_operator: bool,
    unbalanced_quotes: bool,
    unbalanced_grouping: bool,
    warnings: Vec<String>,
}

pub fn first_significant_token(cmd: &str) -> Option<String> {
    parse_command(cmd).first_token
}

pub fn parse_command(cmd: &str) -> ParsedCommand {
    let scan = scan_shell(cmd);
    let first_token = extract_first_significant_token(&scan.tokens);
    let ambiguous = scan.unbalanced_grouping
        || scan.unbalanced_quotes
        || scan.trailing_operator
        || first_token.is_none();

    ParsedCommand {
        first_token,
        tokens: scan.tokens,
        has_pipeline: scan.has_pipeline,
        has_control_operators: scan.has_control_operators,
        has_redirection: scan.has_redirection,
        has_command_substitution: scan.has_command_substitution,
        has_grouping: scan.has_grouping,
        trailing_operator: scan.trailing_operator,
        unbalanced_quotes: scan.unbalanced_quotes,
        unbalanced_grouping: scan.unbalanced_grouping,
        ambiguous,
        warnings: scan.warnings,
    }
}

fn scan_shell(input: &str) -> ScanResult {
    let mut out = ScanResult::default();
    let mut chars = input.chars().peekable();
    let mut token = String::new();

    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut last_was_operator = false;

    while let Some(ch) = chars.next() {
        if escaped {
            token.push(ch);
            escaped = false;
            last_was_operator = false;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                token.push(ch);
            }
            continue;
        }

        if in_double_quote {
            match ch {
                '"' => in_double_quote = false,
                '\\' => escaped = true,
                _ => token.push(ch),
            }
            continue;
        }

        match ch {
            '\\' => {
                escaped = true;
                last_was_operator = false;
            }
            '\'' => {
                in_single_quote = true;
                last_was_operator = false;
            }
            '"' => {
                in_double_quote = true;
                last_was_operator = false;
            }
            '$' => {
                if matches!(chars.peek(), Some('(')) {
                    out.has_command_substitution = true;
                    out.has_grouping = true;
                    paren_depth = paren_depth.saturating_add(1);
                    token.push('$');
                    token.push('(');
                    chars.next();
                } else {
                    token.push(ch);
                }
                last_was_operator = false;
            }
            '(' => {
                out.has_grouping = true;
                paren_depth = paren_depth.saturating_add(1);
                if !token.is_empty() {
                    token.push(ch);
                }
                last_was_operator = false;
            }
            ')' => {
                out.has_grouping = true;
                if paren_depth == 0 {
                    out.unbalanced_grouping = true;
                    out.warnings
                        .push("unbalanced closing parenthesis".to_string());
                } else {
                    paren_depth -= 1;
                }
                if !token.is_empty() {
                    token.push(ch);
                }
                last_was_operator = false;
            }
            '|' => {
                flush_token(&mut token, &mut out.tokens);
                if matches!(chars.peek(), Some('|')) {
                    chars.next();
                    out.has_control_operators = true;
                } else {
                    out.has_pipeline = true;
                }
                last_was_operator = true;
            }
            '&' => {
                flush_token(&mut token, &mut out.tokens);
                if matches!(chars.peek(), Some('&')) {
                    chars.next();
                }
                out.has_control_operators = true;
                last_was_operator = true;
            }
            ';' => {
                flush_token(&mut token, &mut out.tokens);
                out.has_control_operators = true;
                last_was_operator = true;
            }
            '>' | '<' => {
                flush_token(&mut token, &mut out.tokens);
                out.has_redirection = true;
                if matches!(chars.peek(), Some('>') | Some('<')) {
                    chars.next();
                }
                last_was_operator = true;
            }
            c if c.is_whitespace() => {
                flush_token(&mut token, &mut out.tokens);
                if !out.tokens.is_empty() {
                    last_was_operator = false;
                }
            }
            _ => {
                token.push(ch);
                last_was_operator = false;
            }
        }
    }

    flush_token(&mut token, &mut out.tokens);

    if in_single_quote || in_double_quote || escaped {
        out.unbalanced_quotes = true;
        out.warnings
            .push("unbalanced or unterminated quoting".to_string());
    }
    if paren_depth > 0 {
        out.unbalanced_grouping = true;
        out.warnings
            .push("unbalanced grouping parentheses".to_string());
    }
    if last_was_operator {
        out.trailing_operator = true;
        out.warnings
            .push("command ends with a shell operator".to_string());
    }
    out
}

fn flush_token(token: &mut String, out: &mut Vec<String>) {
    let t = token.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    token.clear();
}

fn extract_first_significant_token(tokens: &[String]) -> Option<String> {
    let mut skipping_sudo_options = false;
    for token in tokens {
        if token == "sudo" {
            skipping_sudo_options = true;
            continue;
        }
        if skipping_sudo_options && token.starts_with('-') {
            continue;
        }
        skipping_sudo_options = false;
        if token == "env" {
            continue;
        }
        if is_env_assignment(token) {
            continue;
        }
        return Some(token.to_string());
    }
    None
}

fn is_env_assignment(token: &str) -> bool {
    let Some((lhs, _rhs)) = token.split_once('=') else {
        return false;
    };
    if lhs.is_empty() {
        return false;
    }
    let mut chars = lhs.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_first_significant_token() {
        assert_eq!(
            first_significant_token("sudo -E rm -rf x"),
            Some("rm".to_string())
        );
        assert_eq!(
            first_significant_token("sudo rm -rf x"),
            Some("rm".to_string())
        );
        assert_eq!(
            first_significant_token("env FOO=1 BAR=2 myprog --x"),
            Some("myprog".to_string())
        );
        assert_eq!(
            first_significant_token("( cd /tmp && ls )"),
            Some("cd".to_string())
        );
        assert_eq!(first_significant_token("ll"), Some("ll".to_string()));
    }

    #[test]
    fn detects_ambiguous_shell_forms() {
        let parsed = parse_command("grep \"foo");
        assert!(parsed.ambiguous);
        assert!(parsed.unbalanced_quotes);

        let parsed2 = parse_command("ls -la |");
        assert!(parsed2.ambiguous);
        assert!(parsed2.trailing_operator);
    }

    #[test]
    fn tracks_risky_constructs() {
        let parsed = parse_command("cat file | rg foo && echo done > out.txt");
        assert!(parsed.has_pipeline);
        assert!(parsed.has_control_operators);
        assert!(parsed.has_redirection);
        assert!(parsed.has_risky_constructs());
    }
}
