//! What the assistant says on its own, without the model, in the language of the chatbot's voice.

pub struct Phrases {
    pub not_understood: &'static str,
    pub no_answer: &'static str,
    pub too_long: &'static str,
    pub at_limit: &'static str,
    pub unreachable: &'static str,
    pub no_tool: &'static str,
    pub not_done: &'static str,
    pub stt_unavailable: &'static str,
    confirm_one: &'static str,
    confirm_many: &'static str,
}

impl Phrases {
    /// Asks before tools that change something. `{}` takes the action or the list of actions.
    pub fn confirm(&self, labels: &[String]) -> String {
        if let [one] = labels {
            self.confirm_one.replace("{}", one)
        } else {
            self.confirm_many.replace("{}", &labels.join(", "))
        }
    }
}

const DE: Phrases = Phrases {
    not_understood: "Das habe ich leider nicht verstanden.",
    no_answer: "Ich kann gerade nicht nachdenken, meine Verbindung ist gestört. \
Versuch es gleich noch einmal.",
    too_long: "Das dauert mir zu lange. Versuch es später noch einmal.",
    at_limit: "Alle KI-Modelle sind gerade am Limit. Versuch es gleich nochmal.",
    unreachable: "Die KI-Modelle sind gerade nicht erreichbar. Versuch es gleich nochmal.",
    no_tool: "Dafür habe ich kein passendes Werkzeug gefunden.",
    not_done: "Das habe ich so nicht ausführen können.",
    stt_unavailable: "Die Spracherkennung ist gerade nicht bereit. Versuch es gleich noch einmal.",
    confirm_one: "Soll ich {} ausführen? Sag ja oder nein.",
    confirm_many: "Soll ich diese Aktionen ausführen: {}? Sag ja oder nein.",
};

const EN: Phrases = Phrases {
    not_understood: "Sorry, I didn't catch that.",
    no_answer: "I can't think right now, my connection is down. Please try again in a moment.",
    too_long: "This is taking me too long. Please try again later.",
    at_limit: "All AI models are at their limit right now. Please try again in a moment.",
    unreachable: "The AI models can't be reached right now. Please try again in a moment.",
    no_tool: "I couldn't find a suitable tool for that.",
    not_done: "I couldn't carry that out.",
    stt_unavailable: "Speech recognition isn't ready right now. Please try again in a moment.",
    confirm_one: "Shall I run {}? Say yes or no.",
    confirm_many: "Shall I run these actions: {}? Say yes or no.",
};

/// "vits-piper-en_US-amy-medium" speaks English. Every other voice gets German, the language of
/// the recognizer.
pub fn of(voice: &str) -> &'static Phrases {
    let name = voice.strip_prefix("vits-piper-").unwrap_or(voice);
    if name.starts_with("en_") || name.starts_with("en-") {
        &EN
    } else {
        &DE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_voice_picks_the_language() {
        assert_eq!(of("vits-piper-en_US-amy-medium").too_long, EN.too_long);
        assert_eq!(of("vits-piper-de_DE-thorsten-medium").too_long, DE.too_long);
        assert_eq!(of("").too_long, DE.too_long);
    }

    #[test]
    fn confirm_names_the_actions() {
        assert_eq!(
            DE.confirm(&["Licht an".into()]),
            "Soll ich Licht an ausführen? Sag ja oder nein."
        );
        assert!(EN.confirm(&["a".into(), "b".into()]).contains("a, b"));
    }
}
