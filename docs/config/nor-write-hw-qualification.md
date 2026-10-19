# Qualification ESP32-S3 — `EspFlash::write` en sémantique NOR (StreamBeWI #19)

## Périmètre et interdits

ESP32-S3 DevKitC-1, lecteur **Metronic 477144 exclusivement** (pas le Sony).
Compare **A** = `f3de781^` (`FlashStorage::write`, lecture-modification-effacement-écriture)
à **B** = `f3de781` (`write_nor`). Ce document est un protocole : rien n'a été exécuté.
Aucun flashage, essai matériel ou coupure d'alimentation sans autorisation explicite.
Le changement de station par détachement USB (StreamBeWI PR #20) n'est pas modifié.
Une baisse des durées d'écriture ne suffit pas à déclarer #19 résolue : la partie C est un critère à part.

Noter pour chaque essai : commit, SHA256 de l'image, chip, capacité flash, toolchains, commande de flash,
logs. Aucun identifiant Wi-Fi, Bearer ou clé TLS dans les rapports.

## Conditions communes (A/B à charge égale)

- Deux images ne différant que par la ligne de `EspFlash::write`, même toolchain, même profil `--release`, même fréquence.
- Départ identique : `erase-flash`, même table de partitions, même image de catalogue. Même graine EKV (`Config::random_seed`).
- Même script de charge, mêmes clés et valeurs, même ordre. Comparer au même état de pages : mesurer d'abord
  sur partition vierge, puis après remplissage à un taux fixé (par ex. 0 %, 50 %, 80 %) pour exposer la compaction.
- Au moins 5 répétitions par couple (image, charge). Cartes et câbles inchangés.
- Instrumentation hors de la section critique : l'enveloppe de mesure (build de qualification uniquement,
  non livré) entoure `NorFlash::write/erase` dans `EspFlash`, lit l'horloge µs avant et après l'appel,
  et n'écrit que dans des compteurs et un tampon circulaire en RAM (taille, durée, type). Aucun `log!`
  ni I/O pendant l'appel ni sous le verrou ; le tampon est vidé par une tâche basse priorité, hors commit mesuré.
  Vérifier que l'enveloppe ajoute moins de 2 µs en la mesurant sur un appel à vide.

## A — Performance flash

| Essai | Mesure |
| --- | --- |
| A1 WRITE de 4, 16, 64, 244, 1024 octets, offsets alignés sur 4, dans un secteur déjà effacé, 200 appels par taille, à des offsets croissants | min, moyenne, médiane, max, p99 par taille ; nombre de WRITE et d'ERASE physiques (attendu B : 0 ERASE) |
| A2 Commit EKV de n écritures (n = 1, 10, 17, 50, 143) | durée totale du commit, nombre de WRITE et d'ERASE, durée max d'un WRITE |
| A3 Remplissage jusqu'à compaction (EKV rempli à 80 % puis écritures répétées) | idem A2 sur le commit qui déclenche la compaction ; ERASE attendus = effacements légitimes d'EKV seulement |
| A4 Série de 100 commits identiques à état de départ restauré | dispersion entre répétitions |

PASS : sur B, ERASE = effacements demandés par EKV (A1 : 0) ; WRITE conservés en nombre (même charge logique) ;
durée médiane et max d'un WRITE ≤ A pour chaque taille ; aucun WRITE > 5 ms pour ≤ 244 octets (seuil indicatif à confirmer
sur la mesure A, ce n'est pas une garantie de pire cas). FAIL : un ERASE hors EKV/OTA, ou un nombre de WRITE différent de A à charge égale.

## B — Intégrité

| Essai | Attendu |
| --- | --- |
| B1 Programmer 4, 8, 16 octets successifs dans un même secteur effacé, relire après chaque | chaque relecture = données écrites ; octets voisins inchangés (motif témoin 0xA5 écrit avant) ; 0 ERASE |
| B2 Écriture non alignée (offset ou longueur non multiple de 4) | `NotAligned`, flash inchangée (relecture du secteur) |
| B3 Reprogrammation d'un mot déjà programmé (bits 0→1 impossibles) | résultat = AND des deux valeurs, sans effacement ; documenté comme comportement NOR attendu |
| B4 Transactions EKV : écrire, supprimer, commit, reset matériel (bouton EN, après fin des opérations), relire tout le catalogue | catalogue identique, génération cohérente, aucune clé perdue |
| B5 Séquence A→B de l'OTA A : écriture d'artefact, `otadata` (corps puis mot de commit), activation, confirmation, reset | slot actif correct, SHA256 de l'artefact relu identique ; pas de régression par rapport à A |
| B6 Relecture flash par `bench` (`benchctl.py dump`) avant/après avec comparaison SHA256 hors zones attendues | aucune différence hors des partitions visées |

PASS : tous les attendus, trois répétitions. Toute différence est un FAIL à investiguer avant la suite.

**Coupures d'alimentation (non couvert, non autorisé ici).** L'exemple hôte
`cargo run -p iobewi-config-space --example qualify_ekv -- cuts` échoue sur 32 cas / 1972
(`mount: Corrupted`), indépendamment de cet adaptateur ; c'est le défaut de reprise EKV suivi dans #70.
`write_nor` supprime l'effacement implicite sous chaque WRITE, il ne corrige pas #70. Toute campagne de
coupures exige une autorisation explicite et un plan séparé.

## C — Continuité USB MSC (StreamBeWI, Metronic 477144)

Firmware StreamBeWI identique entre A et B sauf la dépendance `iobewi-esp-flash`. Lecture continue d'un fichier de
référence sur l'hôte USB, avec comptage des octets lus et de l'horodatage de chaque bloc. Relever pour chaque essai : resets
du bus USB, ouvertures/fermetures de session MSC, compteurs `Pending`, `ZEROFILL`, plus long intervalle sans donnée.

| Essai | Action pendant la lecture |
| --- | --- |
| C1 | création d'un favori |
| C2 | modification d'un favori inactif |
| C3 | suppression d'un favori inactif |
| C4 | dix opérations successives (créer, modifier, supprimer en alternance) |
| C5 | redémarrage de la carte après C4, relecture du catalogue |

Répéter chaque essai 5 fois sur A puis 5 fois sur B, dans cet ordre, avec la même position dans le fichier au déclenchement.
PASS : sur B, 0 reset USB, 0 nouvelle session MSC, `ZEROFILL` = 0 et `Pending` inchangé sur toutes les répétitions,
contenu lu identique octet par octet, catalogue persistant après C5. FAIL : au moins un reset ou une session perdue sur B.
Si B est meilleur que A mais non nul, #19 reste ouverte : consigner la fréquence et la durée du plus long WRITE à l'origine.

## Points de vigilance

- Les durées sont des valeurs observées, pas un pire cas ; le README du crate rappelle que les ROM calls masquent les interruptions.
- Un effacement de secteur d'EKV (légitime) peut encore dépasser 30 ms et affecter l'USB : l'attribuer à EKV, pas au correctif.
- Ne pas comparer deux états de pages EKV différents (compaction).
- L'instrumentation ne doit pas tourner pendant la partie C si elle perturbe le timing ; refaire C sans enveloppe.
- `read()` n'est pas modifié dans ce chantier ; sa lecture à tampon de 4 Kio sur la pile reste hors périmètre.
