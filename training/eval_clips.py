"""Testet das fertige Streaming-Modell auf den eigenen Aufnahmen, so wie der ESP rechnet
(Gleitmittel ueber WINDOW Inferenzen, dann Schwelle).

    python eval_clips.py --word giraffe

Zeigt fuer verschiedene Schwellen: Anteil erkannter Test-Aufnahmen (nicht im Training), Ausloesungen
bei den aehnlichen Woertern und Saetzen, und Fehlausloesungen in den Raumaufnahmen.
"""

import argparse
import wave
from pathlib import Path

import numpy as np

from microwakeword.inference import Model
from words import WAKE_WORDS

DATA = Path(__file__).resolve().parent / "data"
WINDOW = 5
CUTOFFS = [0.5, 0.6, 0.7, 0.8, 0.85, 0.9, 0.95, 0.97]


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path), "rb") as w:
        return np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")


def peak(model: Model, x: np.ndarray) -> float:
    """Hoechstes Gleitmittel; vorne und hinten Stille, damit der Zustand wie im Betrieb einschwingt."""
    pad = np.zeros(16000, dtype=np.int16)
    probs = np.array(model.predict_clip(np.concatenate([pad, x, pad]), step_ms=10))
    if len(probs) < WINDOW:
        return 0.0
    return float(np.convolve(probs, np.ones(WINDOW) / WINDOW, mode="valid").max())


def triggers(model: Model, x: np.ndarray, cutoff: float) -> int:
    """Ausloesungen in einer langen Aufnahme, mit 30 Inferenzen Sperrzeit wie in der Firmware."""
    probs = np.array(model.predict_clip(x, step_ms=10))
    mean = np.convolve(probs, np.ones(WINDOW) / WINDOW, mode="valid")
    n, i = 0, 0
    while i < len(mean):
        if mean[i] > cutoff:
            n += 1
            i += 30
        else:
            i += 1
    return n


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--word", required=True, choices=list(WAKE_WORDS))
    parser.add_argument("--model", help="Standard: data/models/<wort>/tflite_stream_state_internal_quant/...")
    args = parser.parse_args()

    path = args.model or DATA / "models" / args.word / "tflite_stream_state_internal_quant" / "stream_state_internal_quant.tflite"
    model = Model(str(path), stride=3)
    clips = DATA / "clips" / args.word

    groups = {name: [peak(model, read_wav(f)) for f in sorted((clips / name).glob("*.wav"))]
              for name in ["pos_real_test", "pos_real_train", "neg_similar", "neg_real"]}
    # Nur die echt aufgenommenen aehnlichen Woerter (TTS-Clips heissen nach der Quelle piper/edge)
    groups["neg_similar_real"] = [peak(model, read_wav(f)) for f in sorted((clips / "neg_similar").glob("*.wav"))
                                  if not f.name.startswith(("piper_", "edge_"))]
    ambient = [read_wav(f) for f in sorted((clips / "ambient").glob("*.wav"))]
    ambient_hours = sum(len(a) for a in ambient) / 16000 / 3600

    print(f"{'Schwelle':>8} | {'Test erkannt':>12} | {'Train erkannt':>13} | {'aehnlich (alle/echt)':>20} | {'Saetze/Stille':>13} | Raum/h")
    for c in CUTOFFS:
        def share(vals):
            return f"{sum(v > c for v in vals)}/{len(vals)}"
        amb = sum(triggers(model, a, c) for a in ambient) / ambient_hours if ambient else float("nan")
        print(f"{c:>8.2f} | {share(groups['pos_real_test']):>12} | {share(groups['pos_real_train']):>13} | "
              f"{share(groups['neg_similar']) + ' / ' + share(groups['neg_similar_real']):>20} | {share(groups['neg_real']):>13} | {amb:.1f}")


if __name__ == "__main__":
    main()
