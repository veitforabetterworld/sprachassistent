"""PlatformIO-Vorskript fuer die Envs mit Wake Word (extra_scripts in platformio.ini).

1. Holt TensorFlow Lite Micro und esp-nn nach firmware/lib/, einmalig. Gleiche Versionen wie ESPHome
   micro_wake_word, damit microWakeWord-Modelle laufen. Die Quellen stehen nicht im Repo; library.json
   legt dieses Skript an. Das Audio-Frontend (esp-micro-speech-features) steht in lib_deps.
2. Stellt das Modell aus custom_wakeword (firmware/wakewords/<wort>.h) als wakeword_model.h bereit.
"""

import json
import shutil
import subprocess
from pathlib import Path


TFLM_KERNELS_FROM_ESP_NN = ["add", "conv", "depthwise_conv", "fully_connected", "mul", "pooling", "softmax"]

LIBS = {
    "esp-tflite-micro": {
        "repo": "https://github.com/espressif/esp-tflite-micro.git",
        "tag": "v1.3.3",
        "keep": ["tensorflow", "signal", "third_party", "LICENSE"],
        "library": {
            "name": "esp-tflite-micro",
            "version": "1.3.3",
            "dependencies": [{"name": "esp-nn"}],
            "license": "Apache-2.0",
            "frameworks": "*",
            "platforms": "espressif32",
            "build": {
                "srcDir": ".",
                "includeDir": ".",
                "srcFilter": [
                    "-<*>",
                    "+<tensorflow/lite/micro/*.cc>",
                    "+<tensorflow/lite/micro/kernels/*.cc>",
                    *[f"-<tensorflow/lite/micro/kernels/{k}.cc>" for k in TFLM_KERNELS_FROM_ESP_NN],
                    "+<tensorflow/lite/micro/kernels/esp_nn/*.cc>",
                    "+<tensorflow/lite/micro/tflite_bridge/*.cc>",
                    "+<tensorflow/lite/micro/memory_planner/greedy_memory_planner.cc>",
                    "+<tensorflow/lite/micro/memory_planner/linear_memory_planner.cc>",
                    "+<tensorflow/lite/micro/arena_allocator/*.cc>",
                    "+<tensorflow/lite/core/c/common.cc>",
                    "+<tensorflow/lite/core/api/flatbuffer_conversions.cc>",
                    "+<tensorflow/lite/core/api/tensor_utils.cc>",
                    "+<tensorflow/lite/kernels/kernel_util.cc>",
                    "+<tensorflow/lite/kernels/internal/common.cc>",
                    "+<tensorflow/lite/kernels/internal/quantization_util.cc>",
                    "+<tensorflow/lite/kernels/internal/portable_tensor_utils.cc>",
                    "+<tensorflow/lite/kernels/internal/tensor_utils.cc>",
                    "+<tensorflow/lite/kernels/internal/tensor_ctypes.cc>",
                    "+<tensorflow/lite/kernels/internal/reference/portable_tensor_utils.cc>",
                    "+<tensorflow/lite/kernels/internal/reference/comparisons.cc>",
                    "+<tensorflow/compiler/mlir/lite/core/api/error_reporter.cc>",
                    "+<tensorflow/compiler/mlir/lite/schema/schema_utils.cc>",
                    "+<signal/micro/kernels/*.cc>",
                    "+<signal/src/*.cc>",
                    "+<signal/src/kiss_fft_wrappers/*.cc>",
                ],
                "flags": [
                    "-DESP_NN",
                    "-O3",
                    "-fno-exceptions",
                    "-fno-rtti",
                    "-Wno-unused-parameter",
                    "-Wno-missing-field-initializers",
                ],
            },
        },
    },
    "esp-nn": {
        "repo": "https://github.com/espressif/esp-nn.git",
        "tag": "v1.1.2",
        "keep": ["include", "src", "LICENSE"],
        "library": {
            "name": "esp-nn",
            "version": "1.1.2",
            "license": "Apache-2.0",
            "frameworks": "*",
            "platforms": "espressif32",
            "build": {
                "srcDir": "src",
                "includeDir": "include",
                "srcFilter": ["-<*>", "+<**/*_ansi.c>", "+<**/*_opt.c>"],
                "flags": ["-O2", "-Wno-unused-function"],
            },
        },
    },
}


def fetch(lib_dir: Path, name: str, spec: dict) -> None:
    target = lib_dir / name
    stamp = target / ".version"
    if stamp.exists() and stamp.read_text().strip() == spec["tag"]:
        return
    if target.exists():
        shutil.rmtree(target)
    tmp = lib_dir / f".{name}-tmp"
    if tmp.exists():
        shutil.rmtree(tmp)
    subprocess.run(
        ["git", "clone", "-q", "--depth", "1", "-c", "advice.detachedHead=false", "-b", spec["tag"], spec["repo"], str(tmp)],
        check=True,
    )
    target.mkdir(parents=True)
    for item in spec["keep"]:
        src = tmp / item
        if src.is_dir():
            shutil.copytree(src, target / item)
        else:
            shutil.copy2(src, target / item)
    shutil.rmtree(tmp, onerror=lambda func, path, _: (Path(path).chmod(0o700), func(path)))
    (target / "library.json").write_text(json.dumps(spec["library"], indent=2) + "\n")
    stamp.write_text(spec["tag"] + "\n")
    print(f"{name} {spec['tag']} geholt")


Import("env")  # noqa: F821 (von PlatformIO bereitgestellt)

project = Path(env.subst("$PROJECT_DIR"))  # noqa: F821
(project / "lib").mkdir(exist_ok=True)
for lib_name, lib_spec in LIBS.items():
    fetch(project / "lib", lib_name, lib_spec)

word = env.GetProjectOption("custom_wakeword")  # noqa: F821
model = project / "wakewords" / f"{word}.h"
if not model.exists():
    raise SystemExit(f"Kein Modell {model}; erzeugen mit training/train.py --word {word}")
include = Path(env.subst("$BUILD_DIR")) / "wakeword"  # noqa: F821
include.mkdir(parents=True, exist_ok=True)
target = include / "wakeword_model.h"
if not target.exists() or target.read_bytes() != model.read_bytes():
    shutil.copyfile(model, target)
env.Append(CPPPATH=[str(include)])  # noqa: F821
