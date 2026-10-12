/// Shannon entropy secret scrubber and sensitivity classifier.
/// Computes H(X) = -sum(P(x_i) * log2(P(x_i))).
/// Standard threshold: H >= 4.5 on strings >= 16 chars suggests random keys/hashes/tokens.
pub const ENTROPY_THRESHOLD: f64 = 4.5;
pub const MIN_SENSITIVE_LENGTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntropyAnalysis {
    pub score: f64,
    pub is_likely_secret: bool,
    pub detected_pattern: Option<&'static str>,
}

/// Calculate Shannon entropy for a byte slice.
pub fn calculate_shannon_entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }

    let mut counts = [0usize; 256];
    for &b in data {
        counts[b as usize] += 1;
    }

    let len = data.len() as f64;
    let mut entropy = 0.0;

    for &count in &counts {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }

    entropy
}

/// Analyze text snippet for high entropy or known secret patterns
/// (API keys, Private keys, JWT tokens, AWS credentials).
pub fn analyze_snippet(text: &str) -> EntropyAnalysis {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return EntropyAnalysis {
            score: 0.0,
            is_likely_secret: false,
            detected_pattern: None,
        };
    }

    // Heuristic pattern checks across entire snippet or individual tokens
    let mut detected_pattern = None;

    if trimmed.contains("-----BEGIN") && trimmed.contains("KEY-----") {
        detected_pattern = Some("Asymmetric Private Key (PEM)");
    } else {
        for word in trimmed.split(|c: char| c.is_whitespace() || c == '=' || c == ':' || c == '"' || c == '\'') {
            let w = word.trim();
            if w.starts_with("AKIA") || w.starts_with("ASIA") {
                detected_pattern = Some("AWS Access Key ID");
                break;
            } else if w.starts_with("ghp_") || w.starts_with("github_pat_") {
                detected_pattern = Some("GitHub Personal Access Token");
                break;
            } else if w.starts_with("sk_live_") || w.starts_with("sk_test_") {
                detected_pattern = Some("Stripe Secret Key");
                break;
            } else if w.starts_with("eyJ") && w.matches('.').count() == 2 {
                detected_pattern = Some("JSON Web Token (JWT)");
                break;
            } else if w.starts_with("xoxb-") || w.starts_with("xoxp-") {
                detected_pattern = Some("Slack API Token");
                break;
            }
        }
    }

    let score = calculate_shannon_entropy(trimmed.as_bytes());

    // Also check token-level entropy
    let mut has_high_entropy_token = false;
    for word in trimmed.split_whitespace() {
        if word.len() >= MIN_SENSITIVE_LENGTH && calculate_shannon_entropy(word.as_bytes()) >= ENTROPY_THRESHOLD {
            has_high_entropy_token = true;
            break;
        }
    }

    let is_likely_secret = detected_pattern.is_some()
        || has_high_entropy_token
        || (trimmed.len() >= MIN_SENSITIVE_LENGTH
            && score >= ENTROPY_THRESHOLD
            && !trimmed.contains(' '));

    EntropyAnalysis {
        score,
        is_likely_secret,
        detected_pattern,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shannon_entropy_calculation() {
        // Uniform string has 0 entropy
        let uniform = b"aaaaaaaaaaaaaaaa";
        assert_eq!(calculate_shannon_entropy(uniform), 0.0);

        // High entropy random string
        let high_entropy = "4f8a3d1b9e2c7a5f6b0d8e1c3a7f9b2d";
        let score = calculate_shannon_entropy(high_entropy.as_bytes());
        assert!(score >= 3.5);

        // Known secrets detection
        let analysis_gh = analyze_snippet("ghp_1234567890abcdefghijklmnopqrstuvwxyz");
        assert!(analysis_gh.is_likely_secret);
        assert_eq!(analysis_gh.detected_pattern, Some("GitHub Personal Access Token"));

        let analysis_rsa = analyze_snippet("-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...");
        assert!(analysis_rsa.is_likely_secret);
        assert_eq!(analysis_rsa.detected_pattern, Some("Asymmetric Private Key (PEM)"));
    }

    #[test]
    fn test_normal_text_not_flagged() {
        let normal = "The quick brown fox jumps over the lazy dog in the sunny park today.";
        let analysis = analyze_snippet(normal);
        assert!(!analysis.is_likely_secret);
    }
}
