# Aktivierungswort auf dem ESP32

Die Erkennung läuft lokal auf dem ESP; im Leerlauf geht kein Audio ins WLAN. Nach dem Wort nimmt der ESP auf, bis 0,8 s Stille folgen (höchstens 20 s), und schickt nur diese Aufnahme an den Server. Der Taster funktioniert weiter als Push-to-Talk. Anleitung zum Trainieren: [README](../README.md#4-aktivierungswort-trainieren).

## Aufbau

- **Modell:** microWakeWord (Streaming-CNN „mixednet“ wie die offiziellen v2-Modelle, int8), ein Modell pro Wort. Die Firmware bekommt genau eins: Env mit `custom_wakeword = <wort>`, Modell aus `firmware/wakewords/<wort>.h`.
- **Laufzeit:** TensorFlow Lite Micro 1.3.3 mit esp-nn 1.1.2 und das Frontend esp-micro-speech-features 1.2.3, dieselben Versionen wie ESPHome. `firmware/tools/wakeword.py` holt sie beim ersten Build nach `firmware/lib/`. Frontend: 16 kHz, 30-ms-Fenster, 10 ms Vorschub, 40 Bänder; Inferenz alle 30 ms (Stride 3).
- **Auslösen:** gleitender Mittelwert über 5 Inferenzen über der Schwelle im Header, danach 0,9 s Sperre.
- **Speicher:** Während Aufnahme, Anfrage und Wiedergabe gibt die Firmware Arena und Interpreter frei, sonst reicht der Heap für TLS nicht. Danach wird das Modell neu geladen (~20 ms); bis die Antwort verklungen ist plus 0,5 s hört es nicht zu.
- **Stille-Erkennung:** Sprache ist ein 16-ms-Block mit RMS über max(4 × Grundrauschen, 200). Das Grundrauschen fällt schnell und steigt nur langsam (~30 s) und wird während der Wiedergabe nicht nachgeführt; sonst schaukeln Aktivierungswort und Lautsprecher es hoch.
- **Flash:** Die Wake-Word-Firmware braucht ~1,16 MB und nutzt deshalb die Partition `min_spiffs.csv` (1,9 MB pro App).

Gemessen auf ESP32-D0WD-V3 ohne PSRAM (Arduino 2.0.17): Inferenz 7 ms alle 30 ms, mit Frontend ~31 % eines Kerns; Arena 21,6 KB; Heap-Minimum während TLS mit geladenem Modell nur 13,7 KB, daher das Entladen.

## Training (`training/`)

| Schritt | Skript | |
|---|---|---|
| Aufnahmen | `record.py` + Env `collect` | Mikrofon per USB (921600 Baud) an den PC; geführte Sitzung mit Wort, Sprechweisen, ähnlichen Wörtern, Sätzen, Stille; Raumgeräusche |
| Synthetische Stimmen | `make_tts.py` | Alle deutschen Piper-Stimmen (inkl. 236 MLS-Sprecher) und 10 Edge-TTS-Stimmen (de-DE/AT/CH), Tempo und Tonlage variiert; Lautschrift aus `words.py`, wo Piper falsch betont |
| Fremddaten | `fetch_aug_data.py` | Negativ-Features von microWakeWord (englische Sprache, Partys, Geräusche), FMA-Musik, MIT-Raumimpulsantworten |
| Features | `prepare_features.py` | Zuschneiden, Augmentation (Hall, Hintergrund −5…15 dB SNR, EQ), Spektrogramme. Eigene Aufnahmen werden je Clip fest in Training und Test geteilt |
| Training | `train_wakeword.py` | 20 000 Schritte in zwei Phasen, Fehlauslösungen in Phase 2 härter bestraft; ~1 h auf einem Ryzen 5 5600G |
| Test | `eval_clips.py` | Rechnet wie der ESP auf den eigenen Aufnahmen: erkannte Test-Rufe, ähnliche Wörter, Sätze, Raumgeräusche je Schwelle |
| Export | `firmware_model.py` | `firmware/wakewords/<wort>.h` und `[env:<wort>]` |

Die ~28 GB Features liegen in Docker-Volumes, weil das Training zufällig daraus liest; über einen Windows-Ordner wartet es fast nur auf I/O. Die Partygeräusche für die Auswertung sind auf 1 h (Validierung) bzw. 2 h (Test) gekürzt, sonst reichen 7 GB RAM nicht.

## Stand „Giraffe“

1813 synthetische Clips, 38 eigene Aufnahmen einer Person, 5 min Raumgeräusch. Schwelle 0,97:

| Test | Ergebnis |
|---|---|
| Eigene Rufe, nicht im Training | 15/18 erkannt (verpasst: 2× langsam, 1× aus der Ferne) |
| Echt aufgenommene ähnliche Wörter | 0/33 |
| Normale Sätze | 1/56 löst aus |
| Raumgeräusch | 0 Fehlauslösungen |
| Live am Gerät | 10/10 erkannt (0,98–1,00), keine Fehlauslösung |

Schwachstelle: Deutsche Alltagssprache fehlt als Negativbeispiel (die Fremddaten sind englisch). Nächster Schritt: Common Voice Deutsch einbinden, Sätze ohne das Wort als Negative, mit dem Wort per Forced Alignment als zusätzliche echte Sprecher. Mehr Personen aufnehmen hilft am meisten gegen überhörte Rufe.

## Verworfene Alternativen

| Alternative | Grund |
|---|---|
| WakeNet (Espressif) | Eigene Wörter trainiert nur Espressif gegen Gebühr; „Computer“ gibt es nur für ESP32-S3/P4. |
| openWakeWord auf dem Server | Der ESP müsste dauerhaft 32 KB/s ins WLAN streamen. |
| Porcupine, Edge Impulse | Kein ESP32-Build bzw. Trainingsdaten in fremder Cloud. |
| Vorlagenvergleich (MFCC + DTW) | Funktioniert fast nur für einen Sprecher. |
