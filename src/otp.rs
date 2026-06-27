//! Verification-code extraction (DESIGN.md §4).
//!
//! Default: digit runs of 4–8 near keywords (`code`, `otp`, `verify`, `pin`,
//! …), falling back to the first standalone run. Codes printed with internal
//! separators (`481 920`, `481-920`) are normalized before length checks. A
//! `--pattern` with one capture group overrides everything.

use regex::Regex;

/// A successful extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// The extracted code (separators stripped).
    pub code: String,
    /// How it was found: `pattern`, `keyword-proximity`, or `default-digits`.
    pub matched_by: &'static str,
}

const KEYWORDS: &[&str] = &[
    "code",
    "otp",
    "verify",
    "verification",
    "pin",
    "passcode",
    "one-time",
    "one time",
];

/// How close (in bytes) a keyword must precede a candidate to count as
/// "near" it.
const PROXIMITY_WINDOW: usize = 48;

/// Extract a verification code from `text`.
///
/// `pattern` (one capture group) overrides the default heuristic; `len` pins the
/// expected digit count (otherwise 4–8 are tried).
pub fn extract_code(text: &str, pattern: Option<&str>, len: Option<usize>) -> Option<Extracted> {
    if let Some(pat) = pattern {
        let re = Regex::new(pat).ok()?;
        let caps = re.captures(text)?;
        // Prefer the first capture group; fall back to the whole match.
        let code = caps
            .get(1)
            .or_else(|| caps.get(0))
            .map(|m| m.as_str().to_string())?;
        return Some(Extracted {
            code,
            matched_by: "pattern",
        });
    }

    let (min, max) = match len {
        Some(l) => (l, l),
        None => (4, 8),
    };

    let candidates = digit_candidates(text, min, max);
    if candidates.is_empty() {
        return None;
    }

    let lower = text.to_lowercase();
    // Prefer a candidate immediately preceded by a keyword.
    if let Some(c) = candidates
        .iter()
        .find(|c| keyword_precedes(&lower, c.byte_pos))
    {
        return Some(Extracted {
            code: c.digits.clone(),
            matched_by: "keyword-proximity",
        });
    }

    // Fall back to the first standalone run.
    Some(Extracted {
        code: candidates[0].digits.clone(),
        matched_by: "default-digits",
    })
}

struct Candidate {
    byte_pos: usize,
    digits: String,
}

/// Find digit runs (allowing internal single spaces/hyphens) whose digit count,
/// after stripping separators, falls in `[min, max]`, in text order.
fn digit_candidates(text: &str, min: usize, max: usize) -> Vec<Candidate> {
    // A digit, then any mix of digits and single separators, then a digit;
    // or a lone digit run. Word boundaries keep us off larger tokens.
    let re = Regex::new(r"\b\d[\d \-]*\d\b|\b\d+\b").expect("static regex");
    re.find_iter(text)
        .filter_map(|m| {
            let digits: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
            if (min..=max).contains(&digits.len()) {
                Some(Candidate {
                    byte_pos: m.start(),
                    digits,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Whether any keyword appears within the proximity window before `pos`.
fn keyword_precedes(lower: &str, pos: usize) -> bool {
    let start = pos.saturating_sub(PROXIMITY_WINDOW);
    let window = &lower[start..pos];
    KEYWORDS.iter().any(|kw| window.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_keyword_proximal_code() {
        let r = extract_code("Your verification code is 481920. Ignore 2026.", None, None).unwrap();
        assert_eq!(r.code, "481920");
        assert_eq!(r.matched_by, "keyword-proximity");
    }

    #[test]
    fn prefers_keyword_over_other_numbers() {
        // A year appears first; the code is keyword-proximal and should win.
        let text = "Copyright 2026. Enter code 123456 to continue.";
        let r = extract_code(text, None, None).unwrap();
        assert_eq!(r.code, "123456");
    }

    #[test]
    fn normalizes_separated_digits() {
        let r = extract_code("Your code: 481 920", None, None).unwrap();
        assert_eq!(r.code, "481920");
        let r2 = extract_code("PIN 481-920 now", None, None).unwrap();
        assert_eq!(r2.code, "481920");
    }

    #[test]
    fn falls_back_to_first_standalone_run() {
        let r = extract_code("No keyword here, just 5567 alone.", None, None).unwrap();
        assert_eq!(r.code, "5567");
        assert_eq!(r.matched_by, "default-digits");
    }

    #[test]
    fn respects_explicit_len() {
        // 4-digit run ignored when len=6 is required.
        let r = extract_code("first 1234 then 567890", None, Some(6)).unwrap();
        assert_eq!(r.code, "567890");
    }

    #[test]
    fn pattern_override_uses_capture_group() {
        let r = extract_code("token=ABC-7", Some(r"token=([A-Z]+-\d)"), None).unwrap();
        assert_eq!(r.code, "ABC-7");
        assert_eq!(r.matched_by, "pattern");
    }

    #[test]
    fn no_code_returns_none() {
        assert!(extract_code("nothing numeric to see here", None, None).is_none());
        // Too short / too long for the default 4-8 window.
        assert!(extract_code("12 and 1234567890", None, None).is_none());
    }
}
