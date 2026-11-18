#!/usr/bin/env python3
"""Fetch the demo recordings used by the app's Play button into demos/.

They are MIDI captures of live competition performances from the MAESTRO dataset v3.0.0
(Hawthorne et al., 2019), licensed CC BY-NC-SA 4.0, so they are downloaded rather than
kept in this repository. Run from the repository root: python3 -I tools/fetch_demos.py
"""
import csv, io, os, re, urllib.request, zipfile

URL = "https://storage.googleapis.com/magentadata/datasets/maestro/v3.0.0/maestro-v3.0.0-midi.zip"
WANT = [
    ("Chopin - Nocturne Op. 9 No. 2", r"Nocturne Op\. 9 No\. 2"),
    ("Bach - Prelude and Fugue in C major (WTC)", r"BWV 846"),
    ("Debussy - Suite bergamasque (Clair de lune is the third piece)", r"SuiteBergamasque"),
    ("Rachmaninoff - Prelude in C-sharp minor", r"Prelude Op\. 3 No\. 2"),
    ("Liszt - La Campanella", r"La Campanella"),
    ("Chopin - Revolutionary Etude", r"Etude Op\. 10 No\. 12$"),
    ("Chopin - Raindrop Prelude (and No. 24)", r"Two Preludes from Op\. 28"),
    ("Bach - Jesu, Joy of Man's Desiring", r"Jesu, Joy"),
    ("Liszt - Hungarian Rhapsody No. 2", r"Hungarian Rhapsody No\. 2 in"),
    ("Chopin - Ballade No. 1", r"Ballade No\. 1"),
    ("Beethoven - Appassionata, first movement", r'Appassionata", First'),
]

os.makedirs("demos", exist_ok=True)
print("downloading (about 58 MB)...")
z = zipfile.ZipFile(io.BytesIO(urllib.request.urlopen(URL).read()))
rows = list(csv.DictReader(io.TextIOWrapper(z.open("maestro-v3.0.0/maestro-v3.0.0.csv"), encoding="utf-8")))
lines = []
for i, (title, pattern) in enumerate(WANT):
    match = [r for r in rows if re.search(pattern, r["canonical_title"])]
    if not match:
        print("not found:", title)
        continue
    name = f"{i + 1:02d}.mid"
    with open(os.path.join("demos", name), "wb") as f:
        f.write(z.read("maestro-v3.0.0/" + match[0]["midi_filename"]))
    lines.append(f"{name}|{title}|{match[0]['year']} International Piano-e-Competition performance")
with open("demos/demos.txt", "w") as f:
    f.write("\n".join(lines) + "\n")
print(f"{len(lines)} recordings in demos/")
