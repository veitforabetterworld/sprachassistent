"""Uebersicht ueber die eigenen Aufnahmen aus record.py, mit Hinweisen auf auffaellige Clips.

    python inspect_recordings.py            # alle Personen
    python inspect_recordings.py --person veit
"""

import argparse
import json
from collections import defaultdict
from pathlib import Path

import numpy as np

from record import DATA, PRE_ROLL, RATE, analyse, read_wav


def load(person: str | None) -> list[dict]:
    entries = []
    for log in sorted(DATA.glob("*/clips.jsonl")):
        if person and log.parent.name != person:
            continue
        for line in log.read_text(encoding="utf-8").splitlines():
            e = json.loads(line)
            if (DATA / e["file"]).exists():
                entries.append(e)
    return entries


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--person")
    args = parser.parse_args()
    entries = load(args.person)
    if not entries:
        print("Keine Aufnahmen gefunden")
        return

    groups = defaultdict(list)
    for e in entries:
        key = e["class"].split("/")[0] if e["kind"] == "similar" else e["class"]
        groups[key].append(e)

    print(f"{len(entries)} Clips von {len({e['person'] for e in entries})} Person(en)\n")
    print(f"{'Klasse':<18}{'Anzahl':>7}{'Dauer s':>14}{'Spitze dBFS':>14}{'Abstand dB':>13}{'Rauschen':>10}")
    for key in sorted(groups):
        g = groups[key]
        def col(name):
            v = [e[name] for e in g if e.get(name) is not None]
            return f"{np.median(v):.1f} ({min(v):.1f}-{max(v):.1f})" if v else "-"
        print(f"{key:<18}{len(g):>7}{col('speech_seconds'):>14}{col('peak_dbfs'):>14}{col('snr_db'):>13}{np.median([e['noise_dbfs'] for e in g]):>10.1f}")

    print("\nNach Sprechweise (nur Wake Words):")
    by_style = defaultdict(list)
    for e in entries:
        if e["kind"] == "wake":
            by_style[(e["class"], e["style"])].append(e)
    for (cls, style), g in sorted(by_style.items()):
        print(f"  {cls:<10}{style:<10}{len(g):>3}x  Dauer {np.median([e['speech_seconds'] for e in g]):.2f} s"
              f"  Spitze {np.median([e['peak_dbfs'] for e in g]):6.1f} dBFS  Abstand {np.median([e['snr_db'] for e in g]):5.1f} dB")

    print("\nAuffaellig:")
    found = False
    for e in entries:
        if e["kind"] not in ("wake", "similar"):
            continue
        notes = []
        raw = read_wav(DATA / e["raw"])
        info = analyse(raw)
        start, end = info["speech"]
        if start < int(PRE_ROLL * RATE):
            notes.append(f"Sprache schon vor JETZT ({start / RATE:.2f} s)")
        if end > len(raw) - RATE // 10:
            notes.append("Sprache bis zum Ende der Aufnahme, evtl. abgeschnitten")
        if e["speech_seconds"] < 0.3:
            notes.append(f"sehr kurz ({e['speech_seconds']} s)")
        if e["snr_db"] < 10:
            notes.append(f"wenig Abstand zum Rauschen ({e['snr_db']} dB)")
        if e["clipped"]:
            notes.append(f"{e['clipped']} Samples uebersteuert")
        if notes:
            found = True
            print(f"  {e['file']}: " + "; ".join(notes))
    if not found:
        print("  nichts")


if __name__ == "__main__":
    main()
