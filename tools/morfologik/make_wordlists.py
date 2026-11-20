#!/usr/bin/env python3
"""Word lists for the Morfologik oracle comparisons (see README.md here).

usage: make_wordlists.py <repo root> <output dir> <random typos> <speller list name>

Writes <output dir>/lookup_words.txt, synth_keys.txt and <speller list name>.
The lists are deterministic (fixed seed).
"""
import random
import sys
import unicodedata

random.seed(20261120)
root, out, n_random, speller_name = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]

forms = []
prev = None
with open(root + "/assets/fr/languagetool-pos-dict.txt", encoding="utf-8") as f:
    for line in f:
        form = line.split("\t", 1)[0]
        if form != prev:
            forms.append(form)
            prev = form

# ---------------------------------------------------------------- lookup list
lookup = forms[::160]
lookup += """maison suis Paris pome a à été est l' qu' aujourd'hui c'est-à-dire œuf oeuf MAISON Maison maison_ _ a_b
chevaux États-Unis peut-être 123 1er XIV ° … — 😀 naïve Noël ÉTÉ Été résumé resume là la LA où ou çà ca ça zzz the of und
porte-monnaie arc-en-ciel grand-mère jusqu' presqu'île s' d' -t-il t-il M. etc. etc km kg""".split()
lookup += ["", "  ", "mais on"]
lookup += [w.capitalize() for w in forms[5::3000]]
lookup += [w.upper() for w in forms[7::3000]]
lookup += [w[:-1] for w in forms[11::3000] if len(w) > 2]
with open(out + "/lookup_words.txt", "w", encoding="utf-8") as f:
    f.write("\n".join(lookup) + "\n")

# ------------------------------------------------------ synthesis dictionary
# Keys are "lemma|tag"; take them from the tagger dump, plus keys that do not
# exist.
keys = []
with open(root + "/assets/fr/languagetool-pos-dict.txt", encoding="utf-8") as f:
    for i, line in enumerate(f):
        if i % 900 == 0:
            form, lemma, tag = line.rstrip("\n").split("\t")
            keys.append(lemma + "|" + tag)
keys += ["cheval|N m p", "cheval|N m s", "cheval|N f s", "cheval", "être|V etre ind pres 1 s",
         "aller|V ind futu 3 p", "beau|J m s", "beau|J f s", "beau|J m p", "bigouden|N f p", "|", "", "xyz|N m s",
         "avoir|V avoir ind pres 3 s", "œil|N m p", "ciel|N m p", "aïeul|N m p", "pouvoir|V ind pres 1 s",
         "s'asseoir|V ind pres 1 s", "asseoir|V ind pres 1 s", "payer|V ind pres 1 s"]
with open(out + "/synth_keys.txt", "w", encoding="utf-8") as f:
    f.write("\n".join(keys) + "\n")

# --------------------------------------------------------------- speller list
common = """le la les un une des du de et est sont était être avoir fait faire dit dire peut pouvoir tout tous toute
maison jardin école enfant enfants femme homme temps année jour nuit ville pays monde travail famille
grand petite beau belle nouveau vieille premier dernière bon mauvais heureux français française
aller venir prendre donner parler manger boire dormir écrire lire comprendre apprendre savoir vouloir devoir
aujourd'hui demain hier maintenant toujours jamais souvent parfois beaucoup peu très trop assez
bonjour merci pardon s'il plaît monsieur madame mademoiselle
ordinateur téléphone voiture avion bateau train métro vélo
cœur sœur œuvre œil bœuf nœud vœu fiancée élève hôpital forêt fenêtre garçon leçon façon reçu
château gâteau bâtiment théâtre pâte tâche âge île dîner maître naître paraître connaître
développement gouvernement environnement appartement évidemment apparemment récemment
accueil cueillir orgueil recueil fauteuil écureuil millefeuille
Paris France Marseille Lyon Europe Afrique Jean Marie Pierre Napoléon Molière
anticonstitutionnellement intergouvernemental""".split()

typos = """pome oeufs acceuil parmis language apartement apartemment biensur bien-sur malgrés quelquechose quelque-chose
ormis hormi aujourdhui aujourdui baucoup bocoup beaucoups bonjours bonjoure mersi merçi sil-vous-plait
dévelopement developpement dévellopement gouvernment gouvernemant envirronement enviromement
ecole élephant elephant éléfant téléfone telephone ordinnateur ordinateure voitur voitture
coeur soeur oeuvre oeil boeuf noeud voeu manoeuvre foetus
chateau gateau batiment theatre fenetre foret hopital garcon lecon facon recu
connaitre paraitre maitre diner ile
comprandre aprendre apprandre savoire vouloire devoire pouvoire mangé mangez mangeais mangait
chevals chevaus journals travails bijous genous hibous
je suit tu il nous somme vous ète ils on
occurence occurrance ocurrence professionel profesionnel traditionel personel personnelle
adresse addresse aggressif agressif appeler apeler appeller rappeller
dilemne dilemme etymologie ethymologie rythme rhytme rithme hypothèse hipothèse
exemple example exemples exempel connexion connection
intéressant interressant interesant interéssant
necessaire nécéssaire nécessaire neccessaire
definitivement définitivement définitivment définitivemment
malheuresement malheureusement malheureusment heureusment
peut-etre peutetre peut-être peu-être
quand quant qu'en quan cand
ortographe orthographe ortografe orthografe
fotographie photographie filosofie philosophie farmacie pharmacie
kilometre kilomètre kilomettre
payer paier payé paye paie essuyer essuier
vraiement vraiment gentiement gentiment
tranquile tranquille tranquil
longtemp longtemps lontemps
plusieur plusieurs plusieures
chaqu'un chacun chaqun chacuns
ceci-dit cela-dit celà cela
voila voilà viola
deja déjà dèja dejà
tres trés
apres après aprés
evenement événement évènement évenement
pere père pére mere mère mére frere frère frére
etudiant étudiant etudient étudient
francais françai françaises francaises
reçut recevoire
aquis acquis aquérir acquérir
exellent excellent excelent
bizare bizarre bizzare
courir courrir mourir mourrir nourir nourrir
abréviation abbréviation
atention attention attension
balade ballade
cauchemard cauchemar
committé comité
enveloppe envelope
erreure erreur ereur
langage langagee
mmaison maisonn maiso maisn misaon amison maisno mzison
lamaison unemaison grandemaison mapetite jesuis cest cestadire parceque parcequ puisque lorsqu jusqua
tabl tablle tabel tbale table tablee
qqch tjrs bcp pk pq svp stp mdr lol
iphone iPhone eBay YouTube McDonald LaTeX JavaScript OpenOffice
ètre etre ëtre êtr
àpres apès
shampoing shampooing week-end weekend e-mail email mél courriel
aéroport aeroport aréoport
bibliothèque bibliothéque biblioteque bibliotèque
grammaire gramaire grammère
conjugaison conjuguaison conjugaisson
orthographique ortographique
dictionnaire dictionaire dictionnère
synthétiseur sintétiseur
xylophone xilophone zylophone
wagon vagon
quelqu'un quelqun quelqu'une quelquun
jusqu'à jusqu'a jusque
presqu'île presquile presqu'ile
d'accord daccord dacord d'acord
l'école lécole l'ecole
n'importe nimporte
c'est-à-dire cestàdire""".split()
typos += ["peu etre", "a b", "bien sur"]

letters = "abcdefghijklmnopqrstuvwxyzéèàùçêîôûœ'-"
pool = [w for w in forms[3::47] if 4 <= len(w) <= 14 and w.isalpha()]
random.shuffle(pool)


def mutate(w):
    kind = random.randrange(6)
    i = random.randrange(len(w))
    if kind == 0:  # deletion
        return w[:i] + w[i + 1:]
    if kind == 1:  # insertion
        return w[:i] + random.choice(letters) + w[i:]
    if kind == 2:  # substitution
        return w[:i] + random.choice(letters) + w[i + 1:]
    if kind == 3 and i + 1 < len(w):  # transposition
        return w[:i] + w[i + 1] + w[i] + w[i + 2:]
    if kind == 4:  # accents removed
        return "".join(c for c in unicodedata.normalize("NFD", w) if not unicodedata.combining(c))
    return w[:i] + w[i] + w[i:]  # doubled letter


rnd = [mutate(w) for w in pool[:n_random]]
rnd += [mutate(mutate(w)) for w in pool[n_random:n_random + n_random // 4]]
correct = pool[2 * n_random:2 * n_random + n_random // 2]
base = common + typos + rnd + correct
caps = [w.capitalize() for w in (common[::3] + typos[::3] + rnd[::5])]
upper = [w.upper() for w in (common[1::4] + typos[1::4] + rnd[2::7])]
mixed = """mAison MaiSon mAISON PoMe pOME éCOLE École ÉCOLE Ecole ECOLE école PARIS paris france FRANCE jean JEAN
Acceuil ACCEUIL Oeufs OEUFS ŒUFS Œufs Parmis PARMIS Pome POME GrandÉléphant GrandeMaison grandeMaison""".split()
edge = """a b à y é z 1 12 123 1er 2e 3ème abc123 A4 B2B x86 ! ? . , … « » - ' ’ -- ...
l' d' qu' l’école aujourd’hui c’est _ a_b maison_ maison_N $ € % °C @ #hashtag http www
😀 maison😀 日本 日本語 Москва москва Ελλάδα straße Straße STRASSE İstanbul ǅ ﬁn Ⅷ ⅷ ² ½
cœur coeur COEUR Coeur CŒUR""".split()
edge += ["a" * 55, "anticonstitutionnellement" * 2, "x" * 119, "x" * 120, "x" * 121, "maison" * 20,
         "", " ", "  ", "a ", " a", " ", "école", "ecolé"]

seen = set()
words = []
for w in base + caps + upper + mixed + edge:
    if w not in seen and not any(c in w for c in "\n\t\r"):
        seen.add(w)
        words.append(w)
with open(out + "/" + speller_name, "w", encoding="utf-8") as f:
    f.write("\n".join(words) + "\n")
print(len(lookup), "lookup words,", len(keys), "synthesis keys,", len(words), "speller words")
