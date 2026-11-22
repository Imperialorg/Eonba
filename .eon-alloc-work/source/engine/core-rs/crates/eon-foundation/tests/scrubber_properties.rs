//! Property tests for the journal scrubber (docs/implementation/01, section 3.4).

use eon_foundation::scrubber::{scrub, shannon_bits_per_char, strip_invisible};
use proptest::prelude::*;

fn mixed_secret_body(len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
                .chars()
                .collect::<Vec<char>>(),
        ),
        len,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A second pass must not change anything. Otherwise repeated journaling would keep rewriting the log.
    #[test]
    fn scrubbing_is_idempotent(input in "[ -~\u{200B}\u{202E}\u{E0041}]{0,200}") {
        let once = scrub(&input);
        prop_assert_eq!(scrub(&once), once);
    }

    /// A GitHub-style token must never survive, wherever it appears in surrounding text.
    #[test]
    fn github_tokens_never_survive(
        body in mixed_secret_body(36),
        before in "[a-z ]{0,30}",
        after in "[a-z ]{0,30}",
    ) {
        let secret = format!("ghp_{body}");
        let scrubbed = scrub(&format!("{before} {secret} {after}"));
        prop_assert!(!scrubbed.contains(&body), "secret body leaked: {scrubbed}");
    }

    /// Workspace paths made of ordinary words must stay readable so Ciel can cite them.
    /// Segments are long enough that the joined path always clears the 24-character minimum by construction.
    #[test]
    fn workspace_paths_are_preserved(
        segments in prop::collection::vec(
            prop_oneof!["[a-z][a-z0-9_-]{7,15}", "[A-Z][a-z]{7,10}"],
            3..6,
        ),
    ) {
        let path = segments.join("/");
        prop_assert_eq!(scrub(&path), path);
    }

    /// High-entropy base64-style strings with `/` must be redacted even though they contain path separators.
    /// The fixed prefix guarantees upper, lower, and digit classes. Bodies are long enough that
    /// random draws reliably exceed the entropy threshold, so the filter rarely rejects.
    #[test]
    fn high_entropy_slash_strings_are_redacted(rest in "[A-Za-z0-9/]{60,80}") {
        let body = format!("Aa1{rest}");
        prop_assume!(shannon_bits_per_char(&body) > 4.65);
        let scrubbed = scrub(&format!("x {body} y"));
        prop_assert!(!scrubbed.contains(&body), "high-entropy string survived: {scrubbed}");
    }

    /// Equal secrets must map to equal markers so two redacted values can still be compared.
    #[test]
    fn equal_secrets_compare_equal_after_scrubbing(body in mixed_secret_body(40)) {
        let secret = format!("xoxb-{body}");
        let a = scrub(&format!("first {secret}"));
        let b = scrub(&format!("second {secret}"));
        let marker_a = a.split_whitespace().last().unwrap().to_string();
        let marker_b = b.split_whitespace().last().unwrap().to_string();
        prop_assert_eq!(marker_a, marker_b);
    }

    /// Invisible code points are removed and visible text is kept in order.
    #[test]
    fn invisible_strip_keeps_visible_text(visible in "[a-zA-Z0-9 ]{0,80}", hidden in prop::collection::vec(0usize..4, 0..20)) {
        let invisible = ['\u{200B}', '\u{202E}', '\u{FEFF}', '\u{E0041}'];
        let mut noisy = String::new();
        let mut chars = visible.chars();
        for code in hidden {
            noisy.push(invisible[code]);
            if let Some(c) = chars.next() {
                noisy.push(c);
            }
        }
        noisy.extend(chars);
        prop_assert_eq!(strip_invisible(&noisy), visible);
    }
}
