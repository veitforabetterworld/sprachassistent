"""Aufnahmen fuer das Wake-Word-Training mit genau dem Mikrofon des Geraets.

Vorher die Aufnahme-Firmware flashen:  cd firmware && pio run -e collect -t upload

    python record.py test                       # Pegel, Rauschen, Lautsprecher pruefen
    python record.py session --person veit      # gefuehrte Sitzung (~10 min pro Person)
    python record.py ambient --name kueche --minutes 30
    python record.py speaker --dir pfad/zu/wavs # eigene Sprachausgabe ueber den Lautsprecher aufnehmen

Alles landet in training/data/real/ (nicht im Repo).
"""

import argparse
import json
import random
import sys
import time
import wave
from datetime import datetime
from pathlib import Path

import numpy as np

from esp_audio import RATE, EspAudio
from words import SIMILAR, WAKE_WORDS

DATA = Path(__file__).resolve().parent / "data" / "real"

# Ausgesprochen wie im Alltag; "fern" und "seitlich" kommen gesammelt am Ende
STYLES = [
    ("normal", "ganz normal"),
    ("leise", "leise"),
    ("laut", "laut"),
    ("schnell", "schnell"),
    ("langsam", "langsam, gedehnt"),
    ("frage", "fragend"),
    ("muede", "nebenbei, gelangweilt"),
]
FAR_STYLES = [
    ("fern", "aus 2 bis 3 Metern Entfernung"),
    ("seitlich", "vom Geraet weggedreht"),
]
REPEATS_PER_STYLE = 4

SIMILAR_REPEATS = 2

SENTENCES = [
    "Wie wird das Wetter morgen?",
    "Mach bitte das Licht im Wohnzimmer an.",
    "Ich habe heute keine Lust zu kochen.",
    "Hast du den Schluessel irgendwo gesehen?",
    "Wir fahren am Wochenende zu Oma.",
    "Kannst du mir mal kurz helfen?",
    "Das Essen ist gleich fertig.",
    "Wo ist denn die Fernbedienung schon wieder?",
    "Morgen frueh muss ich um sieben aufstehen.",
    "Erzaehl mir einen Witz.",
    "Stell einen Timer auf zehn Minuten.",
    "Die Katze will raus.",
    "Ich geh mal kurz einkaufen.",
    "Wie viel Uhr ist es eigentlich?",
    "Spiel mal was Ruhiges.",
    "Heute war ein langer Tag.",
    "Wer hat die letzte Milch getrunken?",
    "Das Paket kommt wahrscheinlich morgen.",
    "Ruf bitte Papa an.",
    "Wann faengt der Film an?",
]
SENTENCE_COUNT = 10

SILENCE_COUNT = 3

WINDOW = {"wake": 3.0, "satz": 6.0, "similar": 3.0, "sentence": 7.0, "silence": 3.0}
PREVIEW = 2.0  # so lange steht die naechste Aufforderung da, bevor die Aufnahme laeuft
PAUSE = 1.0  # nach jeder Aufnahme
PRE_ROLL = 0.3
MAX_WORD_SECONDS = 1.6
MIN_SPEECH_SECONDS = 0.15
PAD = 0.25


def write_wav(path: Path, samples: np.ndarray):
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(samples.astype("<i2").tobytes())


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path), "rb") as w:
        rate, channels, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
        data = w.readframes(w.getnframes())
    if width != 2:
        raise ValueError(f"{path}: nur 16-Bit-WAV")
    x = np.frombuffer(data, dtype="<i2").astype(np.float32)
    if channels > 1:
        x = x.reshape(-1, channels).mean(axis=1)
    if rate != RATE:
        n = int(len(x) * RATE / rate)
        x = np.interp(np.linspace(0, len(x) - 1, n), np.arange(len(x)), x)
    return x.astype(np.int16)


def dbfs(x: float) -> float:
    return 20 * np.log10(max(x, 1e-9) / 32768)


def frame_rms(x: np.ndarray, frame: int = RATE // 100) -> np.ndarray:
    n = len(x) // frame
    f = x[: n * frame].astype(np.float32).reshape(n, frame)
    return np.sqrt((f**2).mean(axis=1))


def analyse(x: np.ndarray) -> dict:
    """Findet den gesprochenen Teil ueber die Energie pro 10 ms."""
    rms = frame_rms(x)
    if len(rms) == 0:
        return {"speech": None}
    noise = float(np.percentile(rms, 20))
    threshold = max(noise * 4, 120.0)
    active = np.flatnonzero(rms > threshold)
    clipped = int((np.abs(x.astype(np.int32)) >= 32000).sum())
    info = {"noise_dbfs": round(dbfs(noise), 1), "peak_dbfs": round(dbfs(float(np.abs(x).max())), 1), "clipped": clipped}
    if len(active) == 0:
        info["speech"] = None
        return info
    # Luecken unter 150 ms zusammenfassen, dann den groessten Block nehmen
    blocks, start, prev = [], active[0], active[0]
    for i in active[1:]:
        if i - prev > 15:
            blocks.append((start, prev))
            start = i
        prev = i
    blocks.append((start, prev))
    first, last = blocks[0][0], blocks[-1][1]
    speech_rms = float(rms[first : last + 1].mean())
    info.update(
        speech=(first * RATE // 100, (last + 1) * RATE // 100),
        speech_seconds=round((last + 1 - first) / 100, 2),
        snr_db=round(dbfs(speech_rms) - dbfs(noise), 1),
    )
    return info


def trim(x: np.ndarray, speech: tuple[int, int]) -> np.ndarray:
    pad = int(PAD * RATE)
    return x[max(0, speech[0] - pad) : min(len(x), speech[1] + pad)]


def key_pressed() -> str | None:
    try:
        import msvcrt
    except ImportError:
        return None
    if msvcrt.kbhit():
        return msvcrt.getwch().lower()
    return None


def load_people() -> dict:
    p = DATA / "people.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}


def ask_person(person: str) -> dict:
    people = load_people()
    if person in people:
        print(f"Bekannte Person {person}: {people[person]}")
        return people[person]
    print(f"Neue Person '{person}'. Ein paar Angaben fuer die Auswertung (Enter = keine Angabe):")
    info = {
        "alter": input("  Altersgruppe (kind / jugend / erwachsen / senior): ").strip().lower(),
        "geschlecht": input("  Geschlecht (w / m / d): ").strip().lower(),
        "dialekt": input("  Dialekt oder Region (z. B. bairisch, schwaebisch, hochdeutsch): ").strip(),
    }
    people[person] = info
    DATA.mkdir(parents=True, exist_ok=True)
    (DATA / "people.json").write_text(json.dumps(people, indent=2, ensure_ascii=False), encoding="utf-8")
    return info


def build_prompts(words: list[str]) -> list[dict]:
    near, far = [], []
    for w in words:
        for style, hint in STYLES:
            near += [{"kind": "wake", "cls": w, "text": WAKE_WORDS[w], "style": style, "hint": hint}] * REPEATS_PER_STYLE
        for style, hint in FAR_STYLES:
            far += [{"kind": "wake", "cls": w, "text": WAKE_WORDS[w], "style": style, "hint": hint}] * REPEATS_PER_STYLE
        for _ in range(REPEATS_PER_STYLE):
            near.append(
                {
                    "kind": "satz",
                    "cls": f"{w}_satz",
                    "text": f"{WAKE_WORDS[w]}, {random.choice(SENTENCES)}",
                    "style": "satz",
                    "hint": "so, als wuerdest du das Geraet benutzen",
                }
            )
        for s in SIMILAR[w]:
            near += [{"kind": "similar", "cls": f"aehnlich/{s.lower()}", "text": s, "style": "normal", "hint": "normal"}] * SIMILAR_REPEATS
    for s in random.sample(SENTENCES, SENTENCE_COUNT):
        near.append({"kind": "sentence", "cls": "saetze", "text": s, "style": "normal", "hint": "normal vorlesen"})
    random.shuffle(near)
    random.shuffle(far)
    silence = [{"kind": "silence", "cls": "stille", "text": "(nichts sagen)", "style": "stille", "hint": "ruhig bleiben"}] * SILENCE_COUNT
    return silence[:1] + near + [{"kind": "move"}] + far + silence[1:]


def session(args):
    person = args.person.strip().lower()
    meta = ask_person(person)
    words = args.words.split(",") if args.words else list(WAKE_WORDS)
    prompts = build_prompts(words)
    total = sum(1 for p in prompts if p.get("kind") != "move")
    pdir = DATA / person
    log = pdir / "clips.jsonl"
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")

    esp = EspAudio(args.port)
    seconds = sum(PREVIEW + PRE_ROLL + WINDOW[p["kind"]] + PAUSE for p in prompts if p["kind"] != "move") * args.zeit
    print(f"\n{total} Aufnahmen, etwa {seconds / 60:.0f} Minuten. Etwa 30-50 cm vor das Geraet setzen.")
    print("Waehrend der Sitzung: p = Pause, r = letzte Aufnahme wiederholen, q = beenden.")
    input("Enter druecken zum Starten ...")

    done, index, last = 0, 0, None
    queue = list(prompts)
    try:
        while queue:
            prompt = queue.pop(0)
            if prompt["kind"] == "move":
                input("\n>>> Jetzt bitte 2-3 m weggehen bzw. dich vom Geraet wegdrehen. Enter, wenn bereit ...")
                continue
            k = key_pressed()
            if k == "q":
                break
            if k == "p":
                input("Pause. Enter zum Weitermachen ...")
            if k == "r" and last is not None:
                for f in last["files"]:
                    Path(f).unlink(missing_ok=True)
                queue.insert(0, prompt)
                queue.insert(0, last["prompt"])
                done -= 1
                last = None
                print("   letzte Aufnahme verworfen, nochmal")
                continue

            print(f"\n[{done + 1}/{total}] Gleich: {prompt['text']}   ({prompt['hint']})", flush=True)
            time.sleep(PREVIEW * args.zeit)
            esp.start()
            time.sleep(PRE_ROLL)
            print(f"   >>> JETZT: {prompt['text']} <<<", flush=True)
            time.sleep(WINDOW[prompt["kind"]] * args.zeit)
            x = esp.stop()
            print("   (Aufnahme fertig)", flush=True)

            info = analyse(x)
            ok, reason = True, ""
            if prompt["kind"] == "silence":
                clip = x
            elif info["speech"] is None or info["speech_seconds"] < MIN_SPEECH_SECONDS:
                ok, reason = False, "nichts gehoert"
            elif info["speech"][1] > len(x) - RATE // 20:
                ok, reason = False, "bis zum Ende gesprochen, evtl. abgeschnitten - etwas frueher anfangen"
            elif prompt["kind"] in ("wake", "similar") and info["speech_seconds"] > MAX_WORD_SECONDS:
                ok, reason = False, f"zu lang ({info['speech_seconds']} s), nur das Wort sagen"
            elif info["clipped"] > len(x) // 100:
                ok, reason = False, "uebersteuert, etwas leiser oder weiter weg"
            if not ok:
                print(f"   x {reason} - nochmal")
                queue.insert(0, prompt)
                continue

            index += 1
            name = f"{person}_{stamp}_{index:03d}_{prompt['style']}.wav"
            files = []
            if prompt["kind"] in ("wake", "similar"):
                clip = trim(x, info["speech"])
            elif prompt["kind"] in ("satz", "sentence"):
                clip = x
            out = pdir / prompt["cls"] / name
            write_wav(out, clip)
            files.append(str(out))
            raw = pdir / "_raw" / prompt["cls"] / name
            write_wav(raw, x)
            files.append(str(raw))
            entry = {
                "file": str(out.relative_to(DATA)).replace("\\", "/"),
                "raw": str(raw.relative_to(DATA)).replace("\\", "/"),
                "person": person,
                **meta,
                "kind": prompt["kind"],
                "class": prompt["cls"],
                "text": prompt["text"],
                "style": prompt["style"],
                "time": datetime.now().isoformat(timespec="seconds"),
                **{k: v for k, v in info.items() if k != "speech"},
            }
            with log.open("a", encoding="utf-8") as f:
                f.write(json.dumps(entry, ensure_ascii=False) + "\n")
            last = {"files": files, "prompt": prompt}
            done += 1
            if prompt["kind"] == "silence":
                print(f"   ok  Rauschen {info['noise_dbfs']} dBFS")
            else:
                print(f"   ok  {info['speech_seconds']} s, Spitze {info['peak_dbfs']} dBFS, Abstand zum Rauschen {info['snr_db']} dB")
            time.sleep(PAUSE * args.zeit)
    except KeyboardInterrupt:
        print("\nabgebrochen")
    finally:
        esp.close()
    print(f"\n{done} Aufnahmen gespeichert in {pdir}")
    if esp.dropped_packets:
        print(f"Warnung: {esp.dropped_packets} Pakete verloren (USB-Kabel/Port pruefen)")


def ambient(args):
    esp = EspAudio(args.port)
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    out_dir = DATA / "_ambient"
    print(f"Nehme {args.minutes} Minuten auf ({args.name}). Ctrl+C beendet frueher.")
    try:
        for minute in range(int(args.minutes)):
            x = esp.record(60)
            write_wav(out_dir / f"{args.name}_{stamp}_{minute:03d}.wav", x)
            rms = frame_rms(x)
            print(f"  Minute {minute + 1}: Median {dbfs(float(np.median(rms))):.1f} dBFS, Spitze {dbfs(float(np.abs(x).max())):.1f} dBFS")
    except KeyboardInterrupt:
        x = esp.stop()
        if len(x) > RATE:
            write_wav(out_dir / f"{args.name}_{stamp}_rest.wav", x)
    finally:
        esp.close()
    print(f"Gespeichert in {out_dir}")


def speaker(args):
    files = sorted(Path(args.dir).glob("*.wav"))
    if not files:
        sys.exit(f"Keine WAV-Dateien in {args.dir}")
    esp = EspAudio(args.port)
    esp.set_volume(args.volume)
    out_dir = DATA / "_speaker"
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    try:
        for i, f in enumerate(files):
            audio = read_wav(f)
            esp.start()
            esp.play(audio)
            time.sleep(0.5)  # Nachhall und Puffer leerspielen
            x = esp.stop()
            write_wav(out_dir / f"{stamp}_{i:04d}_{f.stem}.wav", x)
            print(f"  {f.name}: {len(audio) / RATE:.1f} s abgespielt, Spitze am Mikrofon {dbfs(float(np.abs(x).max())):.1f} dBFS")
    finally:
        esp.close()
    print(f"Gespeichert in {out_dir}")


def test(args):
    esp = EspAudio(args.port)
    try:
        print("5 s Ruhe aufnehmen ...")
        x = esp.record(5)
        rms = frame_rms(x)
        print(f"  {len(x) / RATE:.2f} s empfangen (erwartet ~5 s), {esp.dropped_packets} Pakete verloren")
        print(f"  Rauschen: Median {dbfs(float(np.median(rms))):.1f} dBFS, Spitze {dbfs(float(np.abs(x).max())):.1f} dBFS")
        print("Ton ueber den Lautsprecher (1 kHz, 1 s) ...")
        t = np.arange(RATE) / RATE
        tone = (np.sin(2 * np.pi * 1000 * t) * 6000).astype(np.int16)
        esp.set_volume(100)
        esp.start()
        time.sleep(0.3)
        esp.play(tone)
        time.sleep(0.6)
        y = esp.stop()
        spec = np.abs(np.fft.rfft(y.astype(np.float32)))
        freqs = np.fft.rfftfreq(len(y), 1 / RATE)
        band = spec[(freqs > 950) & (freqs < 1050)].max()
        rest = np.median(spec[(freqs > 200) & (freqs < 7000)])
        print(f"  Ton am Mikrofon: {20 * np.log10(band / max(rest, 1e-9)):.0f} dB ueber dem Rest, Spitze {dbfs(float(np.abs(y).max())):.1f} dBFS")
        print("Sprich jetzt 3 s lang ...")
        z = esp.record(3)
        info = analyse(z)
        print(f"  Sprache: {info.get('speech_seconds')} s, Spitze {info['peak_dbfs']} dBFS, Abstand zum Rauschen {info.get('snr_db')} dB, {info['clipped']} Samples uebersteuert")
        if esp.messages:
            print("  ESP meldet:", " | ".join(esp.messages))
    finally:
        esp.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", default="COM10")
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("test")
    p = sub.add_parser("session")
    p.add_argument("--person", required=True, help="Kuerzel, z. B. veit")
    p.add_argument("--words", help="z. B. giraffe oder giraffe,computer (Standard: beide)")
    p.add_argument("--zeit", type=float, default=1.0, help="Faktor fuer alle Wartezeiten, z. B. 1.5 = langsamer")
    p = sub.add_parser("ambient")
    p.add_argument("--name", required=True, help="z. B. kueche, wohnzimmer_abends")
    p.add_argument("--minutes", type=float, default=10)
    p = sub.add_parser("speaker")
    p.add_argument("--dir", required=True)
    p.add_argument("--volume", type=int, default=250, help="Prozent, wie VOLUME_PERCENT in main.cpp")
    args = parser.parse_args()
    {"test": test, "session": session, "ambient": ambient, "speaker": speaker}[args.cmd](args)


if __name__ == "__main__":
    main()
