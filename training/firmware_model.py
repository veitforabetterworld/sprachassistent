"""Schreibt ein trainiertes Modell als Header fuer die Firmware: firmware/src/wakeword_<wort>.h

    python firmware_model.py --word giraffe --cutoff 0.90
    python firmware_model.py --word giraffe --tflite pfad/zum/modell.tflite --cutoff 0.97

Die Schwelle (Wahrscheinlichkeit, gemittelt ueber --window Inferenzen) waehlt man anhand der
Testergebnisse des Trainings und am Geraet.
"""

import argparse
from pathlib import Path

from words import WAKE_WORDS

ROOT = Path(__file__).resolve().parent
FIRMWARE_SRC = ROOT.parent / "firmware" / "src"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--word", required=True, choices=list(WAKE_WORDS))
    parser.add_argument("--tflite", type=Path)
    parser.add_argument("--cutoff", type=float, required=True, help="0..1, z. B. 0.9")
    parser.add_argument("--window", type=int, default=5, help="Anzahl Inferenzen fuer den gleitenden Mittelwert")
    parser.add_argument("--arena", type=int, default=30000, help="Tensor-Arena in Bytes (wird auf dem ESP verkleinert)")
    args = parser.parse_args()

    tflite = args.tflite or ROOT / "data" / "models" / args.word / "tflite_stream_state_internal_quant" / "stream_state_internal_quant.tflite"
    data = tflite.read_bytes()
    lines = [", ".join(f"0x{b:02x}" for b in data[i : i + 16]) for i in range(0, len(data), 16)]
    name = WAKE_WORDS[args.word]
    out = FIRMWARE_SRC / f"wakeword_{args.word}.h"
    out.write_text(
        f"// Wake Word \"{name}\", erzeugt von training/firmware_model.py aus {tflite.name}\n"
        "#pragma once\n#include <stdint.h>\n\n"
        f"constexpr const char* WAKEWORD_NAME = \"{name}\";\n"
        f"constexpr uint8_t WAKEWORD_CUTOFF = {round(args.cutoff * 255)};  // {args.cutoff:.2f}\n"
        f"constexpr int WAKEWORD_WINDOW = {args.window};\n"
        f"constexpr size_t WAKEWORD_ARENA = {args.arena};\n\n"
        f"alignas(16) static const uint8_t WAKEWORD_MODEL[{len(data)}] = {{\n  " + ",\n  ".join(lines) + "\n};\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"{out}: {len(data)} Bytes Modell, Schwelle {args.cutoff}")


if __name__ == "__main__":
    main()
