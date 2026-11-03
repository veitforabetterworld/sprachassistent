import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUp, ChevronRight, Loader2, Mic, Square, Volume2 } from "lucide-react";
import { VeitButton, VeitInput, VeitScrollArea } from "@veit/react-controls";
import { adminApi, speakUrl, transcribe, type Message, type ToolCallMeta } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

type Action = { id: string; name: string; detail: string; error: boolean };
type Turn = { id: string; user?: Message; actions: Action[]; reply?: Message };
type Speech = { id: string; url: string | null; player: HTMLAudioElement | null };

function pretty(value: string): string {
  const text = value.trim();
  if (!text) return "";
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

function toolBody(content: string): { name: string; body: string } {
  const start = content.match(/^\[tool_result name=(\S+) untrusted]\s*/);
  if (!start) return { name: "", body: content.trim() };
  let body = content.slice(start[0].length);
  const end = body.indexOf("[/tool_result]");
  if (end >= 0) body = body.slice(0, end);
  return { name: start[1], body: body.trim() };
}

function shortName(name: string): string {
  const parts = name.split(/__|_/).filter((part) => part && part !== "assist" && part !== "intent" && part !== "homeassistant");
  return parts.slice(-2).join(" ") || name;
}

function isErrorText(text: string): boolean {
  return /^\s*fehler/i.test(text) || /"error"\s*:/.test(text);
}

function callsOf(message: Message): ToolCallMeta[] {
  const calls = message.metadata?.tool_calls;
  return Array.isArray(calls) ? calls : [];
}

function toAction(message: Message, calls: ToolCallMeta[]): Action | null {
  if (message.role !== "tool") return null;
  const parsed = toolBody(message.content);
  const call = calls.find((item) => item.id && item.id === message.metadata?.tool_call_id);
  const name = shortName(message.metadata?.name || parsed.name || call?.name || "");
  const detail = [pretty(call?.arguments ?? ""), pretty(parsed.body)].filter(Boolean).join("\n");
  return { id: message.id, name, detail, error: isErrorText(parsed.body) };
}

/** One user line, the tool calls that followed it, then the spoken answer. */
function groupTurns(messages: Message[]): Turn[] {
  const turns: Turn[] = [];
  let turn: Turn | null = null;
  let calls: ToolCallMeta[] = [];
  const push = () => {
    if (turn && (turn.user || turn.reply || turn.actions.length > 0)) turns.push(turn);
  };
  for (const message of messages) {
    if (message.role === "user") {
      push();
      calls = [];
      turn = { id: message.id, user: message, actions: [] };
      continue;
    }
    if (!turn) turn = { id: message.id, actions: [] };
    const next = callsOf(message);
    if (next.length > 0) calls = next;
    if (!message.content.trim() && next.length > 0) continue;
    const action = toAction(message, calls);
    if (action) {
      turn.actions.push(action);
      continue;
    }
    if (message.role === "assistant" && message.content.trim()) turn.reply = message;
  }
  push();
  return turns;
}

function encodeWav(samples: Int16Array, rate: number): Blob {
  const data = samples.length * 2;
  const buffer = new ArrayBuffer(44 + data);
  const view = new DataView(buffer);
  const write = (offset: number, text: string) => {
    for (let i = 0; i < text.length; i++) view.setUint8(offset + i, text.charCodeAt(i));
  };
  write(0, "RIFF");
  view.setUint32(4, 36 + data, true);
  write(8, "WAVE");
  write(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, rate, true);
  view.setUint32(28, rate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  write(36, "data");
  view.setUint32(40, data, true);
  let offset = 44;
  for (const sample of samples) {
    view.setInt16(offset, sample, true);
    offset += 2;
  }
  return new Blob([buffer], { type: "audio/wav" }); // i18n-ignore: mime type
}

/** MediaRecorder output down to the 16 kHz mono WAV the recognizer reads. */
async function toWav16k(blob: Blob): Promise<Blob> {
  const context = new AudioContext();
  try {
    const decoded = await context.decodeAudioData(await blob.arrayBuffer());
    const length = decoded.length;
    const mono = new Float32Array(length);
    for (let channel = 0; channel < decoded.numberOfChannels; channel++) {
      const data = decoded.getChannelData(channel);
      for (let i = 0; i < length; i++) mono[i] += data[i] / decoded.numberOfChannels;
    }
    const rate = 16000;
    const outLength = Math.max(1, Math.floor((mono.length * rate) / decoded.sampleRate));
    const pcm = new Int16Array(outLength);
    for (let i = 0; i < outLength; i++) {
      const src = (i * decoded.sampleRate) / rate;
      const index = Math.floor(src);
      const frac = src - index;
      const a = mono[index] ?? 0;
      const b = mono[index + 1] ?? a;
      const sample = Math.max(-1, Math.min(1, a + (b - a) * frac));
      pcm[i] = sample < 0 ? sample * 0x8000 : sample * 0x7fff;
    }
    return encodeWav(pcm, rate);
  } finally {
    await context.close();
  }
}

function Actions({ actions }: { actions: Action[] }) {
  const { t, tf } = useI18n();
  const [open, setOpen] = useState(false);
  if (actions.length === 0) return null;
  const label = tf(actions.length === 1 ? "chatAction" : "chatActions", { count: actions.length });
  return (
    <div className="flex flex-col items-start gap-2">
      <VeitButton type="button" variant="ghost" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
        <ChevronRight className={`size-4 ${open ? "rotate-90" : ""}`} aria-hidden />
        {label}
      </VeitButton>
      {open && (
        <div className="flex w-full flex-col gap-2">
          {actions.map((action) => (
            <pre
              key={action.id}
              className={`m-0 overflow-x-auto whitespace-pre-wrap rounded-xl bg-muted px-3 py-2 text-xs ${action.error ? "text-destructive" : "text-muted-foreground"}`}
            >
              {action.name ? `${action.name}\n` : ""}
              {action.detail || t("error")}
            </pre>
          ))}
        </div>
      )}
    </div>
  );
}

/** Chat transcript with a one-line composer. Speech plays only from the speaker button. */
export function ChatPanel({ deviceId, historyVersion = 0 }: { deviceId: string; historyVersion?: number }) {
  const { t } = useI18n();
  const [messages, setMessages] = useState<Message[]>([]);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [recording, setRecording] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [playing, setPlaying] = useState<{ id: string; loading: boolean } | null>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const speech = useRef<Speech | null>(null);
  const recorder = useRef<MediaRecorder | null>(null);
  const chunks = useRef<Blob[]>([]);

  const reload = useCallback(() => {
    void adminApi.memory(deviceId).then((memory) => setMessages(memory.messages));
  }, [deviceId, historyVersion]);

  useEffect(reload, [reload]);
  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [messages, busy, notice]);

  /** At most one voice output: stops the current one (loading or playing) and frees its blob URL. */
  const stopAudio = useCallback(() => {
    const current = speech.current;
    speech.current = null;
    current?.player?.pause();
    if (current?.url) URL.revokeObjectURL(current.url);
    setPlaying(null);
  }, []);

  useEffect(() => stopAudio, [deviceId, stopAudio]);

  const send = async (body: string) => {
    const value = body.trim();
    if (!value || busy) return;
    setText("");
    setBusy(true);
    setNotice(null);
    setMessages((current) => [...current, { id: "pending", role: "user", content: value, created_at: new Date().toISOString() }]);
    try {
      await adminApi.chat(deviceId, value);
      const memory = await adminApi.memory(deviceId);
      setMessages(memory.messages);
    } catch (error) {
      setMessages((current) => current.filter((message) => message.id !== "pending"));
      setNotice(error instanceof Error ? error.message : t("error"));
    } finally {
      setBusy(false);
    }
  };

  const speak = async (message: Message) => {
    const current = speech.current;
    if (current?.id === message.id) {
      if (current.player) stopAudio();
      return;
    }
    stopAudio();
    setNotice(null);
    const entry: Speech = { id: message.id, url: null, player: null };
    speech.current = entry;
    setPlaying({ id: message.id, loading: true });
    try {
      const url = await speakUrl(deviceId, message.content);
      if (speech.current !== entry) {
        URL.revokeObjectURL(url);
        return;
      }
      entry.url = url;
      entry.player = new Audio(url);
      entry.player.onended = () => {
        if (speech.current === entry) stopAudio();
      };
      setPlaying({ id: message.id, loading: false });
      await entry.player.play();
    } catch (error) {
      if (speech.current !== entry) return;
      stopAudio();
      setNotice(error instanceof Error ? error.message : t("error"));
    }
  };

  const dictate = async () => {
    if (recorder.current && recorder.current.state === "recording") {
      recorder.current.stop();
      return;
    }
    setNotice(null);
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const media = new MediaRecorder(stream);
      chunks.current = [];
      media.ondataavailable = (event) => {
        if (event.data.size > 0) chunks.current.push(event.data);
      };
      media.onstop = () => {
        stream.getTracks().forEach((track) => track.stop());
        setRecording(false);
        const blob = new Blob(chunks.current, { type: media.mimeType });
        void toWav16k(blob)
          .then((wav) => transcribe(wav))
          .then((heard) => {
            const value = heard.trim();
            if (!value) setNotice(t("chatNothingHeard"));
            else setText((current) => (current.trim() ? `${current.trim()} ${value}` : value));
          })
          .catch((error: unknown) => setNotice(error instanceof Error ? error.message : t("error")));
      };
      recorder.current = media;
      media.start();
      setRecording(true);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : t("error"));
    }
  };

  const turns = groupTurns(messages.filter((message) => message.id !== "pending"));
  const pending = messages.find((message) => message.id === "pending");
  const empty = turns.length === 0 && !pending;

  return (
    <div className={`flex min-h-0 flex-1 flex-col gap-3 ${empty ? "justify-center" : ""}`}>
      {!empty && (
      <VeitScrollArea>
      <div className="flex flex-col gap-4 px-1">
        {turns.map((turn) => (
          <div key={turn.id} className="flex flex-col gap-3">
            {turn.user && (
              <div className="flex justify-end">
                <p className="m-0 max-w-md whitespace-pre-wrap rounded-3xl bg-primary px-4 py-2 text-sm text-primary-foreground">{turn.user.content}</p>
              </div>
            )}
            <Actions actions={turn.actions} />
            {turn.reply && (
              <div className="flex max-w-md flex-col items-start gap-1">
                <p className="m-0 whitespace-pre-wrap text-sm">{turn.reply.content}</p>
                <VeitButton
                  type="button"
                  variant="ghost"
                  aria-label={
                    playing?.id !== turn.reply.id ? t("chatSpeak") : playing.loading ? t("chatSpeakLoading") : t("chatStop")
                  }
                  disabled={playing?.id === turn.reply.id && playing.loading}
                  onClick={() => void speak(turn.reply!)}
                >
                  {playing?.id !== turn.reply.id ? (
                    <Volume2 className="size-4" aria-hidden />
                  ) : playing.loading ? (
                    <Loader2 className="size-4 animate-spin" aria-hidden />
                  ) : (
                    <Square className="size-4" aria-hidden />
                  )}
                </VeitButton>
              </div>
            )}
          </div>
        ))}
        {pending && (
          <div className="flex justify-end">
            <p className="m-0 max-w-md whitespace-pre-wrap rounded-3xl bg-primary px-4 py-2 text-sm text-primary-foreground">{pending.content}</p>
          </div>
        )}
        {busy && <p className="m-0 text-sm text-muted-foreground">…</p>}
        <div ref={bottom} />
      </div>
      </VeitScrollArea>
      )}
      {notice && <p className="m-0 text-sm text-destructive">{notice}</p>}
      <form
        className="flex items-center gap-2 rounded-full border border-border bg-muted/40 px-3 py-1.5"
        onSubmit={(event) => {
          event.preventDefault();
          void send(text);
        }}
      >
        <VeitInput
          className="input-plain min-w-0 flex-1"
          value={text}
          placeholder={t("chatPlaceholder")}
          disabled={busy}
          aria-label={t("chatPlaceholder")}
          onChange={(event) => setText(event.target.value)}
        />
        <VeitButton
          type="button"
          variant={recording ? "destructive" : "secondary"}
          aria-label={recording ? t("chatMicStop") : t("chatMic")}
          disabled={busy}
          onClick={() => void dictate()}
        >
          {recording ? <Square className="size-4" aria-hidden /> : <Mic className="size-4" aria-hidden />}
        </VeitButton>
        <VeitButton type="submit" aria-label={t("chatSend")} disabled={busy || recording || text.trim() === ""}>
          <ArrowUp className="size-4" aria-hidden />
        </VeitButton>
      </form>
    </div>
  );
}
