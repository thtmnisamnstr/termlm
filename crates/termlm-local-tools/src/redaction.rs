use regex::Regex;
use std::sync::OnceLock;

fn secret_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?im)(authorization\s*:\s*(?:bearer|token|basic)\s+)[^\s\r\n]+")
                .expect("regex"),
            Regex::new(r"(?im)(x-api-key\s*:\s*)[^\s\r\n]+").expect("regex"),
            Regex::new(r"(?im)(cookie\s*:\s*)[^\r\n]+").expect("regex"),
            Regex::new(r"(?im)(set-cookie\s*:\s*[^=;\r\n]+=)[^;\r\n]+").expect("regex"),
            Regex::new(r"(?i)(api[_-]?key\s*[=:]\s*)[A-Za-z0-9._\-]+").expect("regex"),
            Regex::new(r"(?i)(password\s*[=:]\s*)\S+").expect("regex"),
            Regex::new(r"(?i)(aws_secret_access_key\s*[=:]\s*)[A-Za-z0-9/+=]{16,}")
                .expect("regex"),
            Regex::new(
                r#"\b([A-Z][A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|API_KEY|ACCESS_KEY|PRIVATE_KEY)\s*=\s*)([^\s"']+)"#,
            )
            .expect("regex"),
            Regex::new(r"(?i)([?&](?:access_token|token|api_key|apikey|password)=)[^&\s]+")
                .expect("regex"),
            Regex::new(r"AKIA[0-9A-Z]{16}").expect("regex"),
            Regex::new(r"(?i)gh[pousr]_[A-Za-z0-9]{20,}").expect("regex"),
            Regex::new(r"(?i)github_pat_[A-Za-z0-9_]+").expect("regex"),
            Regex::new(r"(?i)sk-[A-Za-z0-9_-]{16,}").expect("regex"),
            Regex::new(
                r"-----BEGIN (?:RSA|OPENSSH|EC|PGP) PRIVATE KEY-----[\s\S]*?-----END (?:RSA|OPENSSH|EC|PGP) PRIVATE KEY-----",
            )
            .expect("regex"),
        ]
    })
}

pub fn redact_secrets(input: &str) -> String {
    let mut out = input.to_string();
    static DB_URL_PASSWORD_RE: OnceLock<Regex> = OnceLock::new();
    let db_url_re = DB_URL_PASSWORD_RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b((?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|redis|amqp)://[^/\s:@]+:)[^@/\s]+(@)",
        )
        .expect("regex")
    });
    out = db_url_re.replace_all(&out, "$1<redacted>$2").to_string();

    for re in secret_patterns() {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                if caps.len() > 1 {
                    let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    format!("{prefix}<redacted>")
                } else {
                    "<redacted>".to_string()
                }
            })
            .to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::redact_secrets;

    #[test]
    fn redacts_headers_and_cookies() {
        let input = "Authorization: Bearer abc123\nCookie: session=secret; id=1\nX-API-Key: xyz";
        let redacted = redact_secrets(input);
        assert!(redacted.contains("Authorization: Bearer <redacted>"));
        assert!(redacted.contains("Cookie: <redacted>"));
        assert!(redacted.contains("X-API-Key: <redacted>"));
    }

    #[test]
    fn redacts_secret_like_env_vars() {
        let input = "API_TOKEN=abc\nPASSWORD=hunter2\nNORMAL=value";
        let redacted = redact_secrets(input);
        assert!(redacted.contains("API_TOKEN=<redacted>"));
        assert!(redacted.contains("PASSWORD=<redacted>"));
        assert!(redacted.contains("NORMAL=value"));
    }

    #[test]
    fn redacts_database_urls_with_passwords() {
        let input = "postgres://alice:s3cret@localhost:5432/db";
        let redacted = redact_secrets(input);
        assert_eq!(redacted, "postgres://alice:<redacted>@localhost:5432/db");
    }

    #[test]
    fn redacts_private_key_blocks_and_tokens() {
        let input = "ghp_abcdefghijklmnopqrstuvwxyz123456\n-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----";
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("ghp_abcdefghijklmnopqrstuvwxyz123456"));
        assert!(redacted.contains("<redacted>"));
    }

    #[test]
    fn redacts_query_tokens() {
        let input = "https://x.test/path?access_token=abc123&keep=1";
        let redacted = redact_secrets(input);
        assert!(redacted.contains("access_token=<redacted>"));
        assert!(redacted.contains("&keep=1"));
    }
}
