//! The agent loop shared by the speaker and the admin chat. The channel only changes the system
//! prompt and whether a busy line is spoken before tools run.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::assistant::{self, ToolRules};
use crate::authz::{approval_word, frame_tool_result, truncate, validate_args, Approval, Effect};
use crate::context::{self, ReplyCut};
use crate::failure;
use crate::llm::{model_tool_names, ChatMessage, ChatModel, ToolCall, ToolDef};
use crate::phrases::{self, Phrases};
use crate::runs::{RunGuard, CANCELLED, COMPLETED, FAILED, WAITING};
use crate::secrets::Secrets;
use crate::sentences::SentenceSplitter;
use crate::store::{Device, Store, StoredMessage};
use crate::tools::{ToolExecutor, ToolHints, ToolOffer};

const VOICE_BUDGET: Duration = Duration::from_secs(90);
const TEXT_BUDGET: Duration = Duration::from_secs(110);
const TOOL_TIMEOUT: Duration = Duration::from_secs(20);
/// Reading from outside (`ToolKind::Explore`) may wait for slow servers, several pages in a row.
const EXPLORE_TIMEOUT: Duration = Duration::from_secs(45);
const TOOL_RESULT_CHARS: usize = 2000;
/// What comes from outside is longer, a web page with its links fits whole.
const EXPLORE_RESULT_CHARS: usize = 6000;
/// After this many steps reading from outside the model answers from what it has.
const EXPLORE_STEPS: usize = 4;
/// The argument in which a tool takes the model's new context, see the tool's guide.
const CONTEXT_ARG: &str = "kontext";
const APPROVAL_TTL_SECS: i64 = 600;
const STILL_BUSY_AFTER: Duration = Duration::from_secs(15);
const HISTORY_CHARS: usize = 180;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Voice,
    Text,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Voice => "voice",
            Self::Text => "text",
        }
    }
}

/// What goes to the speaker: sentences to say, and what the assistant is doing in between,
/// so the device can show it.
#[derive(Debug, Clone, PartialEq)]
pub enum Voice {
    Say(String),
    Status(Status),
}

/// Phases that hold for any chatbot, whatever tools it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The model is writing.
    Thinking,
    /// A tool is running; the name is only shown in logs, the device does not interpret it.
    Tool(String),
    /// The first sentence is being turned into speech.
    Speaking,
}

impl Status {
    /// `thinking`, `tool:<name>` or `speaking`.
    pub fn label(&self) -> String {
        match self {
            Self::Thinking => "thinking".into(),
            Self::Tool(name) => format!("tool:{name}"),
            Self::Speaking => "speaking".into(),
        }
    }
}

#[derive(Clone)]
pub struct TurnResult {
    pub reply: String,
    pub status: String,
    pub run_id: Option<Uuid>,
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    llm: Arc<dyn ChatModel>,
    store: Store,
    tools: Arc<dyn ToolExecutor>,
    secrets: Secrets,
    device: Device,
    user_text: String,
    channel: Channel,
    say: Option<mpsc::Sender<Voice>>,
) -> TurnResult {
    let words = phrases::of(&device.voice);
    let Some(user_id) = device.user_id else {
        speak(&say, words.no_answer).await;
        return done(None, words.no_answer, FAILED);
    };
    let guard =
        match RunGuard::start(&store, user_id, device.id, channel.as_str(), &user_text).await {
            Ok(guard) => guard,
            Err(e) => {
                tracing::error!("Lauf nicht angelegt: {e:#}");
                speak(&say, words.no_answer).await;
                return done(None, words.no_answer, FAILED);
            }
        };

    if let Some(pending) = store.pending_approval(device.id).await.ok().flatten() {
        let age = chrono::Utc::now().signed_duration_since(pending.started_at);
        let word = approval_word(&user_text);
        if age.num_seconds() > APPROVAL_TTL_SECS || word == Approval::Other {
            reject_pending(&store, device.id, &pending.pending_tools).await;
            let _ = store.cancel_pending(pending.id).await;
        } else {
            let _ = store.close_pending(pending.id).await;
            return resume(
                llm, store, tools, secrets, device, user_id, channel, say, guard, pending, word,
            )
            .await;
        }
    }

    let offers = tools.list(user_id).await.unwrap_or_default();
    let use_tools = device.tools_enabled && !offers.is_empty();
    let mut messages = prompt(&store, &device, channel, &offers).await;
    messages.push(ChatMessage::new("user", user_text.clone()));
    context::attach(&mut messages, &device.context);

    if !use_tools {
        return stream_reply(
            llm, store, secrets, device, say, guard, messages, &user_text,
        )
        .await;
    }
    drive(
        llm, store, tools, secrets, device, user_id, channel, say, guard, messages, &user_text,
        offers, true,
    )
    .await
}

fn done(run_id: Option<Uuid>, reply: &str, status: &str) -> TurnResult {
    TurnResult {
        reply: reply.to_string(),
        status: status.to_string(),
        run_id,
    }
}

async fn fail(say: &Option<mpsc::Sender<Voice>>, guard: &mut RunGuard, reply: &str) -> TurnResult {
    speak(say, reply).await;
    let id = guard.id();
    guard.finish(FAILED, reply, reply, 0, 0, 0, None).await;
    done(Some(id), reply, FAILED)
}

async fn prompt(
    store: &Store,
    device: &Device,
    channel: Channel,
    offers: &[ToolOffer],
) -> Vec<ChatMessage> {
    let rules = (device.tools_enabled && !offers.is_empty()).then(|| ToolRules {
        writes: offers
            .iter()
            .any(|offer| tool_kind(offer.hints) == ToolKind::Change),
        guides: offers
            .iter()
            .map(|offer| offer.guide.trim())
            .filter(|guide| !guide.is_empty())
            .map(str::to_string)
            .collect(),
    });
    let mut messages = vec![ChatMessage::new(
        "system",
        assistant::system_prompt(device, channel, rules),
    )];
    // Everything older than the last exchange lives on only in the context.
    if let Ok(history) = store.last_exchange(device.id).await {
        messages.extend(trim_history(history));
    }
    messages
}

/// Keeps the context the model wrote, unless it wrote none or the same again.
async fn remember(store: &Store, device: &Device, new: Option<String>) {
    let Some(new) = new else {
        return;
    };
    if new != device.context {
        if let Err(e) = store.set_context(device.id, &new).await {
            tracing::warn!("Kontext nicht gespeichert: {e:#}");
        }
    }
}

fn trim_history(history: Vec<StoredMessage>) -> Vec<ChatMessage> {
    history
        .into_iter()
        .filter_map(|message| {
            if message.role == "tool" {
                return None;
            }
            let content = truncate(&message.content, HISTORY_CHARS);
            if message.role == "assistant" && content.trim().is_empty() {
                return None;
            }
            Some(ChatMessage::new(&message.role, content))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn stream_reply(
    llm: Arc<dyn ChatModel>,
    store: Store,
    secrets: Secrets,
    device: Device,
    say: Option<mpsc::Sender<Voice>>,
    mut guard: RunGuard,
    messages: Vec<ChatMessage>,
    user_text: &str,
) -> TurnResult {
    let providers = match store.providers_for(&device, &secrets).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!("{e:#}");
            return fail(&say, &mut guard, phrases::of(&device.voice).no_answer).await;
        }
    };
    let started = Instant::now();
    let mut cut = ReplyCut::default();
    show(&say, Status::Thinking).await;
    match llm.stream(&providers, &messages).await {
        Ok(mut tokens) => {
            let mut splitter = SentenceSplitter::default();
            while let Some(token) = tokens.next().await {
                match token {
                    Ok(piece) => {
                        let speakable = cut.push(&piece);
                        if let Some(tx) = &say {
                            for sentence in splitter.push(&speakable) {
                                if tx.send(Voice::Say(sentence)).await.is_err() {
                                    let reply = context::split(cut.full()).0;
                                    guard
                                        .finish(CANCELLED, &reply, "abgebrochen", 1, 0, 0, None)
                                        .await;
                                    return done(Some(guard.id()), &reply, CANCELLED);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("LLM-Stream abgebrochen: {e:#}");
                        break;
                    }
                }
            }
            if let Some(tx) = &say {
                let rest = cut.finish();
                for sentence in splitter.push(&rest) {
                    let _ = tx.send(Voice::Say(sentence)).await;
                }
                if let Some(rest) = splitter.finish() {
                    let _ = tx.send(Voice::Say(rest)).await;
                }
            }
        }
        Err(e) => {
            tracing::error!("{}: {e:#}", device.name);
            let reply = provider_failure_reply(phrases::of(&device.voice), &e);
            let _ = store.add_message(device.id, "user", user_text).await;
            let _ = store.add_message(device.id, "assistant", &reply).await;
            return fail(&say, &mut guard, &reply).await;
        }
    }
    let (reply, new_context) = context::split(cut.full());
    remember(&store, &device, new_context).await;
    if reply.is_empty() {
        let _ = store.add_message(device.id, "user", user_text).await;
        let _ = store
            .add_message(device.id, "assistant", phrases::of(&device.voice).no_answer)
            .await;
        return fail(&say, &mut guard, phrases::of(&device.voice).no_answer).await;
    }
    let _ = store.add_message(device.id, "user", user_text).await;
    let _ = store.add_message(device.id, "assistant", &reply).await;
    guard
        .step(
            1,
            "llm",
            "",
            json!({"ms": started.elapsed().as_millis() as u64}),
            started.elapsed().as_millis() as i32,
            "",
        )
        .await;
    guard.finish(COMPLETED, &reply, "", 1, 0, 0, None).await;
    tracing::info!("{}: {}", device.name, reply);
    done(Some(guard.id()), &reply, COMPLETED)
}

#[allow(clippy::too_many_arguments)]
async fn resume(
    llm: Arc<dyn ChatModel>,
    store: Store,
    tools: Arc<dyn ToolExecutor>,
    secrets: Secrets,
    device: Device,
    user_id: i64,
    channel: Channel,
    say: Option<mpsc::Sender<Voice>>,
    guard: RunGuard,
    pending: crate::store::PendingRun,
    word: Approval,
) -> TurnResult {
    let offers = tools.list(user_id).await.unwrap_or_default();
    let mut messages = prompt(&store, &device, channel, &offers).await;
    context::attach(&mut messages, &device.context);
    let calls = pending_calls(&pending.pending_tools);
    let results = if word == Approval::Yes {
        execute(
            &tools,
            user_id,
            &offers,
            calls,
            channel,
            &say,
            &device.still_busy_phrase,
        )
        .await
    } else {
        calls
            .into_iter()
            .map(|call| (call, "Der Nutzer hat abgelehnt.".to_string()))
            .collect()
    };
    let aliases = model_tool_names(offers.iter().map(|offer| offer.name.as_str()));
    for (call, body) in results {
        let label = aliases
            .get(&call.name)
            .map(String::as_str)
            .unwrap_or(&call.name);
        let framed = frame_tool_result(label, &body);
        messages.push(ChatMessage::tool_result(&call.id, truncate(&framed, 2000)));
        let _ = store
            .add_message_meta(
                device.id,
                "tool",
                &truncate(&framed, 500),
                json!({"tool_call_id": call.id, "name": call.name}),
            )
            .await;
    }
    drive(
        llm, store, tools, secrets, device, user_id, channel, say, guard, messages, "", offers,
        false,
    )
    .await
}

async fn reject_pending(store: &Store, device: Uuid, pending_tools: &Value) {
    for call in pending_calls(pending_tools) {
        let framed = frame_tool_result(
            &call.name,
            "Nicht ausgeführt, weil eine neue Frage kam. Das ist keine Ablehnung. Für die neue Frage das passende Werkzeug benutzen.",
        );
        let _ = store
            .add_message_meta(
                device,
                "tool",
                &truncate(&framed, 500),
                json!({"tool_call_id": call.id, "name": call.name}),
            )
            .await;
    }
}

fn pending_calls(value: &Value) -> Vec<ToolCall> {
    value
        .get("calls")
        .and_then(|v| v.as_array())
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| serde_json::from_value(call.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    llm: Arc<dyn ChatModel>,
    store: Store,
    tools: Arc<dyn ToolExecutor>,
    secrets: Secrets,
    device: Device,
    user_id: i64,
    channel: Channel,
    say: Option<mpsc::Sender<Voice>>,
    mut guard: RunGuard,
    mut messages: Vec<ChatMessage>,
    user_text: &str,
    offers: Vec<ToolOffer>,
    save_user: bool,
) -> TurnResult {
    // Every model may call tools; whether a chatbot gets them is the chatbot's own setting.
    let mut providers = match store.providers_for(&device, &secrets).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!("{e:#}");
            return fail(&say, &mut guard, phrases::of(&device.voice).no_answer).await;
        }
    };
    let aliases = model_tool_names(offers.iter().map(|offer| offer.name.as_str()));
    let kinds = Kinds::new(&offers, &aliases);
    // Every enabled tool goes to the model. What a chatbot should not use is switched off in
    // its tool settings, not here.
    let defs: Vec<ToolDef> = offers
        .iter()
        .map(|offer| ToolDef {
            name: aliases
                .get(&offer.name)
                .cloned()
                .unwrap_or_else(|| offer.name.clone()),
            description: offer.description.clone(),
            parameters: offer.parameters.clone(),
        })
        .collect();
    let budget = match channel {
        Channel::Voice => VOICE_BUDGET,
        Channel::Text => TEXT_BUDGET,
    };
    let started = Instant::now();
    let limit = device.max_iterations.max(1);
    let mut iterations = 0;
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut said_busy = false;
    let mut seq = 0i32;
    let mut user_saved = !save_user;
    let mut phase = PromptPhase::Lookup;
    let mut nudged = false;
    let mut gathered = false;
    let mut action_misses = 0;
    let mut settle = false;
    let can_reread = defs
        .iter()
        .any(|tool| kinds.of(&tool.name) == ToolKind::Read);
    let can_change = defs
        .iter()
        .any(|tool| kinds.of(&tool.name) == ToolKind::Change);
    let mut explore_steps = 0usize;
    let mut explored = HashSet::new();
    let mut explore_results: Vec<(usize, String, String)> = Vec::new();
    // The context a tool call carried, kept when the final reply brings none.
    let mut written_context: Option<String> = None;
    loop {
        if started.elapsed() > budget || iterations >= limit {
            return fail(&say, &mut guard, phrases::of(&device.voice).too_long).await;
        }
        iterations += 1;
        // After a tool result the old chat stays out; the context in front of the question
        // stays. Changes stay available until one has run, so reading the state can be
        // followed by the change itself.
        let short;
        let sent = if phase == PromptPhase::Lookup && !gathered {
            messages.as_slice()
        } else {
            short = search_follow_up(&messages);
            short.as_slice()
        };
        let offered_owned;
        let offered = match phase {
            PromptPhase::Lookup => {
                // A change waits for a look at the state, if there is a tool to look with.
                offered_owned = defs
                    .iter()
                    .filter(|tool| kinds.of(&tool.name) != ToolKind::Change || !can_reread)
                    .cloned()
                    .collect::<Vec<_>>();
                offered_owned.as_slice()
            }
            PromptPhase::Act => {
                offered_owned = defs
                    .iter()
                    .filter(|tool| {
                        matches!(kinds.of(&tool.name), ToolKind::Change | ToolKind::Unknown)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                offered_owned.as_slice()
            }
            PromptPhase::Answer => {
                offered_owned = Vec::new();
                offered_owned.as_slice()
            }
        };
        // Tools are offered, never forced: a greeting needs none, and a forced call kept the
        // model from ever answering. `complete` moves on to the next model at a limit.
        // On the speaker the text is spoken while it arrives, unless a nudge could still send
        // this first reply back (`nudge_tools`): what was said cannot be taken back.
        show(&say, Status::Thinking).await;
        let nudge_possible = phase == PromptPhase::Lookup
            && !nudged
            && !gathered
            && providers.first().is_some_and(|p| p.nudge_tools);
        let mut spoke = false;
        let turn_result = match say.as_ref().filter(|_| !nudge_possible) {
            Some(tx) => {
                let (result, said, gone) = ask_and_speak(&llm, &providers, sent, offered, tx).await;
                spoke = said;
                if gone {
                    guard
                        .finish(
                            CANCELLED,
                            "",
                            "abgebrochen",
                            iterations,
                            input_tokens,
                            output_tokens,
                            None,
                        )
                        .await;
                    return done(Some(guard.id()), "", CANCELLED);
                }
                result
            }
            None => llm.complete(&providers, sent, offered, false, false).await,
        };
        let mut turn = match turn_result {
            Ok(turn) => turn,
            Err(e) => {
                let detail = e.to_string();
                if phase == PromptPhase::Act
                    && detail.contains("tool_use_failed")
                    && action_misses < 3
                {
                    action_misses += 1;
                    messages.push(ChatMessage::new(
                        "system",
                        "Der letzte Aufruf war ungültig. Benutze nur ein angebotenes Werkzeug. \
Argumente aus dem vorherigen Werkzeugergebnis übernehmen.",
                    ));
                    continue;
                }
                tracing::error!("{}: {e:#}", device.name);
                let reply = provider_failure_reply(phrases::of(&device.voice), &e);
                if !user_saved && !user_text.is_empty() {
                    let _ = store.add_message(device.id, "user", user_text).await;
                }
                let _ = store.add_message(device.id, "assistant", &reply).await;
                return fail(&say, &mut guard, &reply).await;
            }
        };
        // Stay on the provider that answered. The others stay behind it, so a later outage or
        // limit still moves to the next one; the transcript is in the messages.
        if let Some(at) = providers
            .iter()
            .position(|provider| provider.name == turn.provider)
        {
            let answered = providers.remove(at);
            providers.insert(0, answered);
        }
        input_tokens += turn.input_tokens;
        output_tokens += turn.output_tokens;
        seq += 1;
        guard
            .step(
                seq,
                "llm",
                &turn.provider,
                json!({
                    "tool_calls": turn.tool_calls.len(),
                    "finish": turn.finish_reason,
                    "output_tokens": turn.output_tokens,
                }),
                0,
                "",
            )
            .await;
        if turn.tool_calls.is_empty() {
            if let Some(call) = recover_text_tool_call(&turn.content, offered) {
                turn.tool_calls.push(call);
                turn.content.clear();
            }
        }
        if should_nudge(
            turn.nudge_tools,
            phase,
            turn.tool_calls.is_empty(),
            nudged,
            gathered,
            !offered.is_empty(),
        ) {
            nudged = true;
            messages.push(ChatMessage::new(
                "system",
                "Neue Anfrage. Bestätige keine erledigte Aktion aus dem Verlauf. \
Wenn aktuelle Daten oder eine Änderung nötig sind, rufe jetzt ein angebotenes Werkzeug auf.",
            ));
            continue;
        }
        if turn.tool_calls.is_empty() {
            let (answer, new_context) = context::split(&turn.content);
            remember(&store, &device, new_context.or(written_context.take())).await;
            let reply = spoken_reply(phrases::of(&device.voice), &answer);
            if reply.is_empty() {
                return fail(&say, &mut guard, phrases::of(&device.voice).no_answer).await;
            }
            // Streamed text has been spoken already.
            if !spoke && !speak(&say, &reply).await {
                guard
                    .finish(
                        CANCELLED,
                        &reply,
                        "abgebrochen",
                        iterations,
                        input_tokens,
                        output_tokens,
                        None,
                    )
                    .await;
                return done(Some(guard.id()), &reply, CANCELLED);
            }
            if !user_saved && !user_text.is_empty() {
                let _ = store.add_message(device.id, "user", user_text).await;
            }
            let _ = store.add_message(device.id, "assistant", &reply).await;
            guard
                .finish(
                    COMPLETED,
                    &reply,
                    "",
                    iterations,
                    input_tokens,
                    output_tokens,
                    None,
                )
                .await;
            tracing::info!("{}: {}", device.name, reply);
            return done(Some(guard.id()), &reply, COMPLETED);
        }

        // A model that said something before its tool call has filled the pause itself.
        if channel == Channel::Voice && !said_busy && !spoke {
            said_busy = true;
            if !speak(&say, &device.busy_phrase).await {
                guard
                    .finish(
                        CANCELLED,
                        "",
                        "abgebrochen",
                        iterations,
                        input_tokens,
                        output_tokens,
                        None,
                    )
                    .await;
                return done(Some(guard.id()), "", CANCELLED);
            }
        }

        let calls = turn.tool_calls.clone();
        let mut auto = Vec::new();
        let mut confirm = Vec::new();
        let mut finished = Vec::new();
        let mut repeated = 0usize;
        for call in turn.tool_calls {
            let Some(offer) = offer_for(&offers, &aliases, &call.name) else {
                finished.push((call, "Dieses Werkzeug gibt es nicht.".into()));
                continue;
            };
            let mut exec = call;
            exec.name = offer.name.clone();
            // The same request outside twice only costs tokens; the first result is still above.
            if kinds.of(&exec.name) == ToolKind::Explore
                && !explored.insert(call_key(&exec.arguments))
            {
                repeated += 1;
                finished.push((exec, ALREADY_READ.into()));
                continue;
            }
            match offer.effect {
                Effect::Confirm => confirm.push(exec),
                Effect::Auto => auto.push(exec),
                Effect::Deny => finished.push((exec, "Diese Aktion ist gesperrt.".into())),
            }
        }
        let saw_read = auto
            .iter()
            .any(|call| kinds.of(&call.name) == ToolKind::Read);
        let explore_now = auto
            .iter()
            .filter(|call| kinds.of(&call.name) == ToolKind::Explore)
            .count();
        // One change at a time. A second change to the same thing can cancel the first
        // before it has finished.
        let mut kept = Vec::new();
        let mut deferred = Vec::new();
        let mut kept_action = false;
        for call in auto {
            if kinds.of(&call.name) == ToolKind::Change {
                if kept_action {
                    deferred.push(call);
                } else {
                    kept_action = true;
                    kept.push(call);
                }
            } else {
                kept.push(call);
            }
        }
        let saw_action = kept_action;
        explore_steps += explore_now + repeated;
        // Asking again for what is already above means the model has what it can get: answer.
        let only_repeats = repeated > 0 && explore_now == 0 && !saw_action && !saw_read;
        let stop_exploring = only_repeats
            || (explore_now + repeated > 0 && explore_steps >= EXPLORE_STEPS && !saw_action);
        finished.extend(
            execute(
                &tools,
                user_id,
                &offers,
                kept,
                channel,
                &say,
                &device.still_busy_phrase,
            )
            .await,
        );
        if saw_action {
            // `execute` marks a failed call this way, also one the MCP server reported.
            let failed = finished
                .iter()
                .any(|(_, body)| body.to_lowercase().starts_with("fehler"));
            if failed && action_misses < 1 {
                action_misses += 1;
                phase = PromptPhase::Act;
            } else if !settle && can_reread {
                settle = true;
                phase = PromptPhase::Lookup;
                messages.push(ChatMessage::new(
                    "system",
                    "Der Auftrag wurde nur angenommen, der neue Stand ist damit nicht belegt. \
Lies ihn jetzt erneut. Nenne den Wert aus diesem Lese-Ergebnis, nicht die Zahl, die du gesendet hast.",
                ));
            } else {
                phase = PromptPhase::Answer;
            }
        } else if stop_exploring {
            phase = PromptPhase::Answer;
        } else if explore_now > 0 {
            phase = PromptPhase::Lookup;
        } else if saw_read {
            phase = if settle {
                PromptPhase::Answer
            } else if can_change {
                PromptPhase::Act
            } else {
                PromptPhase::Lookup
            };
        }
        if !deferred.is_empty() {
            let names = deferred
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            messages.push(ChatMessage::new(
                "system",
                format!(
                    "Nicht gestartet, damit der erste Auftrag zu Ende laufen kann: {names}. \
Erst nach dem neuen Lese-Ergebnis einzeln aufrufen, falls es noch nötig ist."
                ),
            ));
        }
        for (call, body) in &finished {
            seq += 1;
            guard
                .step(
                    seq,
                    "tool_result",
                    &call.name,
                    json!({"result": truncate(body, 500)}),
                    0,
                    "",
                )
                .await;
        }

        if !user_saved && !user_text.is_empty() {
            let _ = store.add_message(device.id, "user", user_text).await;
            user_saved = true;
        }
        // A new step outside carries the model's new context with what it read so far. The
        // older results then leave the prompt; only the newest one is sent in full.
        let written = context_of_calls(&calls);
        if explore_now > 0 {
            for (at, label, start) in explore_results.drain(..) {
                messages[at].content = compact_result(&label, &start, written.is_some());
            }
        }
        if written.is_some() {
            written_context = written;
        }
        messages.push(ChatMessage {
            role: "assistant".into(),
            content: turn.content.clone(),
            tool_call_id: None,
            tool_calls: calls.clone(),
        });
        let _ = store
            .add_message_meta(
                device.id,
                "assistant",
                &turn.content,
                json!({"tool_calls": calls}),
            )
            .await;
        for (call, body) in &finished {
            let label = aliases
                .get(&call.name)
                .map(String::as_str)
                .unwrap_or(&call.name);
            let framed = frame_tool_result(label, body);
            let limit = if kinds.of(&call.name) == ToolKind::Explore {
                explore_results.push((
                    messages.len(),
                    label.to_string(),
                    truncate(body, OLD_RESULT_CHARS),
                ));
                EXPLORE_RESULT_CHARS
            } else {
                TOOL_RESULT_CHARS
            };
            messages.push(ChatMessage::tool_result(&call.id, truncate(&framed, limit)));
            let _ = store
                .add_message_meta(
                    device.id,
                    "tool",
                    &truncate(&framed, 500),
                    json!({"tool_call_id": call.id, "name": call.name}),
                )
                .await;
        }
        if !finished.is_empty() {
            gathered = true;
        }
        if stop_exploring {
            messages.push(ChatMessage::new(
                "system",
                "Keine weiteren Recherche-Schritte. Antworte jetzt aus den Ergebnissen und nenne die Quellen.",
            ));
        }
        if !confirm.is_empty() {
            let question = confirm_question(phrases::of(&device.voice), &confirm, &offers);
            if !speak(&say, &question).await {
                guard
                    .finish(
                        CANCELLED,
                        &question,
                        "abgebrochen",
                        iterations,
                        input_tokens,
                        output_tokens,
                        None,
                    )
                    .await;
                return done(Some(guard.id()), &question, CANCELLED);
            }
            guard
                .finish(
                    WAITING,
                    &question,
                    "",
                    iterations,
                    input_tokens,
                    output_tokens,
                    Some(json!({"calls": confirm})),
                )
                .await;
            return done(Some(guard.id()), &question, WAITING);
        }
    }
}

fn offer_for<'a>(
    offers: &'a [ToolOffer],
    aliases: &HashMap<String, String>,
    called: &str,
) -> Option<&'a ToolOffer> {
    offers.iter().find(|offer| {
        offer.name == called
            || aliases
                .get(&offer.name)
                .is_some_and(|alias| alias == called)
    })
}

async fn execute(
    tools: &Arc<dyn ToolExecutor>,
    user_id: i64,
    offers: &[ToolOffer],
    calls: Vec<ToolCall>,
    channel: Channel,
    say: &Option<mpsc::Sender<Voice>>,
    still: &str,
) -> Vec<(ToolCall, String)> {
    let mut out = Vec::new();
    let mut warned = false;
    for call in calls {
        show(say, Status::Tool(call.name.clone())).await;
        let offer = offers.iter().find(|offer| offer.name == call.name);
        let schema = offer
            .map(|offer| offer.parameters.clone())
            .unwrap_or_else(|| json!({}));
        let timeout = match offer.map(|offer| tool_kind(offer.hints)) {
            Some(ToolKind::Explore) => EXPLORE_TIMEOUT,
            _ => TOOL_TIMEOUT,
        };
        let tools = Arc::clone(tools);
        let run = async move {
            let args = serde_json::from_str::<Value>(&call.arguments).unwrap_or(json!({}));
            if let Err(error) = validate_args(&schema, &args) {
                return (call, format!("Fehler: {error}"));
            }
            match tokio::time::timeout(timeout, tools.execute(user_id, &call.name, args)).await {
                Ok(Ok(text)) => (call, text),
                Ok(Err(error)) => (call, format!("Fehler: {error:#}")),
                Err(_) => (call, "Zeitüberschreitung".into()),
            }
        };
        tokio::pin!(run);
        let result = loop {
            tokio::select! {
                result = &mut run => break result,
                _ = tokio::time::sleep(STILL_BUSY_AFTER), if channel == Channel::Voice && !warned => {
                    warned = true;
                    speak(say, still).await;
                }
            }
        };
        out.push(result);
    }
    out
}

fn confirm_question(words: &Phrases, calls: &[ToolCall], offers: &[ToolOffer]) -> String {
    let labels: Vec<String> = calls
        .iter()
        .map(|call| {
            offers
                .iter()
                .find(|offer| offer.name == call.name)
                .map(|offer| truncate(&offer.description, 80))
                .unwrap_or_else(|| call.name.clone())
        })
        .collect();
    words.confirm(&labels)
}

/// Sends the text as spoken sentences. Returns false when the listener is gone.
async fn speak(say: &Option<mpsc::Sender<Voice>>, text: &str) -> bool {
    let Some(tx) = say else {
        return true;
    };
    let mut splitter = SentenceSplitter::default();
    let mut parts = splitter.push(text);
    if let Some(rest) = splitter.finish() {
        parts.push(rest);
    }
    if parts.is_empty() && !text.trim().is_empty() {
        parts.push(text.trim().to_string());
    }
    for part in parts {
        if tx.send(Voice::Say(part)).await.is_err() {
            return false;
        }
    }
    true
}

/// Tells the speaker what is going on; a listener that is gone notices at the next sentence.
async fn show(say: &Option<mpsc::Sender<Voice>>, status: Status) {
    if let Some(tx) = say {
        let _ = tx.send(Voice::Status(status)).await;
    }
}

/// Lets the model write with tools offered and speaks its text while it arrives. A tool call
/// it writes as text or the context block are never spoken. Returns the turn, whether anything
/// was spoken and whether the listener went away.
async fn ask_and_speak(
    llm: &Arc<dyn ChatModel>,
    providers: &[crate::llm::Provider],
    messages: &[ChatMessage],
    offered: &[ToolDef],
    tx: &mpsc::Sender<Voice>,
) -> (anyhow::Result<crate::llm::ModelTurn>, bool, bool) {
    let (pieces, mut received) = mpsc::channel::<String>(64);
    let ask = llm.complete_streamed(providers, messages, offered, pieces);
    let speak_along = async {
        let mut cut = ReplyCut::default();
        let mut splitter = SentenceSplitter::default();
        let (mut spoke, mut gone) = (false, false);
        while let Some(piece) = received.recv().await {
            for sentence in splitter.push(&cut.push(&piece)) {
                spoke = true;
                gone |= tx.send(Voice::Say(sentence)).await.is_err();
            }
        }
        let mut rest = splitter.push(&cut.finish());
        rest.extend(splitter.finish());
        for sentence in rest {
            spoke = true;
            gone |= tx.send(Voice::Say(sentence)).await.is_err();
        }
        (spoke, gone)
    };
    let (turn, (spoke, gone)) = tokio::join!(ask, speak_along);
    (turn, spoke, gone)
}

/// Only for models with `nudge_tools` on: their text reply before any tool result may be a
/// confirmation copied from the old chat, so it goes back once with a hint to use a tool. After a
/// lookup the same reply is the answer and must not be thrown away for another tool call.
fn should_nudge(
    model_wants_it: bool,
    phase: PromptPhase,
    no_tool_call: bool,
    nudged: bool,
    gathered: bool,
    lookup_offered: bool,
) -> bool {
    model_wants_it
        && phase == PromptPhase::Lookup
        && no_tool_call
        && !nudged
        && !gathered
        && lookup_offered
}

fn provider_failure_reply(words: &Phrases, e: &anyhow::Error) -> String {
    if e.to_string().contains("tool_use_failed") {
        words.no_tool.into()
    } else if failure::code_of(e) == Some("llm.at_limit") {
        words.at_limit.into()
    } else {
        words.unreachable.into()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptPhase {
    Lookup,
    Act,
    Answer,
}

/// What a tool does, from its MCP annotations only. The phases of a turn follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    /// Reads and changes nothing (`readOnlyHint`).
    Read,
    /// Reads from outside (`readOnlyHint` and `openWorldHint`): slower, longer results, and
    /// often several steps one after another.
    Explore,
    /// May change something (`readOnlyHint: false`).
    Change,
    /// Says nothing about itself: offered in every phase and never assumed to read or change.
    Unknown,
}

fn tool_kind(hints: ToolHints) -> ToolKind {
    match (hints.read_only, hints.open_world) {
        (Some(true), Some(true)) => ToolKind::Explore,
        (Some(true), _) => ToolKind::Read,
        (Some(false), _) => ToolKind::Change,
        (None, _) => ToolKind::Unknown,
    }
}

/// The kind of every offered tool, by its real name and by the name the model sees.
struct Kinds(HashMap<String, ToolKind>);

impl Kinds {
    fn new(offers: &[ToolOffer], aliases: &HashMap<String, String>) -> Self {
        let mut kinds = HashMap::new();
        for offer in offers {
            let kind = tool_kind(offer.hints);
            if let Some(alias) = aliases.get(&offer.name) {
                kinds.insert(alias.clone(), kind);
            }
            kinds.insert(offer.name.clone(), kind);
        }
        Self(kinds)
    }

    fn of(&self, name: &str) -> ToolKind {
        self.0.get(name).copied().unwrap_or(ToolKind::Unknown)
    }
}

/// Groq sometimes prints the call as text after a bad example in the history.
fn spoken_reply(words: &Phrases, content: &str) -> String {
    let cut = content
        .find("<tool_call>")
        .or_else(|| content.find("<function="))
        .unwrap_or(content.len());
    let spoken = content[..cut].trim();
    if !spoken.is_empty() {
        return spoken.to_string();
    }
    if cut < content.len() {
        words.not_done.into()
    } else {
        String::new()
    }
}

fn recover_text_tool_call(content: &str, defs: &[ToolDef]) -> Option<ToolCall> {
    let call = crate::llm::inline_tool_call(content)?;
    defs.iter().any(|def| def.name == call.name).then_some(call)
}

const ALREADY_READ: &str =
    "Schon gelesen, was davon zählt, steht oben oder in deinem Kontext. Frag etwas anderes ab oder antworte.";

/// What is left of an older result from outside when the model has moved on.
const OLD_RESULT_CHARS: usize = 400;

/// Arguments of a call with the order of keys and spacing evened out.
/// The context changes with every step and does not make a different request.
fn call_key(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .map(|mut args| {
            if let Some(map) = args.as_object_mut() {
                map.remove(CONTEXT_ARG);
            }
            args.to_string()
        })
        .unwrap_or_else(|_| arguments.trim().to_string())
}

/// The context the model wrote into this turn's tool calls, cut to the limit.
fn context_of_calls(calls: &[ToolCall]) -> Option<String> {
    let written = calls
        .iter()
        .filter_map(|call| serde_json::from_str::<Value>(&call.arguments).ok())
        .filter_map(|args| {
            args.get(CONTEXT_ARG)?
                .as_str()
                .map(|s| s.trim().to_string())
        })
        .rfind(|written| !written.is_empty())?;
    Some(written.chars().take(context::CONTEXT_CHARS).collect())
}

/// An older result from outside reduced to a pointer to the new context, or to its start
/// when the model wrote none.
fn compact_result(label: &str, start: &str, context_written: bool) -> String {
    if !context_written {
        return frame_tool_result(label, &format!("Älteres Ergebnis, gekürzt:\n{start}"));
    }
    frame_tool_result(
        label,
        "Älteres Ergebnis, ersetzt durch deinen Kontext im nächsten Aufruf.",
    )
}

fn search_follow_up(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    let mut systems = messages.iter().filter(|message| message.role == "system");
    if let Some(system) = systems.next() {
        out.push(system.clone());
    }
    let later_systems: Vec<_> = systems.cloned().collect();
    if let Some(user) = messages.iter().rev().find(|message| message.role == "user") {
        out.push(user.clone());
    }
    let mut tail = Vec::new();
    for message in messages.iter().rev() {
        if message.role == "tool" || (message.role == "assistant" && !message.tool_calls.is_empty())
        {
            tail.push(message.clone());
            continue;
        }
        break;
    }
    tail.reverse();
    out.extend(tail);
    out.extend(later_systems);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_tool_call_becomes_a_real_call() {
        let defs = vec![ToolDef {
            name: "web_search".into(),
            description: String::new(),
            parameters: json!({}),
        }];
        let content = "<tool_call>\n<function=web_search>\n<parameter=query>\nSchlagzeilen heute\n</parameter>\n</function>\n</tool_call>";
        let call = recover_text_tool_call(content, &defs).unwrap();
        assert_eq!(call.name, "web_search");
        assert!(call.arguments.contains("Schlagzeilen heute"));
        assert!(recover_text_tool_call(content, &[]).is_none());
    }

    #[test]
    fn answer_after_a_lookup_is_kept() {
        assert!(should_nudge(
            true,
            PromptPhase::Lookup,
            true,
            false,
            false,
            true
        ));
        assert!(!should_nudge(
            false,
            PromptPhase::Lookup,
            true,
            false,
            false,
            true
        ));
        assert!(!should_nudge(
            true,
            PromptPhase::Lookup,
            true,
            false,
            true,
            true
        ));
        assert!(!should_nudge(
            true,
            PromptPhase::Lookup,
            true,
            true,
            false,
            true
        ));
        assert!(!should_nudge(
            true,
            PromptPhase::Lookup,
            false,
            false,
            false,
            true
        ));
        assert!(!should_nudge(
            true,
            PromptPhase::Answer,
            true,
            false,
            false,
            true
        ));
    }

    #[test]
    fn limit_is_only_reported_when_every_model_is_at_it() {
        let words = phrases::of("");
        let all = failure::failure("llm.at_limit", "Groq: HTTP 429");
        assert_eq!(provider_failure_reply(words, &all), words.at_limit);
        let mixed = failure::failure("llm.bad_request", "Gemini: HTTP 400; Groq: HTTP 429");
        assert_ne!(provider_failure_reply(words, &mixed), words.at_limit);
        let key = failure::failure("llm.unauthorized", "Groq: HTTP 401");
        assert!(!provider_failure_reply(words, &key).contains("401"));
    }

    fn hints(read_only: Option<bool>, open_world: Option<bool>) -> ToolHints {
        ToolHints {
            read_only,
            open_world,
            ..ToolHints::default()
        }
    }

    #[test]
    fn the_kind_comes_from_the_annotations_not_the_name() {
        assert_eq!(tool_kind(hints(Some(true), None)), ToolKind::Read);
        assert_eq!(tool_kind(hints(Some(true), Some(false))), ToolKind::Read);
        assert_eq!(tool_kind(hints(Some(true), Some(true))), ToolKind::Explore);
        assert_eq!(tool_kind(hints(Some(false), Some(true))), ToolKind::Change);
        assert_eq!(tool_kind(hints(None, Some(true))), ToolKind::Unknown);
        let from_row = ToolHints::from_annotations(
            &json!({"readOnlyHint": true, "destructiveHint": null, "openWorldHint": true}),
        );
        assert_eq!(tool_kind(from_row), ToolKind::Explore);
        assert_eq!(
            tool_kind(ToolHints::from_annotations(&json!({}))),
            ToolKind::Unknown
        );
    }

    #[test]
    fn kinds_know_the_name_the_model_sees() {
        let offer = |name: &str, hints| ToolOffer {
            name: name.into(),
            description: String::new(),
            parameters: json!({}),
            effect: Effect::Auto,
            hints,
            guide: String::new(),
        };
        let offers = vec![
            offer("get_status", hints(Some(false), None)),
            offer("mail--search", hints(Some(true), Some(true))),
            offer("send_everything", hints(None, None)),
        ];
        let aliases = model_tool_names(offers.iter().map(|offer| offer.name.as_str()));
        let kinds = Kinds::new(&offers, &aliases);
        // A name that sounds like reading changes nothing about what the server says.
        assert_eq!(kinds.of("get_status"), ToolKind::Change);
        assert_eq!(kinds.of("mail--search"), ToolKind::Explore);
        assert_eq!(kinds.of(&aliases["mail--search"]), ToolKind::Explore);
        assert_eq!(kinds.of("send_everything"), ToolKind::Unknown);
        assert_eq!(kinds.of("never_offered"), ToolKind::Unknown);
    }

    #[test]
    fn any_tool_can_carry_the_new_context() {
        let call = |arguments: &str| ToolCall {
            id: "a".into(),
            name: "irgendwas".into(),
            arguments: arguments.into(),
            thought_signature: None,
        };
        assert_eq!(
            context_of_calls(&[call(r#"{"kontext":" neu "}"#), call("{}")]).as_deref(),
            Some("neu")
        );
        assert!(context_of_calls(&[call(r#"{"kontext":""}"#)]).is_none());
    }

    #[test]
    fn the_same_call_is_recognised_despite_spacing() {
        let first = call_key(r#"{"action":"follow","url":"https://a.de/x"}"#);
        let again = call_key(" {\"action\": \"follow\",\n \"url\": \"https://a.de/x\"} ");
        let other = call_key(r#"{"action":"follow","url":"https://a.de/y"}"#);
        assert_eq!(first, again);
        assert_ne!(first, other);
        let with_notes = call_key(r#"{"action":"follow","url":"https://a.de/x","kontext":"neu"}"#);
        assert_eq!(first, with_notes);
    }
}
