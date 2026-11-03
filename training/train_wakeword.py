"""Trainiert ein Wake-Word-Modell mit microWakeWord (im Trainings-Container, nach prepare_features.py).

    python train_wakeword.py --word giraffe
    python train_wakeword.py --word giraffe --steps 20000

Laeuft normalerweise ueber train.py. Die ~28 GB Features liest es zufaellig, deshalb liegen sie in
Docker-Volumes (compose.yaml).

Ergebnis: data/models/<wort>/ mit dem quantisierten Streaming-Modell (stream_state_internal_quant.tflite)
und den Testergebnissen. firmware_model.py macht daraus den Header fuer die Firmware.
"""

import argparse
import subprocess
import sys
from pathlib import Path

import yaml

from words import WAKE_WORDS

DATA = Path(__file__).resolve().parent / "data"


def feature_sets(word: str) -> list[dict]:
    f = DATA / "features" / word
    neg = DATA / "negative_datasets"
    sets = [
        # Positiv
        {"features_dir": f / "pos_tts", "sampling_weight": 2.0, "penalty_weight": 1.0, "truth": True, "truncation_strategy": "truncate_start"},
        {"features_dir": f / "pos_real", "sampling_weight": 1.0, "penalty_weight": 1.0, "truth": True, "truncation_strategy": "truncate_start"},
        # Negativ: aehnliche Woerter und eigene Aufnahmen werden haerter bestraft
        {"features_dir": f / "neg_similar", "sampling_weight": 3.0, "penalty_weight": 2.0, "truth": False, "truncation_strategy": "random"},
        {"features_dir": f / "neg_other_word", "sampling_weight": 1.0, "penalty_weight": 2.0, "truth": False, "truncation_strategy": "random"},
        {"features_dir": f / "neg_real", "sampling_weight": 2.0, "penalty_weight": 1.0, "truth": False, "truncation_strategy": "random"},
        {"features_dir": f / "ambient", "sampling_weight": 2.0, "penalty_weight": 1.0, "truth": False, "truncation_strategy": "split"},
        # Fremddaten von microWakeWord (englische Sprache, Partys, Geraeusche)
        {"features_dir": neg / "speech", "sampling_weight": 10.0, "penalty_weight": 1.0, "truth": False, "truncation_strategy": "random"},
        {"features_dir": neg / "dinner_party", "sampling_weight": 10.0, "penalty_weight": 1.0, "truth": False, "truncation_strategy": "random"},
        {"features_dir": neg / "no_speech", "sampling_weight": 5.0, "penalty_weight": 1.0, "truth": False, "truncation_strategy": "random"},
        {"features_dir": neg / "dinner_party_eval_kurz", "sampling_weight": 0.0, "penalty_weight": 1.0, "truth": False, "truncation_strategy": "split"},
    ]
    out = []
    for s in sets:
        if s["features_dir"].exists():
            out.append({**s, "features_dir": str(s["features_dir"]), "type": "mmap"})
        else:
            print(f"fehlt, wird ausgelassen: {s['features_dir']}")
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--word", required=True, choices=list(WAKE_WORDS))
    parser.add_argument("--steps", type=int, default=20000)
    parser.add_argument("--restart", action="store_true", help="vorhandenes Training verwerfen")
    parser.add_argument("--eval-only", action="store_true", help="nicht trainieren, nur gespeicherte Gewichte testen und exportieren")
    parser.add_argument("--weights", choices=["best_weights", "last_weights"], default="best_weights")
    args = parser.parse_args()

    train_dir = DATA / "models" / args.word
    if args.restart and train_dir.exists():
        import shutil

        shutil.rmtree(train_dir)
    train_dir.parent.mkdir(parents=True, exist_ok=True)

    steps = args.steps
    config = {
        "window_step_ms": 10,
        "train_dir": str(train_dir),
        "features": feature_sets(args.word),
        # Zwei Phasen: erst lernen, dann mit kleiner Lernrate und haerterer Strafe fuer Fehlausloesungen feinjustieren
        "training_steps": [steps * 3 // 4, steps // 4],
        "positive_class_weight": [1, 1],
        "negative_class_weight": [20, 30],
        "learning_rates": [0.001, 0.0002],
        "batch_size": 128,
        "time_mask_max_size": [0, 0],
        "time_mask_count": [0, 0],
        "freq_mask_max_size": [0, 0],
        "freq_mask_count": [0, 0],
        "eval_step_interval": 500,
        "clip_duration_ms": 1500,
        # Nur die mittlere Erkennung ueber alle brauchbaren Schwellen bewerten. Mit "Fehlausloesungen bei
        # Schwelle 0,5 minimieren" gewinnt ein fast untrainiertes Modell, das kaum etwas ueber 0,5 hebt.
        "target_minimization": 0.5,
        "minimization_metric": None,
        "maximization_metric": "average_viable_recall",
    }
    config_path = DATA / "models" / f"{args.word}_training.yaml"
    config_path.write_text(yaml.dump(config), encoding="utf-8")

    cmd = [
        sys.executable, "-m", "microwakeword.model_train_eval",
        f"--training_config={config_path}",
        "--train", "0" if args.eval_only else "1",
        "--restore_checkpoint", "1",
        "--test_tf_nonstreaming", "0",
        "--test_tflite_nonstreaming", "0",
        "--test_tflite_nonstreaming_quantized", "0",
        "--test_tflite_streaming", "0",
        "--test_tflite_streaming_quantized", "1",
        "--use_weights", args.weights,
        # Gleiche Architektur wie die offiziellen v2-Modelle (okay_nabu laeuft in Phase 0 mit 7 ms auf dem ESP32)
        "mixednet",
        "--pointwise_filters", "64,64,64,64",
        "--repeat_in_block", "1, 1, 1, 1",
        "--mixconv_kernel_sizes", "[5], [7,11], [9,15], [23]",
        "--residual_connection", "0,0,0,0",
        "--first_conv_filters", "32",
        "--first_conv_kernel_size", "5",
        "--stride", "3",
    ]
    subprocess.run(cmd, check=True)
    print(f"Fertig: {train_dir}/tflite_stream_state_internal_quant/stream_state_internal_quant.tflite")


if __name__ == "__main__":
    main()
