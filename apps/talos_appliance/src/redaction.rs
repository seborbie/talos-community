const SENSITIVE_KEYS: &[&str] = &[
    "APP_ENCRYPTION_KEY",
    "DATABASE_URL",
    "JWT_SECRET",
    "POSTGRES_PASSWORD",
    "RMM_SERVER_API_KEY",
    "TALOS_DATABASE_URL",
    "TALOS_POSTGRES_PASSWORD",
];

pub fn redact_text(input: &str, secret_values: &[&str]) -> String {
    let mut output = input.to_string();
    let mut values: Vec<_> = secret_values
        .iter()
        .copied()
        .filter(|value| value.len() >= 4)
        .collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    for value in values {
        output = output.replace(value, "[REDACTED]");
    }

    let mut redacted_lines = Vec::new();
    for line in output.lines() {
        let upper = line.to_ascii_uppercase();
        let sensitive = SENSITIVE_KEYS.iter().find_map(|key| {
            upper.find(key).and_then(|index| {
                line[index + key.len()..]
                    .find(['=', ':'])
                    .map(|offset| index + key.len() + offset)
            })
        });
        if let Some(separator) = sensitive {
            redacted_lines.push(format!("{}=[REDACTED]", &line[..separator]));
        } else {
            redacted_lines.push(redact_database_urls(line));
        }
    }
    let mut result = redacted_lines.join("\n");
    if input.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn redact_database_urls(input: &str) -> String {
    let mut output = input.to_string();
    for scheme in ["postgresql://", "postgres://"] {
        while let Some(start) = output.find(scheme) {
            let tail = &output[start..];
            let end_offset = tail
                .find(|character: char| {
                    character.is_whitespace()
                        || matches!(character, '"' | '\'' | ',' | ']' | '}' | ')')
                })
                .unwrap_or(tail.len());
            output.replace_range(start..start + end_offset, "[REDACTED_DATABASE_URL]");
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_exact_secrets_database_urls_and_sensitive_assignments() {
        let input = "token=abcdef1234\nDATABASE_URL=postgresql://user:pass@db/talos\nerror connecting to postgres://other:secret@host/db\nJWT_SECRET: visible";
        let redacted = redact_text(input, &["abcdef1234"]);
        assert!(!redacted.contains("abcdef1234"));
        assert!(!redacted.contains("user:pass"));
        assert!(!redacted.contains("other:secret"));
        assert!(!redacted.contains("visible"));
        assert!(redacted.contains("[REDACTED]"));
    }
}
