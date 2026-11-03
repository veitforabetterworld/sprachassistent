"""Wake Words und aehnlich klingende Woerter, gemeinsam fuer Aufnahme, TTS und Training."""

WAKE_WORDS = {"giraffe": "Giraffe", "computer": "Computer"}

# Klingen aehnlich und duerfen nicht ausloesen
SIMILAR = {
    "giraffe": ["Karaffe", "Giraffen", "Gitarre", "Grafik", "Affe", "Paraffin", "Gerade", "Garage"],
    "computer": ["Kompott", "kompakt", "Komponist", "Computerspiel", "Kommune", "Puter", "Kompass"],
}
