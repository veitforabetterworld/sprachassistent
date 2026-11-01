//! Personality, prompt assembly and fact extraction around the language model.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::Datelike;
use futures_util::{Stream, StreamExt};

use crate::llm::{ChatMessage, ChatModel};
use crate::memory::{FactChanges, MemoryStore};
use crate::sentences::SentenceSplitter;

const DEFAULT_PERSONA: &str = include_str!("../persona.md");

/// Rules for speech output, appended to every personality.
const SPEECH_RULES: &str = "Du sprichst mit dem Nutzer über einen kleinen Lautsprecher. \
Antworte kurz, meist in ein bis drei Sätzen, in natürlicher gesprochener Sprache. \
Verwende kein Markdown, keine Listen, keine Emojis und keine Sonderzeichen. \
Schreibe Abkürzungen aus.";

const WEEKDAYS: [&str; 7] = [
    "Montag",
    "Dienstag",
    "Mittwoch",
    "Donnerstag",
    "Freitag",
    "Samstag",
    "Sonntag",
];

pub const NOT_UNDERSTOOD: &str = "Das habe ich leider nicht verstanden.";
const NO_ANSWER: &str = "Ich kann gerade nicht nachdenken, meine Verbindung ist gestört. \
Versuch es gleich noch einmal.";

const EXTRACTION_PROMPT: &str = "Du pflegst das Langzeitgedächtnis eines Sprachassistenten. \
Du bekommst die bisher gespeicherten Fakten über den Nutzer (mit id) und das letzte Gespräch. \
Ergänze dauerhaft relevante Fakten (Name, Vorlieben, Familie, Haustiere, Termine, Wünsche wie \
'merk dir ...'), korrigiere veraltete Fakten und lösche widerlegte. Ignoriere Smalltalk und \
Fragen ohne neue Information über den Nutzer. Formuliere Fakten kurz in der dritten Person. \
Antworte nur mit JSON: {\"add\": [\"...\"], \"update\": [{\"id\": 1, \"content\": \"...\"}], \
\"delete\": [2]}. Wenn sich nichts ändert: {\"add\": [], \"update\": [], \"delete\": []}.";

pub struct PersonaStore {
    path: PathBuf,
}

impl PersonaStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn get(&self) -> String {
        std::fs::read_to_string(&self.path)
            .ok()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_PERSONA.to_string())
    }

    pub fn set(&self, persona: &str) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, persona.trim())?;
        Ok(())
    }

    pub fn reset(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

pub struct Assistant {
    pub llm: Arc<dyn ChatModel>,
    pub memory: Arc<MemoryStore>,
    pub persona: PersonaStore,
    pub history_messages: usize,
    pub memory_extraction: bool,
}

impl Assistant {
    pub fn build_messages(&self, user_text: &str) -> Result<Vec<ChatMessage>> {
        let mut system = format!("{}\n\n{SPEECH_RULES}", self.persona.get().trim());
        let now = chrono::Local::now();
        system.push_str(&format!(
            "\n\nJetzt ist {}, der {} um {} Uhr.",
            WEEKDAYS[now.weekday().num_days_from_monday() as usize],
            now.format("%d.%m.%Y"),
            now.format("%H:%M")
        ));
        let facts = self.memory.facts()?;
        if !facts.is_empty() {
            system.push_str("\n\nDas weißt du über den Nutzer:");
            for f in &facts {
                system.push_str(&format!("\n- {}", f.content));
            }
        }
        let mut messages = vec![ChatMessage::new("system", system)];
        for m in self.memory.recent_messages(self.history_messages)? {
            messages.push(ChatMessage::new(&m.role, m.content));
        }
        messages.push(ChatMessage::new("user", user_text));
        Ok(messages)
    }

    /// Streams the reply sentence by sentence. Once the reply is complete it is stored and facts
    /// are extracted in the background. A failure yields a spoken apology instead.
    pub fn reply_stream(self: Arc<Self>, user_text: String) -> impl Stream<Item = String> + Send {
        async_stream::stream! {
            let mut full = String::new();
            let mut splitter = SentenceSplitter::default();
            let tokens = match self.build_messages(&user_text) {
                Ok(messages) => self.llm.stream(&messages).await,
                Err(e) => Err(e),
            };
            match tokens {
                Ok(mut tokens) => {
                    while let Some(token) = tokens.next().await {
                        match token {
                            Ok(t) => {
                                full.push_str(&t);
                                for sentence in splitter.push(&t) {
                                    yield sentence;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("LLM-Stream abgebrochen: {e:#}");
                                break;
                            }
                        }
                    }
                    if let Some(rest) = splitter.finish() {
                        yield rest;
                    }
                }
                Err(e) => tracing::error!("{e:#}"),
            }
            if full.trim().is_empty() {
                yield NO_ANSWER.to_string();
            } else {
                tracing::info!("Antwort: {}", full.trim());
                self.remember(user_text, full.trim().to_string());
            }
        }
    }

    pub async fn reply(self: Arc<Self>, user_text: String) -> String {
        self.reply_stream(user_text)
            .collect::<Vec<_>>()
            .await
            .join(" ")
    }

    fn remember(self: &Arc<Self>, user_text: String, reply: String) {
        if let Err(e) = self
            .memory
            .add_message("user", &user_text)
            .and_then(|_| self.memory.add_message("assistant", &reply))
        {
            tracing::error!("Gespraech nicht gespeichert: {e:#}");
        }
        if self.memory_extraction {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                if let Err(e) = this.extract_facts(&user_text, &reply).await {
                    tracing::warn!("Fakten nicht aktualisiert: {e:#}");
                }
            });
        }
    }

    pub async fn extract_facts(&self, user_text: &str, reply: &str) -> Result<()> {
        let facts = self.memory.facts()?;
        let known = if facts.is_empty() {
            "(noch keine)".to_string()
        } else {
            facts
                .iter()
                .map(|f| format!("{}: {}", f.id, f.content))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let messages = [
            ChatMessage::new("system", EXTRACTION_PROMPT),
            ChatMessage::new(
                "user",
                format!("Gespeicherte Fakten:\n{known}\n\nNutzer: {user_text}\nAssistent: {reply}"),
            ),
        ];
        let answer = self.llm.complete(&messages, true).await?;
        let changes = parse_changes(&answer)?;
        if changes != FactChanges::default() {
            tracing::info!("Gedaechtnis: {changes:?}");
            self.memory.apply(&changes)?;
        }
        Ok(())
    }
}

/// Takes the outermost JSON object, models sometimes wrap it in prose or code fences.
fn parse_changes(answer: &str) -> Result<FactChanges> {
    let start = answer.find('{').unwrap_or(0);
    let end = answer.rfind('}').map(|i| i + 1).unwrap_or(answer.len());
    Ok(serde_json::from_str(&answer[start..end.max(start)])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FactUpdate;

    #[test]
    fn parses_changes_inside_code_fence() {
        let c = parse_changes(
            "```json\n{\"add\": [\"Mag Tee\"], \"update\": [{\"id\": 3, \"content\": \"x\"}]}\n```",
        )
        .unwrap();
        assert_eq!(
            c,
            FactChanges {
                add: vec!["Mag Tee".into()],
                update: vec![FactUpdate {
                    id: 3,
                    content: "x".into()
                }],
                delete: vec![]
            }
        );
    }
}
