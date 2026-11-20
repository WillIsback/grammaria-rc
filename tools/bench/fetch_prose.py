#!/usr/bin/env python3
"""Fetch well-edited French prose to measure false alarms on.

    python3 tools/bench/fetch_prose.py [OUT_DIR]      (default eval/prose)

Writes OUT_DIR/wiki.txt (encyclopaedic prose from French Wikipedia) and
OUT_DIR/litt.txt (19th-century novels from French Wikisource), one paragraph
per line. The texts are not part of the repository (CC BY-SA / public
domain, fetched on demand); the page list below makes the set reproducible
up to later edits of the pages.
"""
import html
import os
import re
import sys
import time
import urllib.parse
import urllib.request

UA = "prose-bench-fetch/0.1 (offline grammar benchmark; low volume)"
WIKI = """Paris|Révolution française|Victor Hugo|Marie Curie|Système solaire|Photosynthèse|Seconde Guerre mondiale|Molière|Louis XIV|Mont Blanc
Loire|Bretagne|Jazz|Impressionnisme|Cinéma|Football|Tour de France|Vin|Fromage|Abeille|Loup|Chêne|Volcan|Climat|Océan Atlantique|Lune
Mars (planète)|Albert Einstein|Charles de Gaulle|Napoléon Ier|Jeanne d'Arc|Cathédrale Notre-Dame de Paris|Château de Versailles
Musée du Louvre|Internet|Ordinateur|Vaccin|Cœur|Mathématiques|Philosophie|Tour Eiffel|Lyon|Marseille|Québec|Belgique|Suisse|Sénégal
Chocolat|Pain|Bicyclette|Chemin de fer|Imprimerie|Électricité|Eau|Forêt|Agriculture|Musique|Peinture|Théâtre|Roman (littérature)""".replace("\n", "|").split("|")
LITT = (
    [f"Le Tour du monde en quatre-vingts jours/Chapitre {n}" for n in range(1, 21)]
    + [f"Vingt mille lieues sous les mers/Partie 1/Chapitre {n}" for n in range(1, 11)]
    + [f"L’Île mystérieuse/Partie 1/Chapitre {n}" for n in range(1, 9)]
    + ["Boule de Suif", "La Parure", "Le Horla (recueil)/Le Horla", "Un cœur simple"]
)


def fetch(site, title):
    url = f"https://{site}/wiki/" + urllib.parse.quote(title.replace(" ", "_"))
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=40) as r:
            return r.read().decode("utf-8", "replace")
    except Exception as e:  # a missing page is skipped
        print(f"  skipped {title}: {e}", file=sys.stderr)
        return ""


def paragraphs(page):
    page = re.sub(r"<(style|script)\b.*?</\1>", "", page, flags=re.S)
    out = []
    for m in re.finditer(r"<p\b[^>]*>(.*?)</p>", page, flags=re.S):
        p = m.group(1)
        p = re.sub(r"<sup\b.*?</sup>", "", p, flags=re.S)        # note calls
        p = re.sub(r"<span class=\"(mw-editsection|pagenum|indicateur-langue)[^>]*>.*?</span>", "", p, flags=re.S)
        p = re.sub(r"<br\s*/?>", " ", p)
        p = html.unescape(re.sub(r"<[^>]+>", "", p))
        p = re.sub(r"[ \t\r\n]+", " ", p).strip()
        # Keep running prose: long enough, ends like a sentence, no leftovers.
        if len(p.split()) >= 15 and p[-1] in ".!?…»" and "[" not in p and "{" not in p and "↑" not in p:
            out.append(p)
    return out


def build(site, titles, path):
    count = words = 0
    with open(path, "w", encoding="utf-8") as f:
        for t in titles:
            for p in paragraphs(fetch(site, t)):
                f.write(p.replace("\\", "\\\\") + "\n")
                count += 1
                words += len(p.split())
            time.sleep(1.0)
    print(f"{path}: {count} paragraphs, {words} words")


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), "eval", "prose")
    os.makedirs(out, exist_ok=True)
    build("fr.wikipedia.org", WIKI, os.path.join(out, "wiki.txt"))
    build("fr.wikisource.org", LITT, os.path.join(out, "litt.txt"))
