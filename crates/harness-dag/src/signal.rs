//! Loop completion-signal detection.
//!
//! A [`crate::model::LoopConfig`] converges when the agent emits its `until`
//! signal. Following Archon's semantics, a signal counts as emitted when it
//! appears in any of three forms (checked in order):
//!
//! 1. **Tag-wrapped:** `<promise>SIGNAL</promise>` — any matching open/close
//!    tag pair, case-insensitive (the recommended, unambiguous form).
//! 2. **End of output:** `SIGNAL` at the very end, allowing trailing
//!    punctuation/whitespace.
//! 3. **Own line:** a line consisting solely of `SIGNAL`.
//!
//! The end/own-line forms are deliberately restrictive so prose like
//! "not COMPLETE yet" does not falsely trigger.

use regex::Regex;

use crate::error::DagError;

/// Returns `true` if `output` contains the completion `signal` in any
/// recognized form.
///
/// `signal` is treated as a literal string (regex-escaped internally).
pub fn detect_signal(output: &str, signal: &str) -> bool {
    let esc = regex::escape(signal);

    // 1. Tag-wrapped. The `regex` crate has no backreferences, so we capture
    //    both tag names and compare them in code.
    let tagged = Regex::new(&format!(
        r"(?is)<([a-zA-Z][\w-]*)[^>]*>\s*{esc}\s*</([a-zA-Z][\w-]*)>"
    ))
    .expect("valid tag regex");
    if let Some(caps) = tagged.captures(output) {
        if caps[1].eq_ignore_ascii_case(&caps[2]) {
            return true;
        }
    }

    // 2. End of output (optionally followed by trailing punctuation/space).
    let at_end = Regex::new(&format!(r"{esc}[\s.,;:!?]*$")).expect("valid end regex");
    if at_end.is_match(output) {
        return true;
    }

    // 3. On its own line.
    let own_line = Regex::new(&format!(r"(?m)^\s*{esc}\s*$")).expect("valid line regex");
    own_line.is_match(output)
}

/// Validate that a signal string is usable (non-empty after trimming). Returns
/// the signal unchanged on success.
pub fn validate_signal(signal: &str) -> Result<&str, DagError> {
    if signal.trim().is_empty() {
        return Err(DagError::EmptySignal);
    }
    Ok(signal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_tag_wrapped() {
        assert!(detect_signal(
            "blah\n<promise>REVIEW_CLEAN</promise>\n",
            "REVIEW_CLEAN"
        ));
        // Case-insensitive tag, with attributes.
        assert!(detect_signal("<Promise foo=\"1\">DONE</Promise>", "DONE"));
    }

    #[test]
    fn rejects_mismatched_tags() {
        assert!(!detect_signal("<open>DONE</close>", "DONE"));
    }

    #[test]
    fn detects_at_end() {
        assert!(detect_signal(
            "all checks pass, FINAL_VERIFY_CLEAN",
            "FINAL_VERIFY_CLEAN"
        ));
        assert!(detect_signal("work is now COMPLETE.", "COMPLETE"));
    }

    #[test]
    fn detects_own_line() {
        assert!(detect_signal("summary\nDONE\nmore notes", "DONE"));
    }

    #[test]
    fn does_not_falsely_trigger_in_prose() {
        assert!(!detect_signal(
            "this is not COMPLETE yet, keep going",
            "COMPLETE"
        ));
        assert!(!detect_signal("the COMPLETED list has items", "COMPLETE"));
    }

    #[test]
    fn validate_rejects_empty() {
        assert!(validate_signal("   ").is_err());
        assert_eq!(validate_signal("OK").unwrap(), "OK");
    }
}
