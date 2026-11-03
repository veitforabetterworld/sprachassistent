# Lizenzen

- **Dieses Projekt:** AGPL-3.0-or-later (`LICENSE`), ohne Gewährleistung und Haftung (Abschnitte 15 und 16), mit einer zusätzlichen Erlaubnis nach Abschnitt 7 für die Kombination mit veit-framework (siehe README).
- **veit-framework:** PolyForm Noncommercial 1.0.0. Web-API und Adminbereich bauen darauf auf, deshalb ist der Sprachassistent insgesamt nur nicht kommerziell nutzbar. Der Rust-Sprachserver (mit espeak-ng, GPL-3.0) nutzt das Framework nicht.
- **Trainierte Aktivierungswörter** (`firmware/wakewords/*.h`): CC BY-NC-SA 4.0 (`firmware/wakewords/LICENSE`), weil die Trainingsdaten nur nicht kommerzielle Nutzung erlauben (siehe unten).

## Abhängigkeiten

Stand 2026-11-03. Keine Rechtsberatung. Nichts davon liegt im Repo, außer dem trainierten Modell `firmware/wakewords/giraffe.h`; alles andere wird beim Bauen oder Starten heruntergeladen.

### Code

| Teil | Lizenzen | Anmerkung |
|---|---|---|
| Rust-Crates (343) | MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, CDLA-Permissive-2.0 | alle permissiv |
| sherpa-onnx | Apache-2.0 | **die vorgebaute Bibliothek enthält espeak-ng (GPL-3.0-or-later)** für die Aussprache der Piper-Stimmen |
| onnxruntime | MIT | |
| Node-Pakete (254) | MIT, ISC, BSD, Apache-2.0, 0BSD, MIT-0 | lightningcss (MPL-2.0) und caniuse-lite (CC-BY-4.0) nur beim Bauen |
| veit-framework | **PolyForm Noncommercial 1.0.0** | eigenes Projekt, nur nicht kommerziell |
| ESP-IDF, esp-tflite-micro, esp-nn, esp-micro-speech-features | Apache-2.0 | |
| Arduino-Core für ESP32 | LGPL-2.1 | |
| Training: microWakeWord, TensorFlow, datasets | Apache-2.0 | |
| Training: piper-tts | **GPL-3.0-or-later** | von `make_tts.py` importiert |
| Training: edge-tts | LGPL-3.0 | inoffizieller Zugang zur Microsoft-Sprachausgabe, deren Nutzungsbedingungen gelten |
| SearXNG, Caddy, Postgres, Chromium | AGPL-3.0, Apache-2.0, PostgreSQL, BSD u. a. | eigenständige Programme, unverändert |

### Modelle und Daten

| Teil | Lizenz | Verwendet für |
|---|---|---|
| Kroko-ASR Community (Spracherkennung) | CC-BY-SA (laut Banafo; kommerzielle Nutzung nicht eindeutig) | Server |
| Piper thorsten, thorsten_emotional, kerstin | CC0 | Server, Training |
| Piper karlsson, eva_k, ramona (M-AILABS) | eigene M-AILABS-Lizenz, frei nutzbar für Sprach-Training | Server, Training |
| Piper pavoque | **CC-BY-NC-SA-4.0** | Server, Training |
| Piper mls (Multilingual LibriSpeech) | CC-BY-4.0 | Training |
| Edge TTS (Microsoft) | Microsoft-Nutzungsbedingungen | Training |
| Negativ-Features von microWakeWord | **CC-BY-NC-4.0** (Quellen u. a. LibriSpeech, VOiCES, FSD50K, WHAM, CHiME-6, DiPCo) | Training |
| FMA xsmall | MIT laut Mirror, Stücke einzeln unter CC-Lizenzen | Training (Hintergrund) |
| MIT-Raumimpulsantworten | CC-BY-4.0 laut Mirrors | Training (Hall) |

### Folgen

- **Trainierte Wake-Word-Modelle** (`firmware/wakewords/*.h`) stammen unter anderem aus CC-BY-NC-Daten, aus der NC-Stimme pavoque und aus Edge TTS. Sie sind deshalb nur nicht kommerziell nutzbar.
- **GPL-3.0:** espeak-ng ist in der Bibliothek von sherpa-onnx fest eingebaut, piper-tts wird beim Training importiert. Die AGPL-3.0 ist mit der GPL-3.0 vereinbar; wer Programm und Bibliothek zusammen weitergibt (Binärdatei, Docker-Image), hält sich an beide.
- **Namensnennung:** Kroko, MLS, M-AILABS, MIT-Raumimpulsantworten und die microWakeWord-Daten verlangen sie.
