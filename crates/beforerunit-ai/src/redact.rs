//! Scrubs secrets from a command line before it leaves the machine.

use std::sync::LazyLock;

use regex::Regex;

pub const MASK: &str = "[REDACTED]";

struct Rule {
    re: Regex,
    rep: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let r = |p: &str, rep: &'static str| Rule { re: Regex::new(p).expect("valid redaction regex"), rep };
    vec![
        // Authorization / API key headers.
        r(r#"(?i)(authorization\s*:\s*(?:bearer|basic|token)?\s*)("[^"]*"|'[^']*'|[^\s"']+)"#, "${1}[REDACTED]"),
        r(r#"(?i)((?:x-)?(?:api|auth)[-_]?(?:key|token)\s*:\s*)("[^"]*"|'[^']*'|[^\s"']+)"#, "${1}[REDACTED]"),
        // Credentials inside URLs: scheme://user:pass@host
        r(r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/\s:@]+:[^/\s@]+@", "${1}[REDACTED]@"),
        // Secret-looking env assignments: GITHUB_TOKEN=..., DB_PASSWORD="..."
        r(
            r#"(?i)\b([a-z0-9_]*(?:token|secret|passw(?:or)?d|api_?key|access_?key|private_?key|credentials?)[a-z0-9_]*=)("[^"]*"|'[^']*'|\S+)"#,
            "${1}[REDACTED]",
        ),
        // Secret-looking flags: --password x, --token=x, --api-key x
        r(
            r#"(?i)(--?(?:password|passwd|pass|token|secret|api-?key|auth-?token|access-?key|client-?secret)(?:=|\s+))("[^"]*"|'[^']*'|\S+)"#,
            "${1}[REDACTED]",
        ),
        // `mysql -pSECRET`
        r(r"(\bmysql\S*\s(?:.*\s)?-p)(\S+)", "${1}[REDACTED]"),
        // curl -u user:pass
        r(r#"(\s-u\s+|--user[=\s]+)([^\s:"']+):("[^"]*"|'[^']*'|\S+)"#, "${1}${2}:[REDACTED]"),
        // Well-known token formats.
        r(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", MASK),
        r(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}\b", MASK),
        r(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b", MASK),
        r(r"\bglpat-[A-Za-z0-9_\-]{16,}\b", MASK),
        r(r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b", MASK),
        r(r"\b(?:sk|pk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b", MASK),
        r(r"\bsk-(?:proj-|ant-)?[A-Za-z0-9_\-]{20,}\b", MASK),
        r(r"\bgsk_[A-Za-z0-9]{20,}\b", MASK),
        r(r"\bhf_[A-Za-z0-9]{20,}\b", MASK),
        r(r"\bAIza[0-9A-Za-z_\-]{30,}\b", MASK),
        r(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{4,}\b", MASK),
        r(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)", MASK),
    ]
});

/// Returns the command with secrets replaced by `[REDACTED]`.
pub fn redact(cmd: &str) -> String {
    let mut s = cmd.to_string();
    for rule in RULES.iter() {
        if rule.re.is_match(&s) {
            s = rule.re.replace_all(&s, rule.rep).into_owned();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn scrubs_common_secrets() {
        let cases = [
            (r#"curl -H "Authorization: Bearer abc.def.ghi" https://x"#, "abc.def.ghi"),
            ("GITHUB_TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaa make", "ghp_aaaa"),
            ("git clone https://bob:hunter2@github.com/x/y", "hunter2"),
            ("aws configure set aws_secret_access_key=wJalrXUtnFEMI/K7MDENG", "wJalrXUtnFEMI"),
            ("mysql -u root -pSuperSecret db", "SuperSecret"),
            ("deploy --token=s3cr3t-value", "s3cr3t-value"),
            ("export OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwx", "abcdefghijklmnop"),
            ("curl -u admin:pa55 https://x", "pa55"),
            ("echo AKIAIOSFODNN7EXAMPLE", "AKIAIOSFODNN7EXAMPLE"),
        ];
        for (input, secret) in cases {
            let out = redact(input);
            assert!(!out.contains(secret), "{input} -> {out}");
            assert!(out.contains("[REDACTED]"), "{input} -> {out}");
        }
    }

    #[test]
    fn leaves_normal_commands_alone() {
        for c in ["rm -rf build", "pwd", "git push origin main", "ls -la ~/passwords_folder"] {
            assert_eq!(redact(c), c);
        }
    }
}
