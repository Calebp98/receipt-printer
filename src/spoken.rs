//! The rule for turning a spoken note into a print job.
//!
//! An Index 01 posts every recording to the webhook, so most of what arrives is
//! an ordinary note that must be left alone. Only a transcription that *starts*
//! with the word "print" is a request to print, and only the rest of it goes on
//! the paper.

/// The word that asks for a print, at the start of the message and nowhere else.
pub const TRIGGER: &str = "print";

/// The part to print, or None when this was just a note.
///
/// The word has to stand alone: "printer jammed" and "printing the report" are
/// somebody talking, not an instruction. Speech-to-text likes to add a comma or
/// a colon after an opening word, so any punctuation between the trigger and
/// the message is dropped too.
pub fn requested(transcription: &str) -> Option<String> {
    let text = transcription.trim_start();
    let head: String = text.chars().take(TRIGGER.len()).collect();
    if !head.eq_ignore_ascii_case(TRIGGER) {
        return None;
    }

    let rest = &text[head.len()..];
    // "printer", "printing" — the trigger has to be a word of its own.
    if rest.chars().next().is_some_and(char::is_alphanumeric) {
        return None;
    }

    let body = rest
        .trim_start_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .trim();
    (!body.is_empty()).then(|| body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_that_starts_with_print_is_a_request() {
        assert_eq!(requested("print buy milk").as_deref(), Some("buy milk"));
        assert_eq!(requested("Print buy milk").as_deref(), Some("buy milk"));
        assert_eq!(requested("PRINT buy milk").as_deref(), Some("buy milk"));
        assert_eq!(requested("  print buy milk  ").as_deref(), Some("buy milk"));
    }

    #[test]
    fn speech_to_text_punctuation_after_the_trigger_is_dropped() {
        assert_eq!(requested("Print: buy milk").as_deref(), Some("buy milk"));
        assert_eq!(requested("Print, buy milk").as_deref(), Some("buy milk"));
        assert_eq!(requested("print - buy milk").as_deref(), Some("buy milk"));
    }

    #[test]
    fn the_trigger_has_to_be_a_word_of_its_own() {
        assert_eq!(requested("printer jammed again"), None);
        assert_eq!(requested("printing the report tomorrow"), None);
        assert_eq!(requested("prints are expensive"), None);
    }

    #[test]
    fn an_ordinary_note_is_left_alone() {
        assert_eq!(requested("remember to call the dentist"), None);
        assert_eq!(requested("ask Kate about the print run"), None);
        assert_eq!(requested(""), None);
    }

    #[test]
    fn the_trigger_on_its_own_prints_nothing() {
        assert_eq!(requested("print"), None);
        assert_eq!(requested("print   "), None);
        assert_eq!(requested("print."), None);
    }
}
