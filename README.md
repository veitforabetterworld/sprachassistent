# Sprachassistent

Sprachassistent mit Persönlichkeit und Gedächtnis. Ein ESP32 dient als Mikrofon und Lautsprecher, die Arbeit macht ein Rust-Server auf dem Raspberry Pi 5:

```
ESP32 (INMP441, MAX98357A, Taster)
  │  POST /v1/talk  (16 kHz PCM, chunked)
  ▼
sprachassistent.plenivo.de ─ nginx ─ 127.0.0.1:3020  Rust-Server (axum)
  ├─ Spracherkennung  whisper.cpp (whisper-rs)       lokal
  ├─ Gedächtnis       SQLite: Verlauf + Fakten        lokal
  ├─ Persönlichkeit   data/persona.md (API)           lokal
  ├─ KI               Groq → Gemini (Fallback)        extern, nur Text
  └─ Sprachausgabe    Piper-Stimme (sherpa-rs)        lokal
  ▼  Antwort als PCM, Satz für Satz gestreamt
ESP32 spielt ab
```

Nur Text verlässt das Heimnetz. Laufende Kosten: Strom für den Pi und die Gratis-Kontingente von Groq und Gemini.

## Struktur

```
src/                 Rust-Server
  api.rs             HTTP-Endpunkte
  assistant.rs       Prompt, Persönlichkeit, Fakten-Extraktion
  speech.rs          Whisper (Feature whisper) und Piper (Feature piper)
  llm.rs             OpenAI-kompatible Anbieter mit Fallback, Streaming
  memory.rs          SQLite
  sentences.rs       Antwort-Stream in Sätze zerlegen
persona.md           Standard-Persönlichkeit (eingebaut)
firmware/            ESP32 (PlatformIO, Arduino)
deployments/build/   Cross-Compile für den Pi (Docker, Debian bookworm → aarch64)
deployments/pi/      Bootstrap, Runtime-Reload, nginx, pm2, Modell-Download
```

## API

Alles außer `/v1/health` braucht `Authorization: Bearer <API_KEY>`.

| Methode | Pfad | |
|---|---|---|
| GET | `/v1/health` | Status, ob Erkennung und Ausgabe geladen sind, Anbieter |
| POST | `/v1/talk` | Audio rein (rohes s16le 16 kHz oder WAV), gesprochene Antwort raus. `?format=wav` für WAV statt PCM. Header `X-Sample-Rate`, `X-Transcript` |
| POST | `/v1/transcribe` | Audio → `{"text"}` |
| POST | `/v1/chat` | `{"text"}` → `{"reply"}`, ohne Audio, mit Gedächtnis |
| POST | `/v1/speak` | `{"text"}` → Audio |
| GET | `/v1/memory` | Fakten und letzte Nachrichten (`?messages=50`) |
| DELETE | `/v1/memory` | `?what=facts\|messages\|all` |
| POST | `/v1/memory/facts` | `{"content"}` |
| PUT, DELETE | `/v1/memory/facts/{id}` | |
| GET, PUT, DELETE | `/v1/persona` | Persönlichkeit als Text; DELETE setzt `persona.md` zurück |

Beispiele:

```bash
curl -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"text":"Hallo, ich heiße Veit"}' https://sprachassistent.plenivo.de/v1/chat
curl -H "Authorization: Bearer $KEY" --data-binary @frage.wav \
  "https://sprachassistent.plenivo.de/v1/talk?format=wav" -o antwort.wav
curl -X PUT -H "Authorization: Bearer $KEY" --data-binary @meine-persona.md \
  https://sprachassistent.plenivo.de/v1/persona
```

Nach jedem Gespräch fragt der Server das Modell, ob sich Fakten über den Nutzer geändert haben (`MEMORY_EXTRACTION`). Diese Fakten und die letzten `HISTORY_MESSAGES` Nachrichten gehen in jeden Prompt.

## Lokal

Tests ohne Sprachmodelle (keine C++-Abhängigkeiten):

```bash
cargo test --no-default-features
```

Ohne lokale Rust-Installation geht das auch über Docker:

```bash
docker run --rm -v "$PWD:/src" -w /src rust:1-bookworm cargo test --no-default-features
```

Server mit Modellen: `.env.example` nach `.env` kopieren, `bash deployments/pi/download-models.sh`, dann `cargo run --release` (unter Linux; braucht cmake und clang für whisper.cpp).

## Build für den Pi

```bash
docker build -t sprachassistent-build deployments/build
docker run --rm -v "$PWD:/src" sprachassistent-build    # → bin/sprachassistent, bin/lib/*.so (aarch64)
```

Der Build kompiliert unter Debian bookworm mit dem aarch64-Cross-Compiler. Die Binärdatei braucht deshalb höchstens die glibc von bookworm. whisper.cpp wird aus dem Quellcode gebaut. sherpa-onnx gibt es für aarch64 nur als vorgebaute Shared Library; sie liegt mit onnxruntime in `bin/lib/`, und die Binärdatei findet sie über `rpath $ORIGIN/lib`.

## Betrieb auf dem Pi

Einmalig als root (legt `.env` mit `API_KEY` an, lädt die Modelle, setzt den deSEC-CNAME `sprachassistent.plenivo.de → dev.plenivo.de`, holt das Zertifikat und richtet nginx ein):

```bash
DEPLOY_PUBKEY="ssh-ed25519 AAAA... github-actions-sprachassistent" bash deployments/pi/bootstrap.sh
```

Danach in `/root/sprachassistent/.env` `GROQ_API_KEY` und `GEMINI_API_KEY` eintragen (kostenlos bei console.groq.com und aistudio.google.com).

Im GitHub-Environment `production` müssen `SSH_HOST`, `SSH_USER`, `SSH_PRIVATE_KEY` und `DEPLOY_PATH` gesetzt sein. Ein Push auf `deploy` baut die Binärdatei und kopiert sie mit den Deploy-Skripten auf den Pi. `runtime-reload.sh` startet dann pm2 (`sprachassistent-api`) neu.

## Branches

- `development`: Entwicklung, CI testet und baut
- `deploy`: Produktion auf dem Pi

## ESP32

Verkabelung (ESP32-WROOM-32E):

| INMP441 | ESP32 | | MAX98357A | ESP32 |
|---|---|---|---|---|
| VDD | 3.3V | | VIN | 5V |
| GND | GND | | GND | GND |
| L/R | GND | | BCLK | GPIO 26 |
| SCK | GPIO 32 | | LRC | GPIO 25 |
| WS | GPIO 33 | | DIN | GPIO 27 |
| SD | GPIO 34 | | SD, GAIN | offen |

Taster zwischen GPIO 13 und GND. Die Onboard-LED (GPIO 2) leuchtet während der Aufnahme. Blinkt sie dreimal, kam keine Antwort; fünfmal heißt, es gab keine Verbindung.

`firmware/include/secrets.example.h` nach `secrets.h` kopieren (WLAN, `API_KEY`, optional das Let's-Encrypt-Root-Zertifikat), dann:

```bash
cd firmware && pio run -t upload && pio device monitor
```

Ist das Mikrofon stumm, in `firmware/src/main.cpp` `I2S_CHANNEL_FMT_ONLY_LEFT` gegen `ONLY_RIGHT` tauschen. Ist es zu leise oder übersteuert, `MIC_SHIFT` anpassen.
