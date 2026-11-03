"""Bereitet die heruntergeladenen Fremddaten fuer das Training auf (im Trainings-Container).

Erwartet in data/downloads/ (siehe docs/wakeword-plan.md, Phase 2):
  dinner_party.zip, dinner_party_eval.zip, no_speech.zip, speech.zip  (Features von microWakeWord)
  fma_xs.zip                                                          (Musik, Free Music Archive)

Laedt zusaetzlich die MIT-Raumimpulsantworten von Hugging Face.

    python fetch_aug_data.py
"""

import shutil
import subprocess
import zipfile
from pathlib import Path

from tqdm import tqdm

DATA = Path(__file__).resolve().parent / "data"
DOWNLOADS = DATA / "downloads"


def unzip_negatives():
    out = DATA / "negative_datasets"
    for name in ["dinner_party", "dinner_party_eval", "no_speech", "speech"]:
        if (out / name).exists():
            continue
        print("entpacke", name)
        with zipfile.ZipFile(DOWNLOADS / f"{name}.zip") as z:
            z.extractall(out)


def shorten_ambient_eval(hours={"validation_ambient": 1.0, "testing_ambient": 2.0}):
    """dinner_party_eval hat 9,7 h (Validierung) und 5,3 h (Test). microWakeWord zerlegt das am Stueck in
    Fenster, das sprengt 7 GB RAM. Gekuerzte Kopie: gleich viel Zeit aus jeder Aufnahme."""
    from mmap_ninja.ragged import RaggedMmap

    src, out = DATA / "negative_datasets" / "dinner_party_eval", DATA / "negative_datasets" / "dinner_party_eval_kurz"
    if out.exists():
        return
    for split, h in hours.items():
        for d in (src / split).iterdir():
            parts = list(RaggedMmap(str(d)))
            frames = int(h * 3600 * 100 / len(parts))
            target = out / split / d.name
            target.parent.mkdir(parents=True, exist_ok=True)
            RaggedMmap.from_generator(out_dir=str(target), sample_generator=(p[:frames] for p in parts), batch_size=10, verbose=False)
            print(f"{split}: {len(parts)} Aufnahmen auf je {frames / 6000:.0f} min gekuerzt")


def rirs():
    out = DATA / "mit_rirs"
    if out.exists() and any(out.iterdir()):
        return
    # Direkt die WAV-Dateien laden; datasets braeuchte zum Dekodieren torchcodec
    from huggingface_hub import snapshot_download

    snapshot_download("davidscripka/MIT_environmental_impulse_responses", repo_type="dataset",
                      allow_patterns=["16khz/*.wav"], local_dir=DATA / "mit_rirs_raw")
    out.mkdir(parents=True, exist_ok=True)
    for f in sorted((DATA / "mit_rirs_raw" / "16khz").glob("*.wav")):
        shutil.move(f, out / f.name)
    shutil.rmtree(DATA / "mit_rirs_raw")


def fma():
    out = DATA / "fma_16k"
    if out.exists() and any(out.iterdir()):
        return
    raw = DATA / "fma"
    if not raw.exists():
        print("entpacke FMA")
        with zipfile.ZipFile(DOWNLOADS / "fma_xs.zip") as z:
            z.extractall(raw)
    out.mkdir(parents=True, exist_ok=True)
    for mp3 in tqdm(sorted(raw.glob("**/*.mp3")), desc="FMA -> 16 kHz"):
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y", "-i", str(mp3), "-ac", "1", "-ar", "16000", str(out / f"{mp3.stem}.wav")],
            check=False,
        )


if __name__ == "__main__":
    unzip_negatives()
    shorten_ambient_eval()
    rirs()
    fma()
