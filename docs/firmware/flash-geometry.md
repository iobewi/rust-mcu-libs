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

Le second étage est chargé **entièrement en RAM** par la ROM. Sur S3 la fenêtre de code est de **36 Kio**
(`0x403cb000`–`0x403d4000`), la DRAM de 32 Kio (`0x3fce4000`–`0x3fcec000`, au-dessous des données de la ROM à
`0x3fced710`), toutes deux dans `boot_window` (`0x3fcc8000`–`0x3fd00000` sur le bus de données, que les
applications ne chargent jamais). Le script d'édition de liens vérifie la fenêtre, l'alignement des vecteurs
et le non-recouvrement ; `esp_geometry.py` le re-vérifie sur l'ELF produit, avec la même fenêtre.

Mesures sur les ELF liés (octets, console `uart0` par défaut, ESP32-S3 table à `0x10000`, ESP32-C3 à `0x8000`) :

| Mesure | S3 `main` non signé | S3 non signé | S3 vérifié | C3 non signé | C3 vérifié |
|---|---|---|---|---|---|
| image | 29 760 | 29 984 | 34 816 | 23 056 | 27 328 |
| IRAM utilisée / fenêtre | 26 311 / 36 864 | 26 459 / 36 864 | 31 059 / 36 864 | 21 088 / 36 864 | 25 138 / 36 864 |
| IRAM libre | 10 553 (28,6 %) | 10 405 (28,2 %) | 5 805 (15,7 %) | 15 776 | 11 726 |
| DRAM statique / fenêtre | 3 440 / 32 768 | 3 512 / 32 768 | 3 748 / 32 768 | 2 308 / 32 768 | 2 532 / 32 768 |
| région de pile | 29 328 | 29 256 | 29 020 | 30 460 | 30 236 |

Segments S3 vérifié : données `0x3fce4000` (3 748 octets chargés, 32 768 réservés), vecteurs `0x403cb000`
(0x400) et code `0x403cb400` (30 035 octets), image de 34 816 octets. Le profil vérifié ajoute environ 4,6 Kio
de code (SHA-256 et RSA-PSS de la ROM, lecture et jugement de la preuve) à l'IRAM du non signé ; le passage
de `main` au non signé de cette branche ajoute 148 octets de code et 72 octets de données en lecture seule
(notamment l'estampille de géométrie, que `main` n'a pas : l'outil le signale). Le connecteur de console remplace `esp-println` et s'inline : ses symboles
propres pèsent moins de 100 octets, et ne retirent aucun diagnostic.

**Pile.** La région de pile (29 Kio) est la DRAM restante sous le sommet. Borne statique de ce qui est
connu dans le chemin de vérification : environ 1,2 Kio de tampons dans l'admission de la preuve (en-tête,
charge utile alignée, bloc de lecture de 256 octets), 1,2 Kio dans l'adaptateur RSA de la ROM (copies
alignées de la clé de 776 octets et de la signature de 384 octets), un contexte SHA d'une centaine d'octets,
soit environ 3 Kio. **Non déterminable statiquement** : la pile qu'utilisent les routines ROM elles-mêmes.

### Fenêtre IRAM : 36 Kio retenus

Le profil vérifié tient dans 36 Kio avec 5,8 Kio libres : l'extension à 64 Kio n'est pas nécessaire. Une
fenêtre de 64 Kio (`0x403c4000`–`0x403d4000`, qui grandirait vers le bas) a été écrite et mesurée dans une
version précédente de ce dépôt ; elle n'est pas retenue :

* ses adresses de chargement n'ont jamais été chargées par la ROM sur la carte de qualification, alors que celles
  de 36 Kio l'ont été (bootloader non signé de #93 et de P2, et second étage vérifié jusqu'à son point
  d'entrée) : retenir 36 Kio retire une variable de la première qualification ;
* rien ne la justifie aujourd'hui : la croissance prévue (diagnostics de HALT, marqueurs) tient dans 5,8 Kio ;
* un second étage ne dépasse pas 61 440 octets avec la table à `0x10000`, quelle que soit la fenêtre.

Si le profil vérifié dépasse un jour la fenêtre, elle peut grandir vers le bas jusqu'au début de
`boot_window` (`0x403b8000` côté instructions) sans toucher à ce que chargent les applications ; ce
changement demande sa propre qualification matérielle. Le script d'édition de liens garde trois garde-fous
(fenêtre à l'intérieur de `boot_window`, vecteurs en tête de fenêtre, vecteurs alignés sur 1 Kio), en plus du
non-recouvrement IRAM/DRAM.

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

Rien ici n'a été qualifié avec la vérification. Ce qui reste à prouver sur matériel : la durée de démarrage avec
la vérification, la vérification RSA-PSS de la ROM en contexte bootloader, et la pile réellement consommée par
les routines ROM (`verified-boot-qualification.md`). Une première tentative (P3) a chargé le second étage vérifié
jusqu'à son point d'entrée puis est restée muette ; la console du bootloader, alors invisible sur le banc, est
désormais sur UART0.
