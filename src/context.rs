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

/// Where speaking stops: the context block, and tool calls some models write as text.
const STOPS: [&str; 3] = [OPEN, "<tool_call>", "<function="];

/// Streams a reply up to the context block or a tool call written as text. Text that could
/// still turn into one of them is held back until the next piece shows what it is.
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
        let found = STOPS
            .iter()
            .filter_map(|stop| lower[self.passed..].find(stop))
            .min();
        if let Some(at) = found {
            self.cut = true;
            return self.pass(self.passed + at);
        }
        // The stops are ASCII, so a suffix that matches the start of one ends on a char boundary.
        let held = STOPS
            .iter()
            .filter_map(|stop| (1..stop.len()).rev().find(|n| lower.ends_with(&stop[..*n])))
            .max()
            .unwrap_or(0);
        self.pass(self.seen.len() - held)
    }

    /// Whether the reply stopped at a block or a tool call written as text.
    pub fn was_cut(&self) -> bool {
        self.cut
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
    fn a_tool_call_written_as_text_is_never_spoken() {
        let mut cut = ReplyCut::default();
        let mut spoken = cut.push("Moment. <tool");
        spoken.push_str(&cut.push("_call>\n<function=web_research>"));
        spoken.push_str(&cut.finish());
        assert_eq!(spoken, "Moment. ");
        assert!(cut.was_cut());
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
