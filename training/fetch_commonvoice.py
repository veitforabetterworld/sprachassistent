"""Laedt Common Voice Deutsch (Scripted Speech) von Mozilla Data Collective.

Vorher einmalig auf mozilladatacollective.com: Konto anlegen, beim Datensatz die Bedingungen
akzeptieren, im Dashboard einen API-Schluessel erzeugen und in training/.env eintragen:

    MDC_API_KEY=...

    python fetch_commonvoice.py                  # nach D:\\datasets\\commonvoice-de
    python fetch_commonvoice.py --dir E:\\cv     # anderer Ort

Der Download wird bei Abbruch fortgesetzt, wenn man das Skript erneut startet.
"""

import argparse
import os
import sys
from pathlib import Path

# Common Voice Scripted Speech 25.0 - German (CC0), 1486 h, 20 466 Sprecher
DATASET_ID = "cmn4rsdh6009unz07jdn2ol9p"


def load_env():
    env = Path(__file__).resolve().parent / ".env"
    if env.exists():
        for line in env.read_text(encoding="utf-8").splitlines():
            if "=" in line and not line.lstrip().startswith("#"):
                key, value = line.split("=", 1)
                os.environ.setdefault(key.strip(), value.strip().strip('"'))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dir", default=r"D:\datasets\commonvoice-de")
    parser.add_argument("--no-extract", action="store_true", help="nur das Archiv laden, nicht entpacken")
    args = parser.parse_args()

    load_env()
    if not os.environ.get("MDC_API_KEY"):
        sys.exit("MDC_API_KEY fehlt (training/.env), siehe Anleitung oben in dieser Datei")

    from datacollective import download_dataset, get_dataset_details

    details = get_dataset_details(DATASET_ID)
    print(details)
    target = Path(args.dir)
    target.mkdir(parents=True, exist_ok=True)
    path = download_dataset(DATASET_ID, download_directory=str(target), extract=not args.no_extract, max_download_attempts=10)
    print(f"Fertig: {path}")


if __name__ == "__main__":
    main()
