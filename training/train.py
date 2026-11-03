"""Ein Wake Word von Anfang bis Ende: synthetische Stimmen, Fremddaten, Features, Training, Test und
Export fuer die Firmware. Das Wort muss in words.py stehen. Im Trainings-Container:

    docker compose run --rm train python train.py --word giraffe
    docker compose run --rm train python train.py --word giraffe --from export --cutoff 0.9

Erledigte Schritte werden beim naechsten Lauf uebersprungen, soweit ihre Daten schon da sind
(TTS-Clips, Downloads). Mit --from beginnt man bei einem spaeteren Schritt, z. B. um nur mit einer
anderen Schwelle neu zu exportieren.
"""

import argparse
import subprocess
import sys

from words import WAKE_WORDS

STEPS = ["tts", "download", "features", "train", "eval", "export"]


def run(*args: str):
    print(f"\n=== {' '.join(args)}", flush=True)
    subprocess.run([sys.executable, "-u", *args], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--word", required=True, choices=list(WAKE_WORDS))
    parser.add_argument("--from", dest="start", choices=STEPS, default=STEPS[0], help="ab diesem Schritt")
    parser.add_argument("--cutoff", type=float, default=0.97, help="Schwelle fuer die Firmware (0..1)")
    parser.add_argument("--steps", type=int, default=20000, help="Trainingsschritte")
    args = parser.parse_args()

    todo = STEPS[STEPS.index(args.start) :]
    w = args.word
    if "tts" in todo:
        run("make_tts.py", "--word", w)
    if "download" in todo:
        run("fetch_aug_data.py")
    if "features" in todo:
        run("prepare_features.py", "--word", w)
    if "train" in todo:
        run("train_wakeword.py", "--word", w, "--steps", str(args.steps), "--weights", "last_weights", "--restart")
    if "eval" in todo:
        run("eval_clips.py", "--word", w)
    if "export" in todo:
        run("firmware_model.py", "--word", w, "--cutoff", str(args.cutoff))
        print(f"\nFertig. Flashen: cd firmware && pio run -e {w} -t upload")


if __name__ == "__main__":
    main()
