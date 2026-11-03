# Plan: Wake Words „Giraffe“ und „Computer“ auf dem ESP32

Branch `feature/wakeword`. Ziel: Die Aufnahme startet, wenn man „Giraffe“ oder „Computer“ sagt; der Taster funktioniert weiter wie heute. Die Erkennung läuft lokal auf dem ESP, im Leerlauf geht kein Audio ins WLAN.

## Gemessen am angeschlossenen Board (temporärer Spectest, inzwischen entfernt)

| | |
|---|---|
| Chip | ESP32-D0WD-V3, Rev. 3.1, 2 Kerne, 240 MHz, keine SIMD-Befehle |
| Flash | 4 MB, 40 MHz DIO; App-Partition je 1,25 MB (OTA-Layout), aktuelle Firmware ~750 KB |
| PSRAM | **keins** (`PSRAM ID read error`) |
| Core | Arduino 2.0.17 / ESP-IDF 4.4.7 |
| Internes RAM frei | 203 KB nach Boot, 194 KB mit I2S-Mikrofon, **144 KB mit WLAN** (größter Block 108 KB) |
| Rechenleistung, ein Kern | int16 MAC 23,8 Mio/s, float MAC 18,3 Mio/s, FFT 512 Punkte 0,66 ms |

Die Hauptfirmware belegt davon noch 64 KB Audio-Puffer (`AUDIO_BUFFER_BYTES`) und während einer Anfrage den TLS-Client (gemessen ~45 KB, siehe Phase 0).

Trainingsrechner (Dev-PC): Ryzen 5 5600G (6 Kerne/12 Threads), 14 GB RAM, keine NVIDIA-GPU, Docker 29 mit WSL2, D: 580 GB frei. Das Training läuft auf der CPU, für ein Modell dieser Größe dauert das Stunden, nicht Tage.

## Entscheidungen

- **Bestehendes Board, Erkennung lokal auf dem ESP.**
- **Eigenes Modell statt WakeNet.** WakeNet-Wörter trainiert nur Espressif, gegen Gebühr. Wir trainieren selbst auf dem Dev-PC und lassen das Modell mit TensorFlow Lite Micro laufen. Der Arduino-Core 2.0 bleibt. Als Vorlage dient die Trainings-Pipeline von microWakeWord (Streaming-Modell, Augmentation, Feature-Frontend).
- **Wörter: „Giraffe“ und „Computer“, ein Wort pro Gerät.** Für jedes Wort gibt es ein eigenes Modell. Beim Bauen der Firmware wird per Build-Flag festgelegt, welches Modell mitkommt; es landet also nur eins im Flash. Schwellen werden pro Wort eingestellt. Eine Einstellung im Adminbereich gibt es nicht.
- **„Computer“ nur mit deutscher Aussprache.** Englisch gesprochene Clips des Worts sind weder positive noch negative Beispiele. Die Aussprachen liegen so nah beieinander, dass das Modell sonst auch beim deutschen Wort unsicher würde. Englische Sätze kommen als allgemeine Negativ-Sprache dazu. Ein einzeln englisch gesagtes „computer“ kann deshalb trotzdem auslösen.
- **Sprecher:** Sehr gemischt nach Alter, Geschlecht und Dialekt. Die Trainingsdaten müssen das abdecken.
- **Umgebung:** Kaum Musik oder Fernsehen. Das Mikrofon ist aber nicht gut, Rauschen und Frequenzgang des INMP441 müssen deshalb in die Daten.
- **Abwägung:** Lieber eine Fehlauslösung als ein überhörtes Wort, aber nicht zu viele. Wir suchen den besten Kompromiss (siehe Phase 5).
- **Repo:** Nur Skripte und das fertige Modell. Audiodaten, Features und heruntergeladene Datensätze kommen in `.gitignore`.
- **Aufnahme-Ende:** Nach ~800 ms Stille endet die Aufnahme, nach 20 s spätestens (wie `MAX_RECORD_MS`). Der Taster bleibt Push-to-Talk.
- **Rückmeldung:** nur die LED-Farbe, kein Ton.

## Datenquellen

### Positive Beispiele („Giraffe“, „Computer“)

| Quelle | Was | Kosten / Bedingungen |
|---|---|---|
| **Eigene Aufnahmen** über genau dieses INMP441 | alle Personen im Haushalt, 50–100 Wiederholungen je Person: verschiedene Abstände, Lautstärken, Tonlagen | – |
| **Common Voice Deutsch** (Mozilla, v25: 1486 h, 20 466 Sprecher, CC0) | Echte Menschen mit Alter, Geschlecht und Akzent in den Metadaten. Wir suchen Sätze, die „Giraffe“ oder „Computer“ enthalten, und schneiden das Wort per Forced Alignment heraus. Wie viele Treffer es gibt, zeigt sich erst beim Durchsuchen. | kostenlos; Download über Mozilla Data Collective, **dafür musst du dich anmelden und die Bedingungen akzeptieren** |
| **Piper, lokal** | deutsche Stimmen: eva_k, karlsson, kerstin, mls (mehrere Sprecher), pavoque, ramona, thorsten, thorsten_emotional | kostenlos, offline |
| **Coqui XTTS-v2, lokal** | Stimmen klonen: Mit Common-Voice-Sprechern als Referenz entstehen tausende verschiedene Stimmfarben, auch mit Dialekt-Färbung. Auf der CPU langsam (Sekunden pro Clip), also über Nacht laufen lassen. | kostenlos, nur nicht-kommerzielle Lizenz (für uns passend) |
| **Edge TTS** (Microsoft, über `edge-tts`) | etwa 10 deutsche neuronale Stimmen, darunter de-AT und de-CH | kostenlos, inoffizieller Zugang, kann jederzeit wegfallen |
| **Azure Speech F0** | dieselben Stimmen wie Edge TTS, offiziell, mit SSML für Tempo, Tonhöhe und Betonung | kostenloses Kontingent; Azure-Konto nötig, **musst du anlegen** |
| **Google Cloud TTS** | weitere deutsche Stimmen (Standard/WaveNet/Neural2) | monatlich kostenloses Kontingent (Standard ~4 Mio., WaveNet/Neural2 ~1 Mio. Zeichen); Konto mit Abrechnungsdaten nötig, **musst du anlegen** |

Pro Clip sind es nur etwa 10 Zeichen. Selbst zehntausende Varianten bleiben deshalb weit unter den kostenlosen Kontingenten.

### Negative Beispiele (alles, was nicht auslösen darf)
- **Ähnlich klingende Wörter**, von allen TTS-Quellen erzeugt:
  - Giraffe: Karaffe, Giraffen, Gitarre, Grafik, Affe, Paraffin, Gerade, Garage
  - Computer: Kompott, kompakt, Komponist, Computerspiel, Kommune, Puter, Kompass
- **Gesprochenes Deutsch** aus Common Voice ohne die beiden Wörter, dazu englische Sprache aus den vorberechneten Daten von microWakeWord (`speech`, `dinner_party`).
- **Geräusche:** AudioSet und FMA (wie bei microWakeWord), die vorberechneten Daten `no_speech` von microWakeWord und eigene Langaufnahmen aus dem Raum mit dem INMP441 (Stille, Küche, Gespräche).
- **Eigene Sprachausgabe** des Geräts, über den MAX98357A abgespielt und mit dem INMP441 aufgenommen.

### Augmentation
- Raumhall mit den MIT-Raumimpulsantworten (wie bei microWakeWord) und Hintergrundgeräusche aus den oben genannten Quellen.
- **Mikrofon-Simulation:** Aus eigenen Aufnahmen werden Grundrauschen und Frequenzgang des INMP441 gemessen. Beides legen wir auf alle TTS- und Common-Voice-Clips.
- Tempo und Tonhöhe ±10 %, Lautstärke, verschiedene Abstände (Pegel plus Hallanteil).

## Verworfene Alternativen

| Alternative | Grund |
|---|---|
| WakeNet10s / WakeNet9s (Espressif) | Für den normalen ESP32 gibt es nur fertige Wörter (Hi ESP, Alexa, Hi 乐鑫 …). Ein eigenes Wort trainiert nur Espressif gegen Gebühr. |
| WakeNet9 `wn9_computer_tts` | Ein fertiges „Computer“-Modell, laut ESP-SR-Kconfig aber nur für ESP32-S3/P4/S31. |
| Board-Tausch auf ESP32-S3 mit PSRAM | Wir bleiben beim vorhandenen Board. |
| openWakeWord auf dem Pi (es gibt fertige „computer“-Modelle) | Der ESP müsste dauerhaft 32 KB/s ins WLAN streamen. |
| Porcupine | Kein Build für den ESP32. |
| Edge Impulse | Die Trainingsdaten gingen in deren Cloud. |
| Vorlagenvergleich (MFCC + DTW) | Funktioniert fast nur für einen Sprecher und ist störanfällig. |
| NodeMCU mit CH340 | Entweder ein ESP8266 (zu schwach) oder derselbe ESP32 wie jetzt. |

## Technischer Rahmen

- **Frontend:** 16 kHz, 30-ms-Fenster, 10 ms Vorschub, 40 Mel-Bänder. Wir nehmen das TFLM-Micro-Frontend wie bei microWakeWord, damit Training und ESP exakt dieselben Features rechnen. Gemessen: 0,75 ms pro Frame, ~7,5 % eines Kerns.
- **Modell:** Streaming-CNN, int8-quantisiert, ein Modell pro Wort. Budget nach Phase 0: Tensor-Arena ≤ 30 KB, Modell ≤ 100 KB im Flash, Inferenz ≤ 8 ms bei Stride 3.
- **Laufzeit:** Die Erkennung läuft nur im Leerlauf, nicht während Aufnahme, Anfrage und Wiedergabe. Damit konkurriert sie nicht mit TLS um RAM und löst nicht auf die eigene Sprachausgabe aus. Nach der Wiedergabe gibt es eine Sperrzeit von ~500 ms.
- **Auslösen:** Gleitender Mittelwert über mehrere Frames, Schwelle pro Modell.
- **Aufnahme-Start:** Nur was nach dem Wake Word gesagt wird, geht an den Server.

## Phasen

### Phase 0 – Machbarkeit (Spike)
1. TFLite-Micro-Bibliothek mit esp-nn für Arduino 2.0.17 auswählen und ein microWakeWord-Beispielmodell (z. B. `okay_nabu`) auf dem ESP laufen lassen. Gemessen werden Flash, Arena und Laufzeit pro Frame.
2. Den freien Heap mit echter TLS-Verbindung zum Server messen.
3. Ergebnis: Das Modellbudget steht fest. Notfalls wird `AUDIO_BUFFER_BYTES` verkleinert.

**Ergebnis (erledigt; der Spike-Code `wwspike.cpp` ist inzwischen entfernt):**

| | |
|---|---|
| Bibliotheken | esp-tflite-micro 1.3.3 + esp-nn 1.1.2, also dieselben Versionen wie ESPHome. `firmware/tools/fetch_wakeword_libs.py` holt sie nach `firmware/lib/` (nicht im Repo). Das Frontend ist esp-micro-speech-features 1.2.3 (`lib_deps`). Damit es mit Arduino 2.0.17 baut, braucht es `-std=gnu++17` und für TFLM `-fno-exceptions -fno-rtti`. |
| Testmodell | `okay_nabu` (microWakeWord v2), 60 KB Flash, Eingang 3 × 40 Features, Ausgang uint8 |
| Tensor-Arena | 18,4 KB genutzt (26 KB reserviert laut Manifest) + 1 KB Variablen-Arena; `AllocateTensors` 17 ms |
| Frontend | ~11 KB RAM, 0,75 ms pro 10-ms-Frame |
| Inferenz | 7,0 ms (max. 9,4 ms) alle 30 ms; zusammen mit Frontend **31 % eines Kerns** (Kern 1) |
| Erkennung | Am INMP441 mit echter Stimme: 9 Treffer bei Wahrscheinlichkeit 0,97–0,99; im Leerlauf und bei normalem Sprechen max. 0,02 |
| Heap mit Modell + 64 KB Audio-Puffer + WLAN | 71 KB frei |
| Heap während TLS-Verbindung | **26 KB frei, Minimum 13,7 KB**, größter Block 18 KB. TLS belegt ~45 KB. |
| Flash (Spike inkl. WLAN + TLS) | **1,14 MB von 1,31 MB** App-Partition (87 %) |

Folgerungen:
- **Rechenzeit:** passt. Unsere Modelle sollten nicht größer als `okay_nabu` werden; das entspricht ~25 % eines Kerns für die Inferenz.
- **RAM:** Mit Modell und TLS gleichzeitig wird es eng (13,7 KB Minimum). Die Firmware gibt deshalb während Aufnahme, Anfrage und Wiedergabe Arena und Interpreter frei (~27 KB) und lädt sie danach neu (17 ms). Das passt ohnehin zu „Erkennung nur im Leerlauf“. `AUDIO_BUFFER_BYTES` bleibt bei 64 KB.
- **Flash:** Die Hauptfirmware allein braucht schon 924 KB (70,5 %). Mit TFLM und Modell (~300 KB mehr) passt sie nicht mehr in die Standard-Partition. Die Firmware nutzt weder OTA noch SPIFFS. Wir wechseln auf `board_build.partitions = min_spiffs.csv` (1,9 MB pro App, OTA bleibt möglich).
- **Budget für unsere Modelle:** Arena ≤ 30 KB, Modell ≤ 100 KB, Inferenz ≤ 8 ms bei Stride 3.

### Phase 1 – Aufnahme-Werkzeug
1. Firmware-Env `collect`: Das Mikrofon streamt rohes PCM über USB-Seriell mit 921600 Baud an den PC, ohne WLAN.
2. `training/record.py` führt jede Person durch eine Sitzung: Zuerst wird nach Name bzw. Kürzel, Altersgruppe und Geschlecht gefragt. Dann kommen „Giraffe“ und „Computer“ (deutsch, nah und fern, laut und leise), ähnliche Wörter und ein paar freie Sätze. Die Clips werden automatisch geschnitten und landen in `training/data/real/<person>/<klasse>/`.
3. Langaufnahmen: Raumgeräusche und die eigene Lautsprecherausgabe.
4. Ein Teil der Personen und Aufnahmen wird nur zum Testen genutzt, nie zum Trainieren.

**Ergebnis (erledigt):**
- `firmware/src/collect.cpp`, Env `collect`: Liest das Mikrofon genau wie `main.cpp` (`MIC_SHIFT` 14, 16 kHz) und schickt es in Paketen mit Folgenummer per USB (921600 Baud). Gleichzeitig kann es Audio vom PC über den Lautsprecher abspielen. Die LED leuchtet grün bei Aufnahme und blau bei Wiedergabe.
- `training/esp_audio.py`: das Protokoll auf der PC-Seite. `training/record.py` bietet vier Befehle:
  - `test`: Pegel, verlorene Pakete und Lautsprecher-Rückkanal prüfen.
  - `session --person <kürzel>`: Fragt einmalig Altersgruppe, Geschlecht und Dialekt ab (`people.json`). Dann folgen 123 Aufforderungen in zufälliger Reihenfolge, etwa 14 Minuten. Jede Aufforderung steht 2 s vorher als „Gleich: …“ da, dann kommt „JETZT“ mit 3 s Zeit (Sätze 6–7 s). `--zeit 1.5` macht alles langsamer:
    - je Wort 7 Sprechweisen × 4
    - 4 Sätze mit dem Wort vorneweg (für eine realistische Auswertung)
    - je ähnliches Wort 2×
    - 10 normale Sätze und 3× Stille
    - zum Schluss 2 Sprechweisen × 4 aus 2–3 m bzw. weggedreht

    Ist nichts zu hören, das Wort zu lang oder die Aufnahme übersteuert, wird automatisch wiederholt. Die Tasten p/r/q pausieren, wiederholen die letzte Aufnahme oder beenden. Gespeichert wird das zugeschnittene Wort (±250 ms) plus die Rohaufnahme, die Metadaten landen in `clips.jsonl`. Mit `--words giraffe` nur ein Wort.
  - `ambient --name <ort> --minutes N`: Raumgeräusche in 1-Minuten-Dateien.
  - `speaker --dir <wavs>`: WAV-Dateien über den Lautsprecher abspielen und gleichzeitig aufnehmen, für die eigene Sprachausgabe. Die Lautstärke ist standardmäßig 250 % wie in `main.cpp`.
- Gemessen am Gerät: 5,07 s Audio in 5 s ohne Paketverlust, Grundrauschen −67 dBFS (Median), ein 1-kHz-Ton über den Lautsprecher kommt 78 dB über dem Rest an.
- Einrichtung: `cd training`, `python -m venv .venv`, `.venv\Scripts\pip install -r requirements.txt`, dann `.venv\Scripts\python record.py test`.

### Phase 2 – Daten beschaffen und erzeugen
1. Downloads: MIT-Raumimpulsantworten, AudioSet-Ausschnitt, FMA, die Negativ-Features von microWakeWord, Piper-Stimmen, Common Voice Deutsch (von dir heruntergeladen).
2. Common Voice: Sätze mit „Giraffe“ und „Computer“ heraussuchen, das Wort per Forced Alignment ausschneiden, den Rest als negative Sprache verwenden.
3. TTS-Erzeugung: Piper, XTTS-v2, Edge TTS, Azure und Google, jeweils für Zielwörter und ähnliche Wörter, mit Variation über SSML bzw. Parameter. Zeichen werden mitgezählt, damit wir sicher im kostenlosen Kontingent bleiben.
4. Augmentation inklusive Mikrofon-Simulation, dann Feature-Extraktion.

### Phase 3 – Training
1. Streaming-Modell nach dem Vorbild von microWakeWord, ein Training pro Wort, in Docker auf der CPU. Das andere Wort zählt dabei jeweils als Negativbeispiel.
2. Export: int8-TFLite → `firmware/src/wakeword_giraffe.h` und `firmware/src/wakeword_computer.h`.
3. Auswertung je Wort:
   - Trefferquote auf den zurückgehaltenen echten Aufnahmen, getrennt nach Person und Altersgruppe
   - Fehlauslösungen pro Stunde auf deutscher Sprache und den Raumaufnahmen
   - Kurve beider Werte über alle Schwellen

### Phase 4 – Firmware-Integration
1. Modul `wakeword.cpp/.h`: Frontend, Inferenz und Glättung, auf Kern 1 neben dem Mikrofon-Task.
2. Build-Flag `WAKEWORD` (`giraffe` oder `computer`), z. B. als eigenes Env pro Wort in `platformio.ini`. Die Schwelle steht neben dem Modell im Header und lässt sich per Flag überschreiben.
3. Zustände: Leerlauf (hört auf Wake Word) → Aufnahme → Anfrage/Wiedergabe → Sperrzeit → Leerlauf.
4. Stille-Erkennung zum Beenden der Aufnahme.
5. Neue LED-Farbe „erkannt / hört zu“.
6. Debug-Ausgabe der Erkennungswerte über Seriell.

**Ergebnis Phase 2–4 für „Giraffe“ (erster Stand, noch ohne Common Voice):**
- Alles läuft im Docker-Image `wakeword-train` (`training/Dockerfile`). `features/` und `negative_datasets/` (~28 GB) liegen in den Docker-Volumes `ww-features` und `ww-negatives`, weil das Training über den Windows-Bind-Mount fast nur auf I/O wartet. Aufruf siehe `training/train_wakeword.py`.
- `fetch_aug_data.py`: Negativ-Features von microWakeWord, MIT-Raumimpulsantworten, FMA-Musik. Die Partygeräusche für die Auswertung werden gekürzt (Validierung 1 h, Test 2 h), sonst reichen 7 GB RAM nicht.
- `make_tts.py`: 1813 Clips „Giraffe“ (Piper 1213 mit vorgegebener Lautschrift „gi-RAF-fe“/„schi-RAF-fe“, Edge TTS 600) und 897 Clips ähnlicher Wörter.
- `prepare_features.py` → `train_wakeword.py` (mixednet wie okay_nabu, 20 000 Schritte, ~55 min auf dem Ryzen 5 5600G). Modellauswahl nur nach `average_viable_recall`; mit „Fehlauslösungen bei 0,5 minimieren“ gewann ein fast untrainiertes Modell.
- `eval_clips.py` (Schwelle 0,97, eigene Aufnahmen, nicht im Training): 15/18 Rufe erkannt, 0/33 echte ähnliche Wörter, 0 Fehlauslösungen im Raumgeräusch, aber 1/56 normale Sätze („Ruf bitte Papa an.“). Deutsche Alltagssprache fehlt als Negativbeispiel, das soll Common Voice liefern.
- Firmware: Env `giraffe`, Modell 62 KB, Arena 21,6 KB. Live: 8/8 Rufe erkannt (0,98–1,00), keine Fehlauslösung. Die Aufnahme endet 0,8 s nach dem letzten Wort; das Grundrauschen steigt dafür nur langsam und wird während der Wiedergabe nicht nachgeführt.

### Phase 5 – Schwelle finden und Aufräumen
1. Startpunkt: höchste Trefferquote bei höchstens ~1 Fehlauslösung pro 24 h auf den Langaufnahmen.
2. Dann ein paar Tage im echten Betrieb mit allen Personen. Fehlauslösungen und überhörte Wörter zählen (Debug-Log), danach nachjustieren. Die Aufnahmen aus dem Betrieb wandern als neue Trainingsdaten in die nächste Runde.
3. README aktualisieren (Spectest und Spike sind schon entfernt).

## Was du beisteuern musst

1. **Common Voice Deutsch:** Auf Mozilla Data Collective anmelden, die Bedingungen akzeptieren, das Archiv herunterladen (mehrere zehn GB) und mir den Pfad nennen, z. B. `D:\datasets\commonvoice-de`.
2. **Optional Azure- und Google-Konten** für mehr Stimmen. Beide verlangen bei der Anmeldung Zahlungsdaten, auch für das kostenlose Kontingent. Die API-Schlüssel kommen in eine `.env`, die nicht ins Repo geht. Ohne diese Konten nutzen wir Piper, XTTS und Edge TTS.
3. **Aufnahmesitzungen** mit allen Personen, je etwa 10–15 Minuten, sobald Phase 1 steht.

## Offene Fragen an dich

Keine. Alle Entscheidungen für Phase 0 bis 2 stehen fest.

Risiko beim „Computer“-Modell: Die vorberechnete englische Sprache von microWakeWord hat keine Transkripte und kann das englische „computer“ enthalten. Diese Stellen würden dann als negativ gelernt. Falls die Trefferquote beim deutschen Wort darunter leidet, nehmen wir diese Daten für das „Computer“-Modell heraus.
