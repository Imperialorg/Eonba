//! Three-stage journal scrubber (docs/implementation/01, section 3.4).
//!
//! Stage 0 strips invisible Unicode steganography. Stage 1 redacts known secret
//! prefixes in one Aho-Corasick pass. Stage 2 redacts high-entropy tokens that
//! are not paths, URLs, or hashes. Every redaction is replaced by a keyed-free
//! BLAKE3 tag so equal secrets still compare equal without being revealed.

use std::sync::OnceLock;

use aho_corasick::AhoCorasick;

const KNOWN_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "github_pat_",
    "sk-proj-",
    "sk-ant-",
    "AKIA",
    "ASIA",
    "AIza",
    "xoxb-",
    "xoxp-",
    "eyJhbGci",
    "-----BEGIN ",
    "Bearer ",
];

const BASE64_ENTROPY_BITS: f64 = 4.65;
const HEX_ENTROPY_BITS: f64 = 3.75;
const MIN_TOKEN_LEN: usize = 24;
const PATH_SEGMENT_MAX: usize = 24;

/// Removes Unicode Tag, zero-width, and bidi-override code points that can hide instructions.
pub fn strip_invisible(input: &str) -> String {
    input.chars().filter(|&c| !is_invisible(c)).collect()
}

fn is_invisible(c: char) -> bool {
    matches!(
        c as u32,
        0xE0000..=0xE007F | 0x200B..=0x200F | 0xFEFF | 0x202A..=0x202E | 0x2066..=0x2069
    )
}

fn known_prefixes() -> &'static AhoCorasick {
    static MATCHER: OnceLock<AhoCorasick> = OnceLock::new();
    MATCHER.get_or_init(|| AhoCorasick::new(KNOWN_PREFIXES).expect("static prefixes are valid"))
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')
}

/// Shannon entropy in bits per byte.
pub fn shannon_bits_per_char(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    for &b in s.as_bytes() {
        counts[b as usize] += 1;
    }
    let len = s.len() as f64;
    counts
        .iter()
        .filter(|&&n| n > 0)
        .map(|&n| {
            let p = n as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// Returns the `[REDACTED_SECRET:b3_...]` marker for a secret, identical for identical secrets.
pub fn redaction_marker(secret: &str) -> String {
    let digest = blake3::hash(secret.as_bytes());
    let hex = digest.to_hex();
    format!("[REDACTED_SECRET:b3_{}]", &hex.as_str()[..8])
}

/// Applies all three stages and returns the scrubbed text.
pub fn scrub(input: &str) -> String {
    let stripped = strip_invisible(input);
    let after_prefixes = redact_known_prefixes(&stripped);
    redact_high_entropy(&after_prefixes)
}

fn redact_known_prefixes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    for hit in known_prefixes().find_iter(input) {
        if hit.start() < cursor || starts_inside_token(input, hit.start()) {
            continue;
        }
        let end = input[hit.end()..]
            .char_indices()
            .find(|&(_, c)| !is_token_char(c))
            .map(|(i, _)| hit.end() + i)
            .unwrap_or(input.len());
        out.push_str(&input[cursor..hit.start()]);
        out.push_str(&redaction_marker(&input[hit.start()..end]));
        cursor = end;
    }
    out.push_str(&input[cursor..]);
    out
}

/// A prefix such as `xoxb-` inside `axoxb--0` is part of a longer word, not the start of a secret.
fn starts_inside_token(input: &str, at: usize) -> bool {
    input[..at].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn redact_high_entropy(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut token_start: Option<usize> = None;
    for (i, c) in input.char_indices() {
        match (token_start, is_token_char(c)) {
            (None, true) => token_start = Some(i),
            (Some(start), false) => {
                out.push_str(&redact_token_if_secret(&input[..i], start));
                token_start = None;
                out.push(c);
            }
            (None, false) => out.push(c),
            (Some(_), true) => {}
        }
    }
    if let Some(start) = token_start {
        out.push_str(&redact_token_if_secret(input, start));
    }
    out
}

/// Redacts the token beginning at `start`. `text` must contain the token as its suffix.
fn redact_token_if_secret(text: &str, start: usize) -> String {
    let token = &text[start..];
    if is_secret_candidate(token, &text[..start]) {
        redaction_marker(token)
    } else {
        token.to_string()
    }
}

fn is_secret_candidate(token: &str, before: &str) -> bool {
    if token.len() < MIN_TOKEN_LEN || looks_like_path(token) {
        return false;
    }
    let has_upper = token.bytes().any(|b| b.is_ascii_uppercase());
    let has_lower = token.bytes().any(|b| b.is_ascii_lowercase());
    let has_digit = token.bytes().any(|b| b.is_ascii_digit());
    if !(has_upper && has_lower && has_digit) {
        return false;
    }
    let is_hex = token.bytes().all(|b| b.is_ascii_hexdigit());
    if is_hex && (token.len() == 40 || token.len() == 64) {
        let protected = before.ends_with("sha256:") || before.ends_with("commit ");
        return !protected && shannon_bits_per_char(token) > HEX_ENTROPY_BITS;
    }
    shannon_bits_per_char(token) > BASE64_ENTROPY_BITS
}

/// Workspace paths such as `crates/eon-cognition/src/tactical_slm` are `/`-separated words.
/// Random base64 also contains `/`, so each segment must read as a word: one optional leading capital,
/// then only lowercase letters, digits, `_`, `-`, or `.`.
fn looks_like_path(token: &str) -> bool {
    if !token.contains('/') || token.contains('+') || token.contains('=') {
        return false;
    }
    token
        .split('/')
        .filter(|segment| !segment.is_empty())
        .all(|segment| segment.len() <= PATH_SEGMENT_MAX && is_word_like(segment))
}

fn is_word_like(segment: &str) -> bool {
    let mut chars = segment.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_uppercase());
    first_ok
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULE_PATH: &str = "engine/core-rs/crates/eon-cognition/src/tactical_slm";

    #[test]
    fn invisible_code_points_are_stripped() {
        let hidden = "run\u{200B}this\u{E0041}\u{202E}now";
        assert_eq!(strip_invisible(hidden), "runthisnow");
    }

    #[test]
    fn github_token_is_redacted_by_prefix() {
        let line = "auth=ghp_FIXTURE-not-a-real-token end";
        let scrubbed = scrub(line);
        assert!(!scrubbed.contains("ghp_FIXTURE"));
        assert!(scrubbed.contains("[REDACTED_SECRET:b3_"));
        assert!(scrubbed.ends_with(" end"));
    }

    #[test]
    fn module_paths_survive_even_with_high_character_variety() {
        assert!(!scrub(MODULE_PATH).contains("REDACTED"));
    }

    #[test]
    fn hashes_are_exempt_when_labelled_or_commit_referenced() {
        let sha = "3f786850e387550fdab836ed7e6dc881de23001b9f1a2e4c7d8b9a0f1e2d3c4b";
        assert!(!scrub(&format!("sha256:{sha}")).contains("REDACTED"));
        assert!(!scrub(&format!("commit {}", &sha[..40])).contains("REDACTED"));
    }

    #[test]
    fn base64_secret_with_slashes_is_not_mistaken_for_a_path() {
        let scrubbed = scrub("key wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY end");
        assert!(!scrubbed.contains("bPxRfiCYEXAMPLEKEY"), "AWS-style key leaked: {scrubbed}");
    }

    #[test]
    fn leading_slash_fragment_after_a_dot_is_still_a_path() {
        assert!(!scrub("a./Src/Main/Lib/Module/Parser").contains("REDACTED"));
    }

    #[test]
    fn known_prefix_inside_a_longer_word_is_not_a_secret() {
        let path = "crates/axoxb--0/notes";
        assert_eq!(scrub(path), path);
    }

    #[test]
    fn unlabelled_random_token_is_redacted() {
        let token = "aB3dE5gH7jK9mN1pQ2rS4tU6vW8xY0zA1bC2dE3f";
        assert!(
            shannon_bits_per_char(token) > BASE64_ENTROPY_BITS,
            "fixture must be high-entropy"
        );
        assert!(scrub(&format!("token {token}")).contains("[REDACTED_SECRET:b3_"));
    }

    #[test]
    fn equal_secrets_produce_equal_markers() {
        let secret = "ghp_FIXTURE-not-a-real-token";
        assert_eq!(scrub(secret), scrub(secret));
        assert_ne!(
            scrub(secret),
            scrub("ghp_OTHERsecret0000000000000000000000")
        );
    }
}

