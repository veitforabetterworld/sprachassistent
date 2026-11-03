# Trainingsumgebung fuer die Wake Words (microWakeWord, TensorFlow auf der CPU, Piper und Edge TTS).
#
#   docker build -t wakeword-train training
#   docker run --rm -it -v "%cd%\training:/work" wakeword-train python make_tts.py ...
#
# /work ist der Ordner training/ aus dem Repo, Daten und Modelle landen in /work/data.
FROM python:3.11-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        git build-essential ffmpeg libsndfile1 unzip wget ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# microWakeWord auf festem Stand (wie in docs/wakeword-plan.md beschrieben)
ARG MWW_COMMIT=4665173
RUN git clone https://github.com/kahrendt/microWakeWord /opt/microWakeWord \
    && cd /opt/microWakeWord && git checkout ${MWW_COMMIT} \
    && pip install --no-cache-dir 'git+https://github.com/whatsnowplaying/audio-metadata@d4ebb238e6a401bb1a5aaaac60c9e2b3cb30929f' \
    && pip install --no-cache-dir -e /opt/microWakeWord

# datasets 4+ dekodiert Audio nur noch mit torchcodec; 3.x nutzt soundfile wie microWakeWord es erwartet
RUN pip install --no-cache-dir piper-tts edge-tts scipy soundfile tqdm tensorboard "datasets<4"

# Fortschritt des Trainings sofort ausgeben (laeuft als Unterprozess)
ENV PYTHONUNBUFFERED=1

WORKDIR /work
