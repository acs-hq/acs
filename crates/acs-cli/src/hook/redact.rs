//! Redaction of command text before it is stored.
//!
//! Event details are written to the local log and, through session records, may end up on the git
//! remote (SPEC section 4.3), so a command line is stored only for denials and prompts, redacted
//! and truncated. This is a heuristic, not a guarantee: it masks the shapes that leak most often
//! (assignments and flags that name a secret, tokens with well-known prefixes, credentials in
//! URLs, bearer tokens). WP4.1 applies a second pass when it assembles records.

/// The longest command text kept, in characters.
pub const MAX_COMMAND_CHARS: usize = 200;

const SENSITIVE_WORDS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "auth",
    "credential",
    "cookie",
];

const TOKEN_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "sk-",
    "xoxb-",
    "xoxa-",
    "xoxp-",
    "AKIA",
    "AIza",
];

fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_WORDS.iter().any(|word| lower.contains(word))
}

fn mask_url_password(token: &str) -> Option<String> {
    let (scheme, rest) = token.split_once("://")?;
    let (userinfo, tail) = rest.split_once('@')?;
    if userinfo.contains('/') {
        return None;
    }
    match userinfo.split_once(':') {
        Some((user, _password)) => Some(format!("{scheme}://{user}:***@{tail}")),
        // A bare token as the user name (`https://ghp_...@github.com`).
        None if TOKEN_PREFIXES.iter().any(|p| userinfo.starts_with(p)) => {
            Some(format!("{scheme}://***@{tail}"))
        }
        None => None,
    }
}

/// Redacts and truncates a command line for storage.
#[must_use]
pub fn redact_command(command: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut mask_next = 0usize;
    for token in command.split_whitespace() {
        if mask_next > 0 {
            mask_next -= 1;
            out.push("***".to_owned());
            continue;
        }
        let masked = if let Some((name, _value)) = token.split_once('=')
            && is_sensitive_name(name.trim_start_matches('-'))
        {
            format!("{name}=***")
        } else if (token.starts_with('-') && is_sensitive_name(token))
            || token.eq_ignore_ascii_case("bearer")
            || token.eq_ignore_ascii_case("basic")
        {
            // `--password hunter2`, `Bearer <token>`: the next word is the secret.
            mask_next = 1;
            token.to_owned()
        } else if token.ends_with(':') && is_sensitive_name(token) {
            // `Authorization: Bearer <token>`: the scheme and the credential.
            mask_next = 2;
            token.to_owned()
        } else if TOKEN_PREFIXES
            .iter()
            .any(|prefix| token.trim_matches(['"', '\'']).starts_with(prefix))
        {
            "***".to_owned()
        } else if let Some(url) = mask_url_password(token) {
            url
        } else {
            token.to_owned()
        };
        out.push(masked);
    }
    let joined = out.join(" ");
    if joined.chars().count() > MAX_COMMAND_CHARS {
        let mut cut: String = joined.chars().take(MAX_COMMAND_CHARS).collect();
        cut.push('…');
        cut
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_commands_pass_through_with_normalized_spaces() {
        assert_eq!(redact_command("rm  -rf   build"), "rm -rf build");
    }

    #[test]
    fn assignments_and_flags_that_name_secrets_are_masked() {
        assert_eq!(
            redact_command("API_TOKEN=abc123 curl https://x --password=hunter2"),
            "API_TOKEN=*** curl https://x --password=***"
        );
        assert_eq!(
            redact_command("mysql --password hunter2 -u root"),
            "mysql --password *** -u root"
        );
    }

    #[test]
    fn bearer_tokens_and_known_prefixes_are_masked() {
        assert_eq!(
            redact_command(r#"curl -H "Authorization: Bearer abc.def" x"#),
            r#"curl -H "Authorization: *** *** x"#
        );
        assert_eq!(
            redact_command("git clone https://ghp_abcdef@github.com/x/y"),
            "git clone https://***@github.com/x/y"
        );
        assert_eq!(redact_command("echo ghp_abcdef sk-12345"), "echo *** ***");
    }

    #[test]
    fn credentials_in_urls_are_masked() {
        assert_eq!(
            redact_command("git push https://sara:s3cret@example.com/r.git"),
            "git push https://sara:***@example.com/r.git"
        );
    }

    #[test]
    fn long_commands_are_truncated() {
        let long = format!("echo {}", "x".repeat(500));
        let out = redact_command(&long);
        assert_eq!(out.chars().count(), MAX_COMMAND_CHARS + 1);
        assert!(out.ends_with('…'));
    }
}
