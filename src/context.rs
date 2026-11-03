//! The chatbot's only memory: a short context the model rewrites itself. It stands in front of
//! the question, and a new one comes back as a block after the reply, which is never spoken.

use crate::llm::ChatMessage;

/// What the model is told the context holds; a longer block is cut.
pub const CONTEXT_CHARS: usize = 500;

const OPEN: &str = "[kontext]";
const CLOSE: &str = "[/kontext]";

/// The reply without the block, and the new context when the model wrote one.
pub fn split(text: &str) -> (String, Option<String>) {
    let lower = text.to_ascii_lowercase();
    let Some(start) = lower.find(OPEN) else {
        return (text.trim().to_string(), None);
    };
    let body_start = start + OPEN.len();
    let body_end = lower[body_start..]
        .find(CLOSE)
        .map(|at| body_start + at)
        .unwrap_or(text.len());
    let context: String = text[body_start..body_end]
        .trim()
        .chars()
        .take(CONTEXT_CHARS)
        .collect();
    let reply = text[..start].trim().to_string();
    (reply, (!context.is_empty()).then_some(context))
}

/// Puts the context in front of the last question. It stays visible while tool results
/// replace the rest of the prompt, and it never becomes part of the system prompt.
pub fn attach(messages: &mut [ChatMessage], context: &str) {
    let context = context.trim();
    if context.is_empty() {
        return;
    }
    if let Some(question) = messages.iter_mut().rev().find(|m| m.role == "user") {
        question.content = format!("{OPEN}\n{context}\n{CLOSE}\n\n{}", question.content);
    }
}

/// Streams a reply up to the context block. Text that could still turn into the opening tag
/// is held back until the next piece shows what it is.
#[derive(Default)]
pub struct ReplyCut {
    seen: String,
    passed: usize,
    cut: bool,
}

impl ReplyCut {
    /// The part of `piece` that may be spoken now.
    pub fn push(&mut self, piece: &str) -> String {
        self.seen.push_str(piece);
        if self.cut {
            return String::new();
        }
        let lower = self.seen.to_ascii_lowercase();
        if let Some(at) = lower[self.passed..].find(OPEN).map(|at| self.passed + at) {
            self.cut = true;
            return self.pass(at);
        }
        // The tag is ASCII, so a suffix that matches its start ends on a char boundary.
        let held = (1..OPEN.len())
            .rev()
            .find(|n| lower.ends_with(&OPEN[..*n]))
            .unwrap_or(0);
        self.pass(self.seen.len() - held)
    }

    /// Whatever was held back, unless it was the block.
    pub fn finish(&mut self) -> String {
        if self.cut {
            return String::new();
        }
        self.pass(self.seen.len())
    }

    /// Everything the model sent, block included.
    pub fn full(&self) -> &str {
        &self.seen
    }

    fn pass(&mut self, until: usize) -> String {
        let out = self.seen[self.passed..until].to_string();
        self.passed = until;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_leaves_the_reply_and_becomes_the_context() {
        let (reply, context) =
            split("Der Turm ist 330 Meter hoch.\n[Kontext]\nNutzer heißt Veit.\n[/Kontext]");
        assert_eq!(reply, "Der Turm ist 330 Meter hoch.");
        assert_eq!(context.as_deref(), Some("Nutzer heißt Veit."));
    }

    #[test]
    fn without_a_block_the_old_context_stays() {
        assert_eq!(split(" Hallo. "), ("Hallo.".into(), None));
        assert_eq!(
            split("Hallo. [kontext] [/kontext]"),
            ("Hallo.".into(), None)
        );
    }

    #[test]
    fn an_unclosed_or_long_block_is_cut_to_the_limit() {
        let (_, context) = split(&format!("Ja. [kontext] {}", "ä".repeat(900)));
        assert_eq!(context.unwrap().chars().count(), CONTEXT_CHARS);
    }

    #[test]
    fn the_context_goes_in_front_of_the_last_question_only() {
        let mut messages = vec![
            ChatMessage::new("system", "Regeln"),
            ChatMessage::new("user", "alt"),
            ChatMessage::new("assistant", "alte Antwort"),
            ChatMessage::new("user", "neu"),
        ];
        attach(&mut messages, "Nutzer heißt Veit.");
        assert_eq!(messages[1].content, "alt");
        assert_eq!(
            messages[3].content,
            "[kontext]\nNutzer heißt Veit.\n[/kontext]\n\nneu"
        );
        attach(&mut messages[..2], " ");
        assert_eq!(messages[1].content, "alt");
    }

    #[test]
    fn streaming_stops_at_the_block_even_when_the_tag_is_split() {
        let mut cut = ReplyCut::default();
        let mut spoken = String::new();
        for piece in [
            "Gut. Mehr ",
            "weiß ich nicht. [ko",
            "ntext]Nutzer",
            " mag Tee[/kontext]",
        ] {
            spoken.push_str(&cut.push(piece));
        }
        spoken.push_str(&cut.finish());
        assert_eq!(spoken, "Gut. Mehr weiß ich nicht. ");
        assert!(cut.full().ends_with("[/kontext]"));
    }

    #[test]
    fn a_bracket_that_is_no_tag_is_spoken_after_all() {
        let mut cut = ReplyCut::default();
        let mut spoken = cut.push("Siehe [k");
        spoken.push_str(&cut.push("lein]."));
        spoken.push_str(&cut.finish());
        assert_eq!(spoken, "Siehe [klein].");
    }
}
