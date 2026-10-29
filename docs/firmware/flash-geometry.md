# Géométrie flash du second étage (#91, phase B2)

La table de partitions existe pour découpler les offsets des mécanismes de démarrage et de mise à
jour. Ce document dit où chaque adresse est écrite, comment on la change, et ce que chaque géométrie
autorise.

## Une seule valeur

`ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET` (défaut `0x8000`) est lue

* par `esp-bootloader-esp-idf`, donc par **l'application** (elle y lit sa table) ;
* par `iobewi-esp-partitions` (`geometry::TABLE_OFFSET`), donc par le **bootloader**, par la
  découverte des partitions et par la région réservée de `config` ;
* par les outils (`esp_geometry.py`, `prepare.py`) pour le générateur de table, les limites de
  taille et les contrôles.

Elle est acceptée en hexadécimal ou en décimal ; une valeur qui n'est pas un multiple de 4 Kio, ou
hors de `0x8000`–`0x1b000`, fait échouer la compilation. Les contrats portables
(`plan_boot`, OTM1, OTSF, IAM1, IAP1, la découverte du domaine A) ne contiennent aucune adresse ESP32.

Une incohérence échoue de façon sûre, à quatre endroits : à la compilation (valeur invalide), à
l'empaquetage (fichier de partitions qui déclare un autre offset, bootloader ou application dont
l'estampille `IOBEWI-GEOMETRY table=0x…` diffère), au démarrage (un bootloader qui lit la table
ailleurs ne trouve pas de table valide : HALT), et dans l'application (table illisible).

## Les deux géométries (ESP32-S3, 16 Mio)

| Région | Historique | Large |
|---|---|---|
| second étage | `0x0`–`0x8000` | `0x0`–`0x10000` |
| table | `0x8000` | `0x10000` |
| `otadata` | `0xd000` (8 Kio) | `0x11000` (8 Kio) |
| `ota_meta` | `0xf000` (8 Kio) | `0x13000` (8 Kio) |
| libre | `0x11000`–`0x20000` | `0x15000`–`0x20000` |
| `ota_0` | `0x20000` (`0x1b0000`) | idem |
| `ota_1` | `0x1d0000` (`0x1b0000`) | idem |
| `config` | `0x380000` (`0x80000`) | idem |
| preuves IAP1 | `0x1cf000`, `0x37f000` | idem |

Fichiers versionnés : `examples/firmware/ota-system/partitions-s3-16mb-legacy.csv` (la table de la
carte de qualification, qui n'était auparavant que dans un script jeté) et `…-wide.csv`. Chaque fichier
déclare `# table-offset:` et `# flash-size:` ; les trois tables 4 Mio existantes ont reçu le même en-tête.
Les slots, `config` et les secteurs de preuve ne bougent pas.

**ESP32-C3 : géométrie historique uniquement dans cette phase.** Sa fenêtre de RAM du bootloader
coïncide avec le début de `boot_window` : elle ne peut pas grandir sans réduire la RAM des applications.
Son second étage vérifié tient (voir plus bas). `esp_geometry.py` refuse C3 avec une autre table.

## Ce qui est imposé, ce qui est choisi

* **Matériel, ROM, format ESP-IDF** : applications alignées sur 64 Kio, données sur 4 Kio, secteur de
  table de 4 Kio (0xC00 octets utiles), `otadata` d'au moins 8 Kio, secteur de preuve de 4 Kio, fenêtres de
  SRAM du SoC.
* **Paramètre de plateforme** : l'offset de la table (ci-dessus) et la fenêtre de RAM du second étage
  (scripts d'édition de liens).
* **Découvert dans la table** : offsets et tailles de `ota_0`, `ota_1`, `otadata`, `config`.
* **Supprimé** : le `0x9000` de la découverte de boot, de la découverte de stockage et de la région
  réservée de `config`, la constante `0x8000` du bootloader, les offsets `20000`/`1a0000`/`180000` de
  `prepare.py`.

## Capacité du second étage

| | Historique | Large |
|---|---|---|
| région avant la table | 32 768 | 65 536 |
| limite dure (CI bloquante) | 32 768 | **61 440** : un secteur de signature de 4 Kio reste libre |
| budget non signé | S3 30 720 · C3 23 744 | 32 768 |
| budget vérifié | S3 : ne tient pas (rapport) · C3 32 256 | 40 960 |

Secure Boot V2 (non implémenté, non qualifié) ajoute à l'image un secteur de signature de 4 Kio sur une
frontière de 4 Kio et plafonne l'image à 64 Kio, signature exclue (documentation ESP-IDF v6.1 de
l'ESP32-S3). Avec la table à `0x10000`, la signature tient devant la table pour une image jusqu'à
61 440 octets. Une image de 64 Kio plus sa signature demanderait une table à `0x11000` ou au-delà ; ce
n'est pas nécessaire pour la taille actuelle.

## Mémoire du second étage

Le second étage est chargé **entièrement en RAM** par la ROM. Sur S3 la fenêtre de code passe de 36 à
64 Kio (`0x403c4000`–`0x403d4000`) en grandissant vers le bas ; elle reste dans `boot_window`
(`0x3fcc8000`–`0x3fd00000` sur le bus de données, que les applications ne chargent jamais), et la DRAM
(`0x3fce4000`, 32 Kio) ne bouge pas. Le script d'édition de liens vérifie la fenêtre, l'alignement des
vecteurs et le non-recouvrement ; `esp_geometry.py` le re-vérifie sur l'ELF produit.

| Mesure (octets) | S3 non signé | S3 vérifié | C3 non signé | C3 vérifié |
|---|---|---|---|---|
| image | 30 016 | 34 864 | 23 008 | 27 296 |
| IRAM utilisée / fenêtre | 26 535 / 65 536 | 30 663 / 65 536 | 21 074 / 36 864 | 24 806 / 36 864 |
| DRAM statique / fenêtre | 3 484 / 32 768 | 3 692 / 32 768 | 2 280 / 32 768 | 2 476 / 32 768 |
| pile disponible | 29 284 | 29 076 | 30 488 | 30 292 |

Segments S3 : `0x3fce4000` (données, 0xd9c octets chargés, 32 Kio réservés), vecteurs
`0x403c4000` (0x400), code `0x403c4400`. L'image est identique dans les deux géométries à la constante de
la table près : la géométrie ne change que ce que le bootloader lit et ce que la CI autorise.

### Fenêtre IRAM : 64 Kio ou l'ancienne de 36 Kio ?

Le code vérifié actuel (30,7 Kio) **tient dans l'ancienne fenêtre** (`0x403cb000`–`0x403d4000`) : mesuré, le
bootloader S3 vérifié se lie à l'identique (même taille de fichier, IRAM 30 663 sur 36 864, 5,5 Kio libres).
L'extension à 64 Kio n'est donc pas indispensable à cette version ; elle ne sert qu'à la croissance.

| | Anciennes adresses (36 Kio) | Fenêtre de 64 Kio |
|---|---|---|
| ce que la ROM charge | mêmes adresses que le bootloader qualifié (#93) : seule la taille du fichier change | une fenêtre jamais chargée par la ROM sur cette carte : une variable de plus |
| risque de la première qualification | réduit : un seul changement de géométrie (la table) | ajoute « la ROM accepte-t-elle ces adresses ? » |
| marge de croissance du code | 5,5 Kio d'IRAM (14 %) | 34 Kio |
| budget de dérive CI (verifié 40 960) | non atteignable : IRAM + DRAM ≈ 40 Kio | cohérent |
| suite | une 2e qualification matérielle le jour où l'IRAM manque | aucune |

Recommandation : qualifier d'abord avec les anciennes adresses, puis étendre quand l'IRAM manque, avec sa
propre qualification. Le changement se limite à deux lignes du script d'édition de liens
(`vectors_seg`/`IRAM` : `0x403cb000`/`0x403cb400`, longueurs `0x400`/`0x9000 - 0x400`) ; les deux variantes
des quatre bootloaders sont fabriquées et empreintes (jeu de qualification), la CI construit la variante
actuelle. L'état du dépôt reste la fenêtre de 64 Kio tant que cette décision n'est pas prise.

## Construire, vérifier, empaqueter

```sh
# Géométrie large : tous les binaires avec la même valeur.
export ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET=0x10000
python3 examples/firmware/ota-system/prepare.py --chip esp32s3 --flash-size 16MB \
  --partition-tool /chemin/esp-idf/components/partition_table/gen_esp32part.py
python3 .github/scripts/esp_geometry.py csv examples/firmware/ota-system/partitions-s3-16mb-wide.csv
```

Sans la variable, tout reste comme avant (table à `0x8000`). `prepare.py` refuse un bootloader qui
dépasse la table, une application estampillée pour un autre offset et un fichier de partitions
incohérent, et écrit `flash-plan.txt` (offset, fichier, taille, SHA-256).

## Limites

Rien ici n'a été installé sur une carte. Ce qui reste à prouver sur matériel : que la ROM charge le
second étage dans la fenêtre agrandie, la durée de démarrage avec la vérification, et la vérification
RSA-PSS de la ROM en contexte bootloader (`verified-boot-qualification.md`).
