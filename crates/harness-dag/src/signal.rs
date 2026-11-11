//! Loop completion-signal detection.
//!
//! A [`crate::model::LoopConfig`] converges when the agent emits its `until`
//! signal. A signal counts as emitted when it appears in any of three forms
//! (checked in order):
//!
//! 1. **Tag-wrapped:** `<promise>SIGNAL</promise>` — any matching open/close
//!    tag pair, case-insensitive (the recommended, unambiguous form).
//! 2. **End of output:** `SIGNAL` at the very end, allowing trailing
//!    punctuation/whitespace — unless the sentence it ends negates it
//!    ("I'm not outputting SIGNAL.").
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

    // 2. End of output (optionally followed by trailing punctuation/space) —
    //    unless the sentence it ends says the opposite. "…so I'm not
    //    outputting SCORING_DONE." ends with the signal too, and taking it at
    //    its word ended a loop with most of its work still to do.
    let at_end = Regex::new(&format!(r"{esc}[\s.,;:!?]*$")).expect("valid end regex");
    if let Some(m) = at_end.find(output) {
        if !negated(last_sentence(&output[..m.start()])) {
            return true;
        }
    }

    // 3. On its own line.
    let own_line = Regex::new(&format!(r"(?m)^\s*{esc}\s*$")).expect("valid line regex");
    own_line.is_match(output)
}

/// The sentence `text` ends in: what follows its last sentence break.
fn last_sentence(text: &str) -> &str {
    let start = text.rfind(['.', '!', '?', '\n']).map_or(0, |i| i + 1);
    &text[start..]
}

/// Whether a sentence denies what it ends with ("not", "won't", "no"…). A
/// signal is a claim of being done; a sentence that negates it is the agent
/// saying it isn't.
fn negated(sentence: &str) -> bool {
    let negation = Regex::new(
        r"(?i)\b(not|no|never|without|cannot|can't|won't|don't|doesn't|didn't|isn't|aren't|haven't|hasn't|shouldn't|yet to|instead of)\b",
    )
    .expect("valid negation regex");
    negation.is_match(sentence)
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
    fn a_sentence_that_denies_the_signal_does_not_end_the_loop() {
        // What the test audit's scoring loop wrote, 6,737 tests from done.
        let refusal = "`remaining.mjs` reports 6737 tests still unscored, so I'm not \
                       outputting SCORING_DONE.";
        assert!(!detect_signal(refusal, "SCORING_DONE"));
        assert!(!detect_signal("I won't emit DONE", "DONE"));
        assert!(!detect_signal(
            "Two tests still fail, so no FINAL_VERIFY_CLEAN.",
            "FINAL_VERIFY_CLEAN"
        ));
        // The negation must be in the same sentence: an earlier one doesn't
        // take back a later claim.
        assert!(detect_signal(
            "The first pass did not finish. Everything is scored now: SCORING_DONE",
            "SCORING_DONE"
        ));
        // The tag and own-line forms are explicit, whatever the prose says.
        assert!(detect_signal(
            "Not much left to say.\n<promise>SCORING_DONE</promise>",
            "SCORING_DONE"
        ));
        assert!(detect_signal("no issues found\nDONE", "DONE"));
    }

    #[test]
    fn validate_rejects_empty() {
        assert!(validate_signal("   ").is_err());
        assert_eq!(validate_signal("OK").unwrap(), "OK");
    }
}
