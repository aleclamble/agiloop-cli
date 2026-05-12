use regex::Regex;

pub fn redact_secrets(input: &str) -> String {
    let patterns = [
        r"(?i)(api[_-]?key|token|secret|password)=([^\s]+)",
        r"sk-[A-Za-z0-9_\-]{16,}",
        r"ghp_[A-Za-z0-9_]{16,}",
    ];

    patterns.iter().fold(input.to_string(), |acc, pattern| {
        let regex = Regex::new(pattern).unwrap();
        regex.replace_all(&acc, "$1=[REDACTED]").to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_secret_shapes() {
        let redacted = redact_secrets("token=abc123 sk-1234567890abcdefghijklmnop");

        assert!(redacted.contains("token=[REDACTED]"));
        assert!(!redacted.contains("abc123"));
    }
}
