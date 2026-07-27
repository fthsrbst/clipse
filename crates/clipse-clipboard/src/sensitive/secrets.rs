use std::sync::LazyLock;

use regex::Regex;

/// Which family of secret a `detect_secret` hit matched. Carried in
/// `SuppressionReason::DetectedSecret` for logging — never the matched text
/// itself, so a log line can say "suppressed a GitHub token" without ever
/// writing the token to disk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SecretKind {
    Jwt,
    AwsAccessKey,
    GitHubToken,
    SlackToken,
    StripeKey,
    OpenAiKey,
    GoogleApiKey,
    PrivateKeyPem,
    CreditCard,
}

// All patterns are anchored to a recognizable, low-collision prefix (AKIA,
// ghp_, xox[baprs]-, sk_live_/rk_live_, AIza, sk-) and use bounded, linear
// character classes — `regex` guarantees linear-time matching for these (no
// backreferences, no nested quantifiers), so this stays cheap to run on
// every clipboard change.
static JWT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}").unwrap()
});
static AWS_KEY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap());
static GITHUB_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:ghp|gho|ghs)_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{22,}\b").unwrap()
});
static SLACK_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b").unwrap());
static STRIPE_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}\b").unwrap());
static OPENAI_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{20,}\b").unwrap());
static GOOGLE_API_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bAIza[0-9A-Za-z_-]{35}\b").unwrap());
static PEM_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap());

/// Scan text for anything that looks like a secret. Returns the first kind
/// found; order below is cheapest-and-most-specific first so an obvious hit
/// (PEM header, a fixed-prefix key) short-circuits before the more general
/// credit-card scan runs.
pub fn detect_secret(text: &str) -> Option<SecretKind> {
    if PEM_HEADER_RE.is_match(text) {
        return Some(SecretKind::PrivateKeyPem);
    }
    if AWS_KEY_RE.is_match(text) {
        return Some(SecretKind::AwsAccessKey);
    }
    if GITHUB_TOKEN_RE.is_match(text) {
        return Some(SecretKind::GitHubToken);
    }
    if SLACK_TOKEN_RE.is_match(text) {
        return Some(SecretKind::SlackToken);
    }
    if STRIPE_KEY_RE.is_match(text) {
        return Some(SecretKind::StripeKey);
    }
    if OPENAI_KEY_RE.is_match(text) {
        return Some(SecretKind::OpenAiKey);
    }
    if GOOGLE_API_KEY_RE.is_match(text) {
        return Some(SecretKind::GoogleApiKey);
    }
    if looks_like_jwt(text) {
        return Some(SecretKind::Jwt);
    }
    if contains_luhn_valid_card_number(text) {
        return Some(SecretKind::CreditCard);
    }
    None
}

/// A three-segment base64url match is not enough on its own — plenty of
/// dotted identifiers (namespaced config keys, versioned filenames) fit that
/// shape. We additionally require the first segment to decode as base64url
/// and contain the literal `"alg"` claim, which every JWT header has and
/// which unrelated dotted text essentially never decodes to by chance.
fn looks_like_jwt(text: &str) -> bool {
    for m in JWT_RE.find_iter(text) {
        let header = m.as_str().split('.').next().unwrap_or("");
        if let Some(decoded) = base64url_decode(header)
            && let Ok(s) = std::str::from_utf8(&decoded)
            && s.contains("\"alg\"")
        {
            return true;
        }
    }
    false
}

fn base64url_value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// Unpadded base64url decode, as used by JWT segments. Returns `None` on any
/// invalid character or a leftover single symbol (which cannot represent a
/// whole byte) rather than panicking — this runs on arbitrary clipboard text.
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4 + 3);
    let mut chunk = [0u8; 4];
    let mut chunk_len = 0usize;
    for &b in s.as_bytes() {
        chunk[chunk_len] = base64url_value(b)?;
        chunk_len += 1;
        if chunk_len == 4 {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
            out.push((chunk[2] << 6) | chunk[3]);
            chunk_len = 0;
        }
    }
    match chunk_len {
        0 => {}
        1 => return None,
        2 => out.push((chunk[0] << 2) | (chunk[1] >> 4)),
        3 => {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
        }
        _ => unreachable!("chunk_len is masked to 0..=3 by the reset above"),
    }
    Some(out)
}

/// Finds maximal runs of digits/spaces/dashes (the only punctuation real
/// card-number formatting uses) and Luhn-checks the digits in each run. A
/// letter anywhere breaks the run, which is what keeps this from ever
/// looking inside hex strings like UUIDs or git SHAs.
fn contains_luhn_valid_card_number(text: &str) -> bool {
    let mut run = String::new();
    let flush_and_check = |run: &mut String| -> bool {
        let digits: String = run.chars().filter(char::is_ascii_digit).collect();
        run.clear();
        (13..=19).contains(&digits.len()) && has_card_issuer_prefix(&digits) && luhn_valid(&digits)
    };

    for c in text.chars() {
        if c.is_ascii_digit() || c == ' ' || c == '-' {
            run.push(c);
        } else if !run.is_empty() && flush_and_check(&mut run) {
            return true;
        }
    }
    if !run.is_empty() && flush_and_check(&mut run) {
        return true;
    }
    false
}

/// Does this run start like a real card number?
///
/// Luhn alone is a weak filter: roughly one in ten arbitrary digit strings of
/// the right length passes it by chance, and runs here can span separators, so
/// a pair of dates or a couple of order numbers side by side would eventually
/// suppress somebody's clipboard entry for no reason. Requiring an issuer
/// prefix (ISO/IEC 7812 IIN ranges) costs nothing for real cards and removes
/// almost all of that noise.
fn has_card_issuer_prefix(digits: &str) -> bool {
    let two: u32 = digits[..2].parse().unwrap_or(0);
    let four: u32 = digits.get(..4).and_then(|s| s.parse().ok()).unwrap_or(0);

    digits.starts_with('4')                       // Visa
        || (51..=55).contains(&two)               // Mastercard
        || (2221..=2720).contains(&four)          // Mastercard, 2-series
        || two == 34 || two == 37                 // American Express
        || two == 65 || four == 6011              // Discover
        || (644..=649).contains(&(four / 10))     // Discover
        || two == 62                              // UnionPay
        || two == 35                              // JCB
        || two == 30 || two == 36 || two == 38 // Diners Club
}

/// Standard Luhn checksum (ISO/IEC 7812): double every second digit counting
/// from the rightmost, subtract 9 from anything over 9, sum, valid iff the
/// total is a multiple of 10. This is what keeps ordinary long digit strings
/// (order numbers, phone numbers that slip past the length filter, ids) from
/// being misclassified as card numbers — only about 1 in 10 arbitrary digit
/// strings of the right length pass by chance.
fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0u32;
    for (i, ch) in digits.chars().rev().enumerate() {
        let mut d = ch.to_digit(10).expect("caller filtered to ascii digits");
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum.is_multiple_of(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- true positives: each documented shape must be caught -------------

    #[test]
    fn detects_jwt() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
                   eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0.\
                   dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let text = format!("here is a token: {jwt}");
        assert_eq!(detect_secret(&text), Some(SecretKind::Jwt));
    }

    #[test]
    fn detects_aws_access_key() {
        assert_eq!(
            detect_secret("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"),
            Some(SecretKind::AwsAccessKey)
        );
    }

    #[test]
    fn detects_github_tokens() {
        assert_eq!(
            detect_secret("token: ghp_16C7e42F292c6912E7710c838347Ae178B4a"),
            Some(SecretKind::GitHubToken)
        );
        assert_eq!(
            detect_secret(
                "github_pat_11AAAAAAA0aaaaaaaaaaaa_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            Some(SecretKind::GitHubToken)
        );
    }

    #[test]
    fn detects_slack_token() {
        assert_eq!(
            detect_secret(&["xoxb", "-123456789012-1234567890123-abcdefghijklmnopqrstuvwx"].concat()),
            Some(SecretKind::SlackToken)
        );
    }

    #[test]
    fn detects_stripe_key() {
        assert_eq!(
            detect_secret(&["sk", "_live_4eC39HqLyjWDarjtT1zdp7dc"].concat()),
            Some(SecretKind::StripeKey)
        );
    }

    #[test]
    fn detects_openai_key() {
        assert_eq!(
            detect_secret(&["sk", "-abcdefghijklmnopqrstuvwxyz0123456789ABCD"].concat()),
            Some(SecretKind::OpenAiKey)
        );
    }

    #[test]
    fn detects_google_api_key() {
        assert_eq!(
            detect_secret(&["AIza", "SyD-9tSrke72PouQMnMX-a7eZSW0jkFMBWY"].concat()),
            Some(SecretKind::GoogleApiKey)
        );
    }

    #[test]
    fn detects_pem_private_key_header() {
        assert_eq!(
            detect_secret(
                "-----BEGIN RSA PRIVATE KEY-----\nMIIEow...\n-----END RSA PRIVATE KEY-----"
            ),
            Some(SecretKind::PrivateKeyPem)
        );
        assert_eq!(
            detect_secret("-----BEGIN PRIVATE KEY-----\nMIIEvQ...\n-----END PRIVATE KEY-----"),
            Some(SecretKind::PrivateKeyPem)
        );
    }

    #[test]
    fn detects_credit_card_with_and_without_separators() {
        assert_eq!(
            detect_secret("4111111111111111"),
            Some(SecretKind::CreditCard)
        );
        assert_eq!(
            detect_secret("card: 4111-1111-1111-1111"),
            Some(SecretKind::CreditCard)
        );
        assert_eq!(
            detect_secret("card: 4111 1111 1111 1111"),
            Some(SecretKind::CreditCard)
        );
        assert_eq!(
            detect_secret("5500005555555559"),
            Some(SecretKind::CreditCard)
        );
        assert_eq!(
            detect_secret("340000000000009"),
            Some(SecretKind::CreditCard)
        ); // amex, 15 digits
    }

    // --- false positives: ordinary content must pass through clean --------

    #[test]
    fn ordinary_prose_is_not_flagged() {
        let prose = "Hey, can you grab milk on the way home? Also the meeting \
                      moved to 3pm tomorrow, see the calendar invite.";
        assert_eq!(detect_secret(prose), None);
    }

    #[test]
    fn code_snippet_is_not_flagged() {
        let code = r#"
            fn main() {
                let config = app.config.database.pool_size;
                println!("{}", config);
            }
        "#;
        assert_eq!(detect_secret(code), None);
    }

    #[test]
    fn dotted_identifiers_are_not_flagged_as_jwts() {
        // Same dot-separated shape as a JWT but does not decode to a JWT
        // header, so this must not trip the detector.
        assert_eq!(detect_secret("com.example.myapp.MainActivity"), None);
        assert_eq!(detect_secret("version.build.revision.patchlevel"), None);
    }

    #[test]
    fn uuid_is_not_flagged() {
        assert_eq!(detect_secret("550e8400-e29b-41d4-a716-446655440000"), None);
    }

    #[test]
    fn git_sha_is_not_flagged() {
        assert_eq!(
            detect_secret("a94a8fe5ccb19ba61c4c0873d391e987982fbbd3"),
            None
        );
        assert_eq!(
            detect_secret("commit a94a8fe5ccb19ba61c4c0873d391e987982fbbd3 fixed the bug"),
            None
        );
    }

    #[test]
    fn phone_number_is_not_flagged() {
        assert_eq!(detect_secret("call me at 555-123-4567"), None);
        assert_eq!(detect_secret("+1 415 555 2671"), None);
    }

    #[test]
    fn isbn_is_not_flagged() {
        assert_eq!(detect_secret("ISBN 978-3-16-148410-0"), None); // ISBN-13
        assert_eq!(detect_secret("ISBN 0-306-40615-2"), None); // ISBN-10
    }

    #[test]
    fn ordinary_long_digit_strings_are_not_flagged() {
        // Neither passes the Luhn check.
        assert_eq!(detect_secret("order number 12345678901234"), None);
        assert_eq!(detect_secret("tracking id 98765432109876"), None);
    }

    #[test]
    fn luhn_valid_digits_without_an_issuer_prefix_are_not_flagged() {
        // Passes Luhn but starts with 1, which no card issuer uses. Without
        // the issuer check this would suppress a perfectly ordinary clipboard
        // entry roughly one time in ten.
        assert!(
            luhn_valid("1234567890128"),
            "test premise: this passes Luhn"
        );
        assert_eq!(detect_secret("ref 1234567890128"), None);
    }

    #[test]
    fn adjacent_dates_do_not_look_like_a_card() {
        // The digit run spans the space, giving 16 digits — exactly the shape
        // the Luhn-only check was most likely to trip over.
        assert_eq!(
            detect_secret("window 2024-01-15 2026-07-27 inclusive"),
            None
        );
    }

    #[test]
    fn issuer_prefixes_still_accept_real_cards() {
        for (label, number) in [
            ("visa", "4111111111111111"),
            ("mastercard", "5500005555555559"),
            ("mastercard 2-series", "2223000048400011"),
            ("amex", "340000000000009"),
            ("discover", "6011111111111117"),
            ("jcb", "3530111333300000"),
            ("diners", "30569309025904"),
        ] {
            assert_eq!(
                detect_secret(number),
                Some(SecretKind::CreditCard),
                "{label} was not detected"
            );
        }
    }

    #[test]
    fn jwt_shaped_text_without_a_real_header_is_not_flagged() {
        // Matches the JWT_RE shape (three dot-separated base64url runs of
        // 10+ chars) but the first segment does not decode to a JSON object
        // containing "alg" — this is what the header/`"alg"` check guards
        // against, distinct from the earlier tests where the segments were
        // too short to match the regex shape at all.
        assert_eq!(detect_secret("AAAAAAAAAA.BBBBBBBBBB.CCCCCCCCCC"), None);
    }

    #[test]
    fn short_dashed_prefix_lookalikes_are_not_flagged() {
        assert_eq!(
            detect_secret("aki-and-friends went to the AKIA offices"),
            None
        );
        assert_eq!(detect_secret("sk-8"), None); // far too short to be a real key
    }
}
