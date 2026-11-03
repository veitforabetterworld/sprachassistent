"""Erzeugt synthetische Aufnahmen der Wake Words und der aehnlich klingenden Woerter.

Laeuft im Trainings-Container (siehe Dockerfile):

    python make_tts.py --word giraffe              # Piper (lokal) und Edge TTS
    python make_tts.py --word giraffe --only piper

Woerter, Varianten und Lautschrift stehen in words.py.

Ergebnis: data/tts/<wort>/<quelle>/*.wav und data/tts/aehnlich_<wort>/<quelle>/*.wav, 16 kHz mono.
"""

import argparse
import asyncio
import json
import random
import subprocess
import urllib.request
import wave
import zlib
from pathlib import Path

import numpy as np
from scipy.signal import resample_poly

from words import SIMILAR, WORDS

ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data"
VOICES = DATA / "piper_voices"
RATE = 16000

# Satzzeichen aendern die Betonung
PUNCTUATION = ["", "!", "?", ".", ","]
TEXTS = {key: [t + p for t in [w["text"], *w.get("variants", [])] for p in PUNCTUATION] for key, w in WORDS.items()}
PHONEMES = {text: [list(p) for p in variants] for w in WORDS.values() for text, variants in w.get("phonemes", {}).items()}
# Einzelne Woerter, die laenger klingen, sind kaputt (das MLS-Modell plappert gelegentlich weiter)
MAX_SPEECH_SECONDS = 1.3

HF_TREE = "https://huggingface.co/api/models/rhasspy/piper-voices/tree/main/de/de_DE"
HF_FILE = "https://huggingface.co/rhasspy/piper-voices/resolve/main/"


def write_wav(path: Path, samples: np.ndarray):
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(np.clip(samples, -32768, 32767).astype("<i2").tobytes())


def to_16k(samples: np.ndarray, rate: int) -> np.ndarray:
    if rate == RATE:
        return samples.astype(np.float32)
    g = np.gcd(rate, RATE)
    return resample_poly(samples.astype(np.float32), RATE // g, rate // g)


def speech_seconds(x: np.ndarray) -> float:
    """Dauer, in der der Pegel ueber -30 dB relativ zur lautesten Stelle liegt (10-ms-Rahmen)."""
    frame = RATE // 100
    n = len(x) // frame
    if n == 0:
        return 0.0
    rms = np.sqrt((x[: n * frame].astype(np.float32).reshape(n, frame) ** 2).mean(axis=1))
    return float((rms > rms.max() * 0.03).sum()) / 100


def fetch_piper_voices() -> list[Path]:
    """Laedt alle deutschen Piper-Stimmen (jeweils beste Qualitaet je Sprecher)."""
    VOICES.mkdir(parents=True, exist_ok=True)
    found = []
    names = json.load(urllib.request.urlopen(HF_TREE))
    for speaker in [e["path"] for e in names if e["type"] == "directory"]:
        qualities = json.load(urllib.request.urlopen(f"{HF_TREE.rsplit('/de/de_DE', 1)[0]}/{speaker}"))
        best = sorted((e["path"] for e in qualities if e["type"] == "directory"),
                      key=lambda p: ["x_low", "low", "medium", "high"].index(p.rsplit("/", 1)[1]))[-1]
        files = json.load(urllib.request.urlopen(f"{HF_TREE.rsplit('/de/de_DE', 1)[0]}/{best}"))
        for f in files:
            if f["path"].endswith((".onnx", ".onnx.json")):
                target = VOICES / Path(f["path"]).name
                if not target.exists():
                    print("lade", f["path"])
                    urllib.request.urlretrieve(HF_FILE + f["path"], target)
        found += [VOICES / Path(f["path"]).name for f in files if f["path"].endswith(".onnx")]
    return found


def piper(texts: list[str], out: Path, per_speaker: int, seed: int):
    from piper import PiperVoice, SynthesisConfig

    rng = random.Random(seed)
    for model in fetch_piper_voices():
        voice = PiperVoice.load(str(model))
        speakers = voice.config.num_speakers
        # Einzelsprecher bekommen mehr Varianten, damit sie neben den 236 MLS-Sprechern nicht untergehen
        n = per_speaker * speakers if speakers > 1 else per_speaker * 12
        print(f"{model.stem}: {speakers} Sprecher, {n} Clips")
        rejected = 0
        for i in range(n):
            target = out / f"{model.stem}_{i:05d}.wav"
            if target.exists():
                continue
            for _ in range(5):
                cfg = SynthesisConfig(
                    speaker_id=(i % speakers) if speakers > 1 else None,
                    length_scale=rng.uniform(0.8, 1.3),
                    noise_scale=rng.uniform(0.5, 0.8),
                    noise_w_scale=rng.uniform(0.5, 0.9),
                )
                text = rng.choice(texts)
                word, punct = text.rstrip("!?.,"), text[len(text.rstrip("!?.,")) :]
                if word in PHONEMES:
                    ids = voice.phonemes_to_ids(rng.choice(PHONEMES[word]) + list(punct))
                    audio = np.asarray(voice.phoneme_ids_to_audio(ids, syn_config=cfg))
                    if audio.dtype != np.int16:
                        audio = np.clip(audio * 32767, -32768, 32767)
                else:
                    chunks = list(voice.synthesize(text, syn_config=cfg))
                    audio = np.concatenate([c.audio_int16_array for c in chunks])
                audio = to_16k(audio, voice.config.sample_rate)
                if speech_seconds(audio) <= MAX_SPEECH_SECONDS:
                    write_wav(target, audio)
                    break
                rejected += 1
        if rejected:
            print(f"  {rejected} zu lange Clips verworfen und neu erzeugt")


async def edge(texts: list[str], out: Path, per_voice: int, seed: int):
    import edge_tts

    rng = random.Random(seed)
    voices = [v["ShortName"] for v in await edge_tts.list_voices() if v["Locale"].startswith("de-")]
    print(f"Edge TTS: {len(voices)} deutsche Stimmen: {', '.join(voices)}")
    for voice in voices:
        for i in range(per_voice):
            target = out / f"{voice}_{i:04d}.wav"
            if target.exists():
                continue
            rate = f"{rng.randint(-35, 30):+d}%"
            pitch = f"{rng.randint(-15, 15):+d}Hz"
            mp3 = target.with_suffix(".mp3")
            mp3.parent.mkdir(parents=True, exist_ok=True)
            for attempt in range(3):
                try:
                    await edge_tts.Communicate(rng.choice(texts), voice, rate=rate, pitch=pitch).save(str(mp3))
                    break
                except Exception as e:  # Dienst drosselt gelegentlich
                    print(f"  {voice}: {e}, neuer Versuch")
                    await asyncio.sleep(5 * (attempt + 1))
            else:
                continue
            subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(mp3), "-ac", "1", "-ar", str(RATE), str(target)], check=True)
            mp3.unlink()
        print(f"  {voice} fertig")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--word", required=True, choices=list(TEXTS))
    parser.add_argument("--only", choices=["piper", "edge"])
    parser.add_argument("--piper-per-speaker", type=int, default=4)
    parser.add_argument("--edge-per-voice", type=int, default=60)
    parser.add_argument("--similar", type=int, default=1, help="Faktor fuer die aehnlichen Woerter (0 = keine)")
    args = parser.parse_args()

    jobs = [(TEXTS[args.word], DATA / "tts" / args.word, 1.0)]
    if args.similar and SIMILAR[args.word]:
        jobs.append(([w for w in SIMILAR[args.word]], DATA / "tts" / f"aehnlich_{args.word}", 0.5 * args.similar))

    for texts, out, factor in jobs:
        if args.only in (None, "piper"):
            piper(texts, out / "piper", max(1, round(args.piper_per_speaker * factor)), seed=zlib.crc32(out.name.encode()))
        if args.only in (None, "edge"):
            asyncio.run(edge(texts, out / "edge", max(1, round(args.edge_per_voice * factor)), seed=zlib.crc32(out.name.encode())))


if __name__ == "__main__":
    main()
