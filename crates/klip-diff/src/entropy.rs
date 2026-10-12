//! Heuristic "does this clip look like a secret?" detector.
//!
//! It only decides how a clip is *presented* (masked preview in history, "sensitive" flag for
//! keyboards that honour it); it never blocks syncing. It combines three signals:
//!
//! 1. well-known token formats (PEM keys, AWS, GitHub, Stripe, Slack, JWT),
//! 2. a **length-aware** entropy test for random-looking tokens,
//! 3. context: words like `password=`/`token:` make short or hex values suspicious.
//!
//! Plain Shannon entropy with a fixed threshold does not work for short strings: a string of
//! `n` characters cannot have more than `log2(n)` bits per character, and a hex token can never
//! exceed 4.0. So the score is compared with what is *achievable* for that length and alphabet.

pub const MIN_TOKEN_LEN: usize = 20;
/// Fraction of the achievable entropy a random-looking token must reach.
pub const RELATIVE_ENTROPY: f64 = 0.85;
/// Kept for callers that want a quick whole-snippet score.
pub const MIN_SENSITIVE_LENGTH: usize = MIN_TOKEN_LEN;

const CONTEXT_WORDS: [&str; 9] = [
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "private_key",
    "bearer",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntropyAnalysis {
    pub score: f64,
    pub is_likely_secret: bool,
    pub detected_pattern: Option<&'static str>,
}

/// Shannon entropy in bits per byte.
pub fn calculate_shannon_entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let len = data.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

fn is_hex(token: &str) -> bool {
    token.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_uuid(token: &str) -> bool {
    let parts: Vec<&str> = token.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && is_hex(p))
}

fn char_classes(token: &str) -> usize {
    let lower = token.chars().any(|c| c.is_ascii_lowercase());
    let upper = token.chars().any(|c| c.is_ascii_uppercase());
    let digit = token.chars().any(|c| c.is_ascii_digit());
    let symbol = token.chars().any(|c| !c.is_ascii_alphanumeric());
    [lower, upper, digit, symbol].iter().filter(|b| **b).count()
}

/// Highest entropy (bits/char) a token of this length and alphabet could have.
fn achievable_entropy(token: &str) -> f64 {
    let distinct_possible = token.len().min(64) as f64;
    distinct_possible.log2()
}

fn looks_like_url_or_path(token: &str) -> bool {
    token.contains("://")
        || token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("~/")
        || token.matches('/').count() >= 3
}

/// Is this single token random-looking enough to be a credential?
fn random_looking(token: &str, context: bool) -> bool {
    let len = token.len();
    if len > 512 || looks_like_url_or_path(token) || is_uuid(token) {
        return false;
    }
    if is_hex(token) {
        // Hashes (git SHAs, checksums) are copied all the time and are not secrets, so hex only
        // counts when the text around it says it is a key/token/password.
        return context && len >= 12;
    }
    if context && len >= 12 && char_classes(token) >= 2 {
        return true;
    }
    if len < MIN_TOKEN_LEN || char_classes(token) < 3 {
        return false;
    }
    calculate_shannon_entropy(token.as_bytes()) >= RELATIVE_ENTROPY * achievable_entropy(token)
}

fn known_format(token: &str) -> Option<&'static str> {
    if (token.starts_with("AKIA") || token.starts_with("ASIA"))
        && token.len() >= 16
        && token.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Some("AWS Access Key ID");
    }
    if token.starts_with("ghp_")
        || token.starts_with("gho_")
        || token.starts_with("ghs_")
        || token.starts_with("github_pat_")
    {
        return Some("GitHub token");
    }
    if token.starts_with("sk_live_")
        || token.starts_with("sk_test_")
        || token.starts_with("rk_live_")
    {
        return Some("Stripe secret key");
    }
    if token.starts_with("eyJ") && token.matches('.').count() == 2 && token.len() > 30 {
        return Some("JSON Web Token (JWT)");
    }
    if token.starts_with("xoxb-") || token.starts_with("xoxp-") || token.starts_with("xapp-") {
        return Some("Slack token");
    }
    None
}

/// Analyze a clip's text. Never panics; empty text is simply "not secret".
pub fn analyze_snippet(text: &str) -> EntropyAnalysis {
    let trimmed = text.trim();
    let score = calculate_shannon_entropy(trimmed.as_bytes());
    if trimmed.is_empty() {
        return EntropyAnalysis {
            score,
            is_likely_secret: false,
            detected_pattern: None,
        };
    }

    if trimmed.contains("-----BEGIN") && trimmed.contains("PRIVATE KEY-----") {
        return EntropyAnalysis {
            score,
            is_likely_secret: true,
            detected_pattern: Some("Private key (PEM)"),
        };
    }

    let lower = trimmed.to_ascii_lowercase();
    let context = CONTEXT_WORDS.iter().any(|w| lower.contains(w));

    let tokens = || {
        trimmed
            .split(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '=' | ':'
                            | '"'
                            | '\''
                            | ','
                            | ';'
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '<'
                            | '>'
                    )
            })
            .filter(|t| !t.is_empty())
    };

    for token in tokens() {
        if let Some(pattern) = known_format(token) {
            return EntropyAnalysis {
                score,
                is_likely_secret: true,
                detected_pattern: Some(pattern),
            };
        }
    }
    let is_likely_secret = tokens().any(|t| random_looking(t, context));
    EntropyAnalysis {
        score,
        is_likely_secret,
        detected_pattern: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(s: &str) -> bool {
        analyze_snippet(s).is_likely_secret
    }

    #[test]
    fn entropy_basics() {
        assert_eq!(calculate_shannon_entropy(b""), 0.0);
        assert_eq!(calculate_shannon_entropy(b"aaaaaaaaaaaaaaaa"), 0.0);
        // 16 distinct symbols -> exactly 4 bits per symbol
        assert!((calculate_shannon_entropy(b"0123456789abcdef") - 4.0).abs() < 1e-9);
    }

    #[test]
    fn detects_known_formats_and_keys() {
        assert_eq!(
            analyze_snippet("ghp_1234567890abcdefghijklmnopqrstuvwxyz").detected_pattern,
            Some("GitHub token")
        );
        assert_eq!(
            analyze_snippet("AKIAIOSFODNN7EXAMPLE").detected_pattern,
            Some("AWS Access Key ID")
        );
        assert_eq!(
            analyze_snippet("-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...")
                .detected_pattern,
            Some("Private key (PEM)")
        );
        assert!(secret("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"));
        // Assembled at compile time so that no contiguous key-shaped literal sits in the source
        // (secret scanners flag even documentation examples).
        assert!(secret(concat!(
            "Authorization: sk_",
            "live_",
            "4eC39HqLyjWDarjtT1zdp7dc"
        )));
    }

    #[test]
    fn detects_random_tokens_of_realistic_lengths() {
        // base64-ish API secrets of different lengths (the old fixed 4.5 threshold missed all of
        // the shorter ones)
        assert!(secret("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"));
        assert!(secret("Zm9vYmFyQmF6UXV4MTIzNDU2Nzg5MEFiQ2RFZg"));
        assert!(secret("tK8dQ2xV9pLm4RzW7nB3yH6cF1jS5gUa"));
        assert!(secret("A7f$k2Lm9Qp!xV3zR8tYw1Nd"));
        // a bare random token on its own
        assert!(secret("q8W3eR7tY2uI9oP4aS6dF1gH5jK0lZxC"));
    }

    #[test]
    fn hex_secrets_need_context_but_hashes_do_not_trigger() {
        // Git SHA, SHA-256, MD5, UUID: copied constantly, not secrets.
        assert!(!secret("3f8a4c1d9e2b7a5f6b0d8e1c3a7f9b2d4e6f8a0b"));
        assert!(!secret(
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        ));
        assert!(!secret("d41d8cd98f00b204e9800998ecf8427e"));
        assert!(!secret("550e8400-e29b-41d4-a716-446655440000"));
        // The same hex with a context word is treated as a credential.
        assert!(secret("api_key=3f8a4c1d9e2b7a5f6b0d8e1c3a7f9b2d"));
        assert!(secret("password: 9f86d081884c7d659a2f"));
    }

    #[test]
    fn context_words_catch_short_passwords() {
        assert!(secret("password = Tr0ub4dor&3xyz"));
        assert!(secret("export API_TOKEN='a1B2c3D4e5F6'"));
    }

    #[test]
    fn ordinary_clips_are_not_flagged() {
        for s in [
            "The quick brown fox jumps over the lazy dog in the sunny park today.",
            "https://github.com/Tribal-Chief-001/Klip/pull/12#issuecomment-123456789",
            "let value = compute_something_useful(argument_one, argument_two);",
            "/usr/lib/x86_64-linux-gnu/libfoo.so.1.2.3",
            "getUserAccountDetailsFromRemoteService",
            "internationalizationConfigurationManager",
            "meeting at 10:30 in room B-204, bring the laptop",
            "error[E0382]: borrow of moved value: `config`",
            "hello",
            "",
            "   ",
        ] {
            assert!(!secret(s), "false positive: {s:?}");
        }
    }

    #[test]
    fn never_panics_on_odd_input() {
        for s in [
            "\u{0}\u{1}\u{2}",
            "日本語のテキストをここに貼り付けます",
            &"a".repeat(100_000),
            &"é".repeat(300),
        ] {
            let _ = analyze_snippet(s);
        }
    }
}
