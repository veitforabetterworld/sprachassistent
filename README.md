# Sprachassistent

Sprachassistent mit Persönlichkeit und Gedächtnis. ESP32 dienen als Mikrofon und Lautsprecher, die Arbeit macht ein Server im Heimnetz (Raspberry Pi 5 oder PC). Jeder ESP ist ein eigenes Gerät mit eigener Stimme, Persönlichkeit und eigenem Gedächtnis; alles wird im Adminbereich eingestellt. Gestartet wird per Aktivierungswort („Giraffe“, lokal auf dem ESP erkannt) oder Taster.

```
ESP32 (INMP441, MAX98357A, Taster, optional Motor)
  │  POST /v1/talk  (16 kHz PCM, chunked, ESP-Schlüssel) → Rahmen: Audio und Status für die LED
  ▼
Reverse Proxy (Caddy im Docker-Setup, nginx auf dem Pi)
  ├─ /v1/talk, /v1/health ─ :3020  Rust-Sprachserver (axum)
  │     ├─ Spracherkennung  Kroko, live beim Sprechen (sherpa-onnx)  lokal
  │     ├─ KI               Anbieter aus der DB, mit Fallback      extern, nur Text
  │     └─ Sprachausgabe    Piper-Stimmen (sherpa-onnx), je Gerät   lokal
  ├─ /v1/*            ─ :3022  Web-API (Node, veit-framework)
  └─ /                ─ Adminbereich (React, veit-framework)
Postgres: Einstellungen, Anbieter, Geräte, Gedächtnis je Gerät, Konten und Schlüssel
SearXNG: Internetrecherche, selbstgehostet
```

Nur Text verlässt das Heimnetz. Laufende Kosten: Strom und die Gratis-Kontingente von Groq oder Gemini. Die Spracherkennung nutzt das deutsche Streaming-Modell von [Kroko (Banafo)](https://huggingface.co/Banafo/Kroko-ASR) unter CC-BY-SA.

## 1. ESP verdrahten

![Anschlussplan: ESP32-WROOM-32E mit INMP441, MAX98357A, Taster, Lautsprecher und Motor](docs/anschlussplan.svg)

Teile: ESP32-DevKit (WROOM-32E), INMP441, MAX98357A, Lautsprecher (4–8 Ω, 2–3 W), Taster. Optional ein 5-V-Motor über einen Logic-Level-MOSFET an GPIO 4; er dreht, solange der Server arbeitet. Löte nach der Beschriftung auf den Platinen, die Pin-Reihenfolge kann vom Plan abweichen. Den Taster (GPIO 5) beim Einschalten nicht gedrückt halten.

## 2. Sprachassistent hosten

Auf einem Linux-Rechner oder Pi 5 mit Docker:

```bash
git clone --recursive https://github.com/veitforabetterworld/sprachassistent.git
cd sprachassistent
printf 'POSTGRES_PASSWORD=%s\nCREDENTIALS_KEY=%s\n' "$(openssl rand -hex 16)" "$(openssl rand -hex 32)" > .env
docker compose up -d --build
```

Der erste Start baut alles und lädt die Sprachmodelle (~500 MB), das dauert eine Weile. Danach:

1. `http://<rechner>` öffnen und registrieren; das erste Konto wird Admin.
2. **KI-Modelle:** einen Anbieter eintragen, z. B. Groq oder Gemini (Gratis-Key), und mit dem Test-Button prüfen.
3. **Chatbots:** einen Chatbot anlegen (Name, Stimme, Persönlichkeit).
4. **API-Schlüssel:** einen Schlüssel für den Chatbot erzeugen; er kommt in die Firmware.

Für HTTPS von außen `SITE=assistent.example.de` in die `.env` schreiben (Ports 80 und 443 müssen erreichbar sein, das Zertifikat holt Caddy selbst). Ist Port 80 belegt: `HTTP_PORT=8080`.

## 3. Firmware flashen

[PlatformIO](https://platformio.org/install/cli) installieren, dann:

```bash
cd firmware
cp include/secrets.example.h include/secrets.h   # WLAN, Server-Adresse, API-Schlüssel eintragen
pio run -t upload && pio device monitor
```

Das flasht die Firmware mit Aktivierungswort „Giraffe“ und Taster. Weitere Envs: `-e taster` (nur Taster), `-e hwtest` (Mikrofon, Lautsprecher, Taster und Motor ohne WLAN testen).

LED: grün = Aufnahme nach dem Aktivierungswort, rot = Aufnahme per Taster, blau pulsierend = denkt, violett = spricht. Blinkt sie 5× rot: keine Verbindung; 3× orange: API-Schlüssel abgelehnt; 3× rot: Serverfehler. Ist das Mikrofon stumm, in `firmware/src/main.cpp` `I2S_CHANNEL_FMT_ONLY_LEFT` gegen `ONLY_RIGHT` tauschen.

## 4. Aktivierungswort trainieren

Läuft in Docker auf dem PC (CPU reicht, ≥ 12 GB RAM, ~40 GB frei, gut 2 Stunden beim ersten Mal).

1. Das Wort in [training/words.py](training/words.py) eintragen: Schreibweise und ähnlich klingende Wörter, die nicht auslösen sollen.
2. Optional, aber deutlich besser: echte Stimmen mit genau diesem Mikrofon aufnehmen (~14 min pro Person):
   ```bash
   cd firmware && pio run -e collect -t upload && cd ../training
   python -m venv .venv && .venv/bin/pip install -r requirements.txt   # Windows: .venv\Scripts\...
   .venv/bin/python record.py session --person anna --words hallo
   .venv/bin/python record.py ambient --name wohnzimmer --minutes 5
   ```
3. Trainieren und für die Firmware exportieren:
   ```bash
   cd training
   docker compose run --rm train python train.py --word hallo
   ```
   Das erzeugt synthetische Stimmen (Piper, Edge TTS), lädt einmalig ~6 GB Fremddaten, trainiert, zeigt eine Tabelle mit Treffern und Fehlauslösungen je Schwelle und schreibt `firmware/wakewords/hallo.h` samt Env `[env:hallo]`.
4. Flashen: `cd firmware && pio run -e hallo -t upload`

Löst es zu oft aus, die Schwelle anheben (ohne neu zu trainieren): `docker compose run --rm train python train.py --word hallo --from export --cutoff 0.98`. Überhört es zu viel, senken. Details und Messwerte: [docs/wakeword.md](docs/wakeword.md).

## Entwicklung

### Struktur

```
src/                 Rust-Sprachserver
  api.rs             /v1/talk, /v1/health und interne Routen für den Adminbereich
  store.rs           Postgres: Geräte, Gedächtnis, Anbieter, ESP-Schlüssel prüfen
  assistant.rs       Prompt und Persönlichkeit je Gerät, Regeln nur für angebotene Werkzeuge
  context.rs         Gedächtnis: Kontext (500 Zeichen), den das Modell nach der Antwort selbst neu schreibt
  speech.rs          Live-Spracherkennung (Kroko) und Piper-Stimmen über sherpa-onnx (Feature speech)
  llm.rs             OpenAI-kompatible Anbieter mit Fallback, Streaming
  tools.rs           Werkzeuge: Internetrecherche über SearXNG, MCP-Werkzeuge
  web.rs             Seiten lesen (follow): nur öffentliche https-Adressen, Links mit Titel
  page.rs            HTML als knapper Text ohne Kopf, Menüs und Linklisten
  browser.rs         Headless-Chromium für JavaScript-Seiten, Proxy nur zu öffentlichen Adressen
  sentences.rs       Antwort-Stream in Sätze zerlegen
db/migrations/       Nummerierte SQL-Migrationen, angewandt von apps/api
apps/api/            Web-API: @veit/auth-server, @veit/api-keys, Admin-Routen
apps/web/            Adminbereich: @veit/react-auth, -account, -api-keys, -controls
veit-framework/      Submodule (Pin origin/development)
firmware/            ESP32 (PlatformIO, Arduino); wakewords/ enthält die Modelle
training/            Wake-Word-Training (Aufnahme, TTS, microWakeWord in Docker)
compose.yaml         Self-Hosting; Images aus deployments/docker/
deployments/build/   Cross-Compile für den Pi (Docker, Debian bookworm → aarch64)
deployments/pi/      Pi-Betrieb ohne Docker: Bootstrap, Runtime-Reload, nginx, pm2, Modell-Download
```

### Konfiguration

In `.env` stehen nur Datenbank und `CREDENTIALS_KEY` (Vorlage `.env.example`); alles andere liegt in der Datenbank und wird im Adminbereich gepflegt (Chatbots, API-Schlüssel mit der Berechtigung `device.talk`, Werkzeuge, KI-Modelle, Einstellungen, Benutzer). Das erste Konto wird Admin; danach ist die Registrierung geschlossen.

Die Internetrecherche (`web_research`, Aktionen `news` und `search`) fragt nur das selbstgehostete SearXNG unter `SEARXNG_URL` (JSON-Format an, `deployments/pi/searxng/settings.yml`). Ist es nicht erreichbar, meldet das Werkzeug einen Fehler.

Der Agent erkennt Werkzeuge nie am Namen. Jedes eingeschaltete Werkzeug eines Chatbots geht an das Modell. Wie er ein Werkzeug behandelt, folgt allein aus den MCP-Annotationen: `readOnlyHint: true` liest (mit `openWorldHint: true` von außen, darf länger dauern und mehrere Schritte machen), `readOnlyHint: false` ändert etwas und wartet, solange es ein Lesewerkzeug gibt, auf einen Blick auf den Stand; ohne Annotation wird nichts angenommen. Eine längere Anleitung zu einem Werkzeug (`ToolOffer::guide`) steht im Systemprompt, solange es angeboten wird.

`follow` liest bis zu drei Seiten und hängt die Links samt Titel an. Liefert eine Seite kaum Text, lädt sie ein Headless-Chromium (`BROWSER_BIN`, sonst `/usr/bin/chromium`). Chromium läuft nie als root, sondern per `setpriv` als `sprachassistent-browser`, ohne Bilder, und erreicht das Netz nur über einen Proxy im Sprachserver, der ausschließlich https zu öffentlichen Adressen durchlässt (Pi: `deployments/pi/browser.sh`; Docker: im Image, die Sandbox braucht `deployments/docker/seccomp-chromium.json`).

### API für die ESPs

| Methode | Pfad | |
|---|---|---|
| GET | `/v1/health` | Datenbank, Spracherkennung, installierte Stimmen |
| POST | `/v1/talk` | Audio rein (rohes s16le 16 kHz oder WAV), gesprochene Antwort raus. `Authorization: Bearer spa_…`. `?format=wav` für WAV, `?device=<id>` wenn der Schlüssel mehrere Geräte abdeckt. Header `X-Sample-Rate`, `X-Transcript` |

```bash
curl -H "Authorization: Bearer $ESP_KEY" --data-binary @frage.wav "http://<rechner>/v1/talk?format=wav" -o antwort.wav
```

### Lokal ohne Docker

```bash
git submodule update --init --recursive
pnpm install && pnpm build:deps
docker run -d --name sa-pg -e POSTGRES_PASSWORD=sprachassistent -e POSTGRES_USER=sprachassistent -p 5432:5432 postgres:17
cp .env.example .env
pnpm dev:api        # migriert die Datenbank, dann Web-API :3022
pnpm dev:web        # Adminbereich :5183 (Proxy auf :3020 und :3022)
docker run -d --name sa-searxng -p 127.0.0.1:8888:8080 -e SEARXNG_SECRET=lokal \
  -v "$PWD/deployments/pi/searxng/settings.yml:/etc/searxng/settings.yml:ro" searxng/searxng:2026.10.27-b5ed73fcb
bash deployments/pi/download-models.sh
cargo run           # Sprachserver :3020 (Linux; ohne Modelle: --no-default-features)
```

### Datenbank

Das Schema entsteht aus `db/migrations/NNNN_name.sql`. Jede Datei läuft genau einmal, in Namensreihenfolge, und wird in `schema_migrations` vermerkt. Migriert wird nur an einer Stelle: `pnpm --filter @sprachassistent/api migrate` (Docker: beim Start von `web`, Pi: in `runtime-reload.sh`, lokal: `pnpm dev:api`). Fehlt eine Migration, starten Web-API und Sprachserver nicht und nennen sie.

Eine Änderung bekommt immer eine neue Datei, eine angewandte wird nicht mehr bearbeitet. Bis beide Server neu gestartet sind, läuft kurz noch der alte Code gegen das neue Schema: Spalten erst hinzufügen, später in einer eigenen Migration entfernen.

### Tests

Ohne `TEST_DATABASE_URL` werden die API-Tests übersprungen. Sie legen pro Lauf die Datenbank `sa_api_tests` frisch an; jeder Test arbeitet mit eigenen Nutzern und Chatbots, so laufen sie parallel.

```bash
docker run -d --name sa-test-pg -p 127.0.0.1:55432:5432 -e POSTGRES_USER=t -e POSTGRES_DB=t -e POSTGRES_HOST_AUTH_METHOD=trust postgres:17-alpine
TEST_DATABASE_URL=postgres://t@127.0.0.1:55432/t cargo test --no-default-features
pnpm typecheck
```

### Code-Standards

Der Pre-Commit-Hook (`pnpm install` richtet ihn ein) und die CI brechen bei Verstößen ab.

- **Nur Framework-Bausteine:** keine nativen `button`/`input`/`select`/`textarea`, kein `style=`, keine eigenen CSS-Dateien, keine Arbitrary Values und Palettenfarben in Tailwind-Klassen (`veit-framework/scripts/no-custom-css.mjs`). Radix und `VeitDialog` nur über die Veit-Presets, keine `alert`/`confirm` (ESLint).
- **Bearbeiten im Popup:** `VeitOverlayEditDialog` mit Draft (`useDialogFormBaseline`), Löschen über `deleteAction` im Footer. Klickbare Zeilen sind `VeitListItem`; Dialoge bleiben eingebunden und schalten nur `open`.
- **Texte:** alles Sichtbare kommt aus `apps/web/src/i18n` (de und en mit gleichen Schlüsseln), Datum über `@veit/intl`.
- **Seiten unter 1000 Zeilen** (`check-page-loc.mjs`).
- `pnpm lint`, `pnpm check:secrets`, `pnpm check:submodules`, `pnpm check:audit`; Rust: `cargo fmt --check` und `cargo clippy -- -D warnings`.

### Pi-Betrieb ohne Docker (sprachassistent.plenivo.de)

Einmalig als root: `DEPLOY_PUBKEY="ssh-ed25519 …" bash deployments/pi/bootstrap.sh` (Postgres, `.env`, Modelle, SearXNG, Chromium, deSEC-CNAME, Zertifikat, nginx). Ein Push auf `deploy` baut per GitHub Actions (`deployments/build/`, Cross-Compile für den Cortex-A76 des Pi 5) und kopiert alles auf den Pi; `runtime-reload.sh` migriert und startet pm2 neu. Im GitHub-Environment `production`: `SSH_HOST`, `SSH_USER`, `SSH_PRIVATE_KEY`, `DEPLOY_PATH`.

Branches: `development` (CI testet und baut), `deploy` (Produktion).

## Lizenz

Code: [AGPL-3.0-or-later](LICENSE). Wer den Sprachassistenten verändert und anderen anbietet, auch als Dienst übers Netz, muss den geänderten Quelltext offenlegen. Zusätzliche Erlaubnis nach Abschnitt 7 der AGPL: Dieses Programm darf mit [veit-framework](https://github.com/veitforabetterworld/veit-framework) kombiniert und zusammen weitergegeben werden, obwohl das Framework unter [PolyForm Noncommercial 1.0.0](veit-framework/LICENSE) steht; für das Framework gelten dessen Bedingungen. Da Web-API und Adminbereich darauf aufbauen, ist der Sprachassistent insgesamt nur nicht kommerziell nutzbar.

Trainierte Aktivierungswörter in `firmware/wakewords/`: [CC BY-NC-SA 4.0](firmware/wakewords/LICENSE), weil ihre Trainingsdaten es so verlangen. Keine Gewährleistung, keine Haftung. Lizenzen der Abhängigkeiten, Modelle und Daten: [docs/lizenzen.md](docs/lizenzen.md).
