"""Die Wake Words. Ein neues Wort hier eintragen, dann: docker compose run --rm train python train.py --word <schluessel>

  text      so wird es geschrieben und von den TTS-Stimmen gesprochen
  variants  weitere Schreibweisen fuer die TTS, z. B. um eine Aussprache zu erzwingen (optional)
  similar   klingen aehnlich und duerfen nicht ausloesen; je mehr, desto weniger Fehlausloesungen (optional)
  phonemes  Lautschrift (espeak/IPA) fuer Woerter, die Piper falsch betont; je Wort eine oder mehrere
            Aussprachen, die zufaellig gewechselt werden (optional)
"""

WORDS = {
    "giraffe": {
        "text": "Giraffe",
        "similar": ["Karaffe", "Giraffen", "Gitarre", "Grafik", "Affe", "Paraffin", "Gerade", "Garage"],
        # Piper spricht sonst "GI-raffe"; hier "gi-RAF-fe" und das verbreitete "schi-RAF-fe"
        "phonemes": {
            "Giraffe": ["ɡirˈafə", "ʒirˈafə"],
            "Giraffen": ["ɡirˈafən", "ʒirˈafən"],
        },
    },
    "computer": {
        "text": "Computer",
        # Erzwingt die deutsche Aussprache
        "variants": ["Kompjuter"],
        "similar": ["Kompott", "kompakt", "Komponist", "Computerspiel", "Kommune", "Puter", "Kompass"],
    },
}

WAKE_WORDS = {key: w["text"] for key, w in WORDS.items()}
SIMILAR = {key: w.get("similar", []) for key, w in WORDS.items()}
