"""Schneidet die Clips zu, augmentiert sie und erzeugt die Spektrogramm-Features fuer ein Wake Word.

Laeuft im Trainings-Container, nach make_tts.py und fetch_aug_data.py:

    python prepare_features.py --word giraffe

Quellen (alle unter data/):
  tts/<wort>/*/            synthetische Zielwoerter          -> positiv
  real/<person>/<wort>/    eigene Aufnahmen                  -> positiv (je Clip fest Training oder Test)
  tts/aehnlich_<wort>/*/   synthetische aehnliche Woerter    -> negativ
  real/<person>/aehnlich/  aufgenommene aehnliche Woerter    -> negativ
  real/<person>/saetze, stille, das jeweils andere Wake Word -> negativ
  real/_ambient/           Raumgeraeusche                    -> negativ, ausserdem Hintergrund fuer die Augmentation
Ergebnis: data/features/<wort>/<set>/{training,validation,testing}/*_mmap
"""

import argparse
import shutil
import wave
import zlib
from pathlib import Path

import numpy as np
from mmap_ninja.ragged import RaggedMmap

from microwakeword.audio.augmentation import Augmentation
from microwakeword.audio.clips import Clips
from microwakeword.audio.spectrograms import SpectrogramGeneration
from words import WAKE_WORDS

DATA = Path(__file__).resolve().parent / "data"
RATE = 16000


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path), "rb") as w:
        return np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")


def write_wav(path: Path, x: np.ndarray):
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(x.astype("<i2").tobytes())


def trim(x: np.ndarray) -> np.ndarray | None:
    """Stille vorne und hinten entfernen: das Modell lernt, direkt nach dem Wortende auszuloesen."""
    frame = RATE // 100
    n = len(x) // frame
    if n < 5:
        return None
    rms = np.sqrt((x[: n * frame].astype(np.float32).reshape(n, frame) ** 2).mean(axis=1))
    threshold = max(np.percentile(rms, 20) * 4, rms.max() * 10 ** (-35 / 20))
    active = np.flatnonzero(rms > threshold)
    if len(active) == 0:
        return None
    start = max(0, (active[0] - 5) * frame)
    end = min(len(x), (active[-1] + 4) * frame)
    return x[start:end]


def is_test(path: Path) -> bool:
    return zlib.crc32(path.name.encode()) % 2 == 0


def collect(word: str, test_persons: set[str]) -> Path:
    """Sammelt und schneidet alle Clips nach data/clips/<wort>/<gruppe>/."""
    out = DATA / "clips" / word
    if out.exists():
        shutil.rmtree(out)
    other = [w for w in WAKE_WORDS if w != word]
    counts = {}

    def put(group: str, src: Path, do_trim: bool):
        x = read_wav(src)
        if do_trim:
            x = trim(x)
            if x is None:
                return
        write_wav(out / group / f"{src.parent.name}_{src.name}", x)
        counts[group] = counts.get(group, 0) + 1

    for f in sorted((DATA / "tts" / word).glob("*/*.wav")):
        put("pos_tts", f, True)
    for f in sorted((DATA / "tts" / f"aehnlich_{word}").glob("*/*.wav")):
        put("neg_similar", f, False)
    for o in other:
        for f in sorted((DATA / "tts" / o).glob("*/*.wav")):
            put("neg_other_word", f, False)

    for person_dir in sorted(p for p in (DATA / "real").iterdir() if p.is_dir() and not p.name.startswith("_")):
        person = person_dir.name
        for f in sorted((person_dir / word).glob("*.wav")):
            test = person in test_persons if test_persons else is_test(f)
            put("pos_real_test" if test else "pos_real_train", f, True)
        for f in sorted((person_dir / "aehnlich").glob("*/*.wav")):
            put("neg_similar", f, False)
        for cls in ["saetze", "stille", f"{word}_satz"] + other + [f"{o}_satz" for o in other]:
            # Saetze mit dem eigenen Wort vorneweg sind nicht eindeutig negativ: nur zum Testen, nicht im Training
            if cls == f"{word}_satz":
                continue
            for f in sorted((person_dir / cls).glob("*.wav")):
                put("neg_real", f, False)

    for f in sorted((DATA / "real" / "_ambient").glob("*.wav")):
        put("ambient", f, False)
    for f in sorted((DATA / "real" / "_speaker").glob("*.wav")):
        put("neg_real", f, False)

    for group, n in sorted(counts.items()):
        print(f"  {group:<16}{n:>6} Clips")
    return out


def augmenter(backgrounds: list[str]) -> Augmentation:
    return Augmentation(
        augmentation_duration_s=3.2,
        augmentation_probabilities={
            "SevenBandParametricEQ": 0.25,  # grobe Nachbildung unterschiedlicher Mikrofone
            "TanhDistortion": 0.1,
            "PitchShift": 0.15,
            "BandStopFilter": 0.1,
            "AddColorNoise": 0.25,
            "AddBackgroundNoise": 0.75,
            "Gain": 1.0,
            "RIR": 0.5,
        },
        impulse_paths=[str(DATA / "mit_rirs")],
        background_paths=backgrounds,
        background_min_snr_db=-5,
        background_max_snr_db=15,
        min_jitter_s=0.195,
        max_jitter_s=0.205,
    )


def write_split(out: Path, split_dir: str, generator):
    target = out / split_dir / "features_mmap"
    if target.exists():
        shutil.rmtree(target)
    target.parent.mkdir(parents=True, exist_ok=True)
    RaggedMmap.from_generator(out_dir=str(target), sample_generator=generator, batch_size=100, verbose=False)
    print(f"    {split_dir}: {len(RaggedMmap(str(target)))} Spektrogramme")


def features_random_split(src: Path, out: Path, aug: Augmentation, train_repeat: int):
    """Teilt zufaellig 10 % fuer Validierung und Test ab."""
    clips = Clips(input_directory=str(src), file_pattern="*.wav", random_split_seed=10, split_count=0.1)
    for split_dir, split, repeat, slide in [("training", "train", train_repeat, 10), ("validation", "validation", 1, 10), ("testing", "test", 1, 1)]:
        spec = SpectrogramGeneration(clips=clips, augmenter=aug, slide_frames=slide, step_ms=10)
        write_split(out, split_dir, spec.spectrogram_generator(split=split, repeat=repeat))


def features_fixed_split(train_src: Path, test_src: Path, out: Path, aug: Augmentation, train_repeat: int):
    """Eigene Aufnahmen: Training und Test stehen pro Clip fest."""
    if any(train_src.glob("*.wav")):
        spec = SpectrogramGeneration(clips=Clips(str(train_src), "*.wav"), augmenter=aug, slide_frames=10, step_ms=10)
        write_split(out, "training", spec.spectrogram_generator(repeat=train_repeat))
    if any(test_src.glob("*.wav")):
        test_clips = Clips(str(test_src), "*.wav")
        spec = SpectrogramGeneration(clips=test_clips, augmenter=aug, slide_frames=10, step_ms=10)
        write_split(out, "validation", spec.spectrogram_generator(repeat=2))
        spec = SpectrogramGeneration(clips=test_clips, augmenter=aug, slide_frames=1, step_ms=10)
        write_split(out, "testing", spec.spectrogram_generator(repeat=2))


def features_ambient(src: Path, out: Path):
    """Lange Aufnahmen ohne Augmentation: Training in 3-s-Stuecken, Validierung/Test als Ganzes (Fehlausloesungen pro Stunde)."""
    files = sorted(src.glob("*.wav"))
    if not files:
        return
    split_at = max(1, len(files) * 3 // 5)
    tmp_train, tmp_eval = src.parent / "ambient_train", src.parent / "ambient_eval"
    for d, fs in [(tmp_train, files[:split_at]), (tmp_eval, files[split_at:])]:
        d.mkdir(exist_ok=True)
        for f in fs:
            shutil.copy(f, d / f.name)
    spec = SpectrogramGeneration(clips=Clips(str(tmp_train), "*.wav"), step_ms=10, split_spectrogram_duration_s=3.0)
    write_split(out, "training", spec.spectrogram_generator())
    if any(tmp_eval.glob("*.wav")):
        spec = SpectrogramGeneration(clips=Clips(str(tmp_eval), "*.wav"), step_ms=10)
        write_split(out, "validation_ambient", spec.spectrogram_generator())
        write_split(out, "testing_ambient", SpectrogramGeneration(clips=Clips(str(tmp_eval), "*.wav"), step_ms=10).spectrogram_generator())


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--word", required=True, choices=list(WAKE_WORDS))
    parser.add_argument("--test-persons", default="", help="Personen nur zum Testen, z. B. anna,ben (Standard: je Clip zufaellig)")
    args = parser.parse_args()

    print("Clips sammeln und zuschneiden:")
    clips = collect(args.word, {p for p in args.test_persons.split(",") if p})
    out = DATA / "features" / args.word
    backgrounds = [str(DATA / "fma_16k"), str(clips / "ambient")]
    aug = augmenter(backgrounds)

    print("pos_tts:")
    features_random_split(clips / "pos_tts", out / "pos_tts", aug, train_repeat=2)
    print("pos_real:")
    features_fixed_split(clips / "pos_real_train", clips / "pos_real_test", out / "pos_real", aug, train_repeat=10)
    for neg in ["neg_similar", "neg_other_word", "neg_real"]:
        if any((clips / neg).glob("*.wav")):
            print(f"{neg}:")
            features_random_split(clips / neg, out / neg, aug, train_repeat=2)
    print("ambient:")
    features_ambient(clips / "ambient", out / "ambient")


if __name__ == "__main__":
    main()
