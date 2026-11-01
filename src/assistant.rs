//! Personality, prompt assembly and fact extraction around the language model, per device.

use std::sync::Arc;

use anyhow::Result;
use chrono::Datelike;
use futures_util::{Stream, StreamExt};

use crate::llm::{ChatMessage, ChatModel};
use crate::sentences::SentenceSplitter;
use crate::store::{Device, FactChanges, Store};

/// For devices without a personality of their own.
const DEFAULT_PERSONA: &str = "Du bist ein freundlicher, hilfsbereiter Sprachassistent.";

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

pub struct Assistant {
    pub llm: Arc<dyn ChatModel>,
    pub store: Store,
}

impl Assistant {
    pub async fn build_messages(
        &self,
        device: &Device,
        user_text: &str,
    ) -> Result<Vec<ChatMessage>> {
        let persona = match device.persona.trim() {
            "" => DEFAULT_PERSONA,
            p => p,
        };
        let mut system = format!(
            "{persona}

{SPEECH_RULES}"
        );
        let now = chrono::Local::now();
        system.push_str(&format!(
            "

Jetzt ist {}, der {} um {} Uhr.",
            WEEKDAYS[now.weekday().num_days_from_monday() as usize],
            now.format("%d.%m.%Y"),
            now.format("%H:%M")
        ));
        let facts = self.store.facts(device.id).await?;
        if !facts.is_empty() {
            system.push_str(
                "

Das weißt du über den Nutzer:",
            );
            for f in &facts {
                system.push_str(&format!(
                    "
- {}",
                    f.content
                ));
            }
        }
        let mut messages = vec![ChatMessage::new("system", system)];
        for m in self
            .store
            .recent_messages(device.id, device.history_messages)
            .await?
        {
            messages.push(ChatMessage::new(&m.role, m.content));
        }
        messages.push(ChatMessage::new("user", user_text));
        Ok(messages)
    }

    /// Streams the reply sentence by sentence. Once the reply is complete it is stored and facts
    /// are extracted in the background. A failure yields a spoken apology instead.
    pub fn reply_stream(
        self: Arc<Self>,
        device: Device,
        user_text: String,
    ) -> impl Stream<Item = String> + Send {
        async_stream::stream! {
            let mut full = String::new();
            let mut splitter = SentenceSplitter::default();
            let tokens = async {
                let messages = self.build_messages(&device, &user_text).await?;
                let providers = self.store.providers_for(&device).await?;
                self.llm.stream(&providers, &messages).await
            }
            .await;
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
                Err(e) => tracing::error!("{}: {e:#}", device.name),
            }
            if full.trim().is_empty() {
                yield NO_ANSWER.to_string();
            } else {
                tracing::info!("{}: {}", device.name, full.trim());
                self.remember(device, user_text, full.trim().to_string()).await;
            }
        }
    }

    pub async fn reply(self: Arc<Self>, device: Device, user_text: String) -> String {
        self.reply_stream(device, user_text)
            .collect::<Vec<_>>()
            .await
            .join(" ")
    }

    async fn remember(self: &Arc<Self>, device: Device, user_text: String, reply: String) {
        let stored = async {
            self.store
                .add_message(device.id, "user", &user_text)
                .await?;
            self.store.add_message(device.id, "assistant", &reply).await
        };
        if let Err(e) = stored.await {
            tracing::error!("Gespraech nicht gespeichert: {e:#}");
        }
        if device.memory_extraction {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                if let Err(e) = this.extract_facts(&device, &user_text, &reply).await {
                    tracing::warn!("Fakten nicht aktualisiert: {e:#}");
                }
            });
        }
    }

    pub async fn extract_facts(&self, device: &Device, user_text: &str, reply: &str) -> Result<()> {
        let facts = self.store.facts(device.id).await?;
        let known = if facts.is_empty() {
            "(noch keine)".to_string()
        } else {
            facts
                .iter()
                .map(|f| format!("{}: {}", f.id, f.content))
                .collect::<Vec<_>>()
                .join(
                    "
",
                )
        };
        let messages = [
            ChatMessage::new("system", EXTRACTION_PROMPT),
            ChatMessage::new(
                "user",
                format!(
                    "Gespeicherte Fakten:
{known}

Nutzer: {user_text}
Assistent: {reply}"
                ),
            ),
        ];
        let providers = self.store.providers_for(device).await?;
        let answer = self.llm.complete(&providers, &messages, true).await?;
        let changes = parse_changes(&answer)?;
        if changes != FactChanges::default() {
            tracing::info!("Gedaechtnis {}: {changes:?}", device.name);
            self.store.apply(device.id, &changes).await?;
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
    use crate::store::FactUpdate;

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
