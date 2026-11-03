"""Laedt und entpackt die Fremddaten fuers Training (im Trainings-Container, einmalig ~6 GB Download):

  Negativ-Features von microWakeWord (Sprache, Partys, Geraeusche) -> data/negative_datasets/
  Musik aus dem Free Music Archive                                -> data/fma_16k/
  MIT-Raumimpulsantworten                                         -> data/mit_rirs/

    python fetch_aug_data.py

Abgebrochene Downloads werden fortgesetzt; die ZIPs werden nach dem Entpacken geloescht.
"""

import shutil
import subprocess
import urllib.request
import zipfile
from pathlib import Path

from tqdm import tqdm

DATA = Path(__file__).resolve().parent / "data"
DOWNLOADS = DATA / "downloads"
NEGATIVES_URL = "https://huggingface.co/datasets/kahrendt/microwakeword/resolve/main/{}.zip"
FMA_URL = "https://huggingface.co/datasets/mchl914/fma_xsmall/resolve/main/fma_xs.zip"


def download(url: str) -> Path:
    """Laedt nach data/downloads/, setzt einen abgebrochenen Download fort."""
    DOWNLOADS.mkdir(parents=True, exist_ok=True)
    target = DOWNLOADS / url.rsplit("/", 1)[1]
    part = target.with_suffix(".part")
    if target.exists():
        return target
    have = part.stat().st_size if part.exists() else 0
    req = urllib.request.Request(url, headers={"Range": f"bytes={have}-"} if have else {})
    with urllib.request.urlopen(req) as r:
        if have and r.status != 206:
            have = 0
        total = have + int(r.headers.get("Content-Length", 0))
        with open(part, "ab" if have else "wb") as f, tqdm(total=total, initial=have, unit="B", unit_scale=True, desc=target.name) as bar:
            while chunk := r.read(1 << 20):
                f.write(chunk)
                bar.update(len(chunk))
    part.rename(target)
    return target


def unzip_negatives():
    out = DATA / "negative_datasets"
    for name in ["dinner_party", "dinner_party_eval", "no_speech", "speech"]:
        done = out / name / ".fertig"
        if done.exists():
            continue
        archive = download(NEGATIVES_URL.format(name))
        print("entpacke", name)
        shutil.rmtree(out / name, ignore_errors=True)
        with zipfile.ZipFile(archive) as z:
            z.extractall(out)
        done.touch()
        archive.unlink()


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
        archive = download(FMA_URL)
        print("entpacke FMA")
        with zipfile.ZipFile(archive) as z:
            z.extractall(raw)
        archive.unlink()
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
