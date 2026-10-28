# Stockage des métadonnées OTA (`ota_meta`) et reprovisionnement

Les métadonnées OTA (OTM1 : candidat `Written`/`Activating`, identité du dernier
déploiement confirmé) ne vivent plus dans ConfigManager/EKV. Elles ont leur
partition et leur cycle de vie. Ce document décrit le format, les états lus au
démarrage et la procédure de reprovisionnement des cartes de développement.

## Ce qui est séparé

| Donnée | Propriétaire | Partition |
|---|---|---|
| Sélection de boot, probation, rollback | bootloader + application (`BOOT` v2) | `otadata` (`0xd000`, `0x2000`) |
| Transaction OTA (OTM1) | moteur OTA | **`ota_meta`** (`0xf000`, `0x2000`) |
| Paramètres applicatifs (Wi-Fi…) | ConfigManager / EKV | `config` |

Réinitialiser ou perdre `config` n'efface pas l'état OTA. Perdre `ota_meta` ne
change pas le slot démarré (c'est `otadata` qui décide) : au pire le candidat
`Written`/`Activating` est abandonné et l'identité du dernier déploiement
confirmé est oubliée. Un candidat **en probation** pendant cette perte n'est
jamais confirmé : `reconcile` le déclare non expliqué et il est rejeté (rollback
vers le dernier firmware confirmé).

Le bootloader ne lit pas `ota_meta` et ne décode pas OTM1.

## Partition

`ota_meta, data, 0x41, 0xf000, 0x2000` : deux unités d'effacement consécutives
(4 Kio), une copie chacune. L'adresse `0xf000` est la première libre après
`otadata` sur la table de qualification (`0xd000` + `0x2000`) et reste avant
`ota_0` (`0x20000`). Elle est découverte par son **label** (sous-type `0x41`),
la table complète étant validée comme pour les autres partitions : aucune
adresse n'est codée en dur dans le firmware.

## Trame

OTM1 est conservé tel quel (au plus 512 octets). Une enveloppe lui apporte ce
qu'il n'a pas : détection d'écriture déchirée et choix de la copie récente.

| Offset | Taille | Champ |
|---|---|---|
| 0 | 4 | magic `OTSF` |
| 4 | 1 | version de trame, `1` |
| 5 | 1 | réservé, `0` |
| 6 | 2 | longueur de la charge utile |
| 8 | 4 | séquence (arithmétique sérielle, tolère le bouclage) |
| 12 | longueur arrondie à 4 | charge utile OTM1, complétée par des zéros |
| fin | 4 | CRC-32 (zlib) de tout ce qui précède |

Le CRC détecte une écriture ou un effacement interrompus. Il n'authentifie pas
l'enregistrement (voir #90/#91).

## Protocole de commit

1. Lire les deux copies et choisir la plus récente valide.
2. Écrire dans **l'autre** copie : effacer l'unité, vérifier qu'elle est vierge,
   programmer la trame, relire et comparer.
3. La copie précédente n'est pas touchée : quel que soit l'instant de la coupure,
   la lecture suivante renvoie l'ancien enregistrement ou le nouveau.

## Lecture au démarrage

| Contenu des deux copies | Résultat | Effet |
|---|---|---|
| deux unités entièrement vierges | `None` (rien n'a jamais été publié) | état OTA par défaut |
| au moins une trame valide | la plus récente | l'autre copie, invalide ou vierge, est réparée au commit suivant |
| ni vierge ni aucune trame valide (déchirée, corrompue, autre format) | **erreur** | jamais réinitialisé automatiquement ; un commit est refusé |

Dans le cas d'erreur, le démarrage traite OTM1 comme absent (un candidat en
probation est rejeté) mais **aucun nouvel upload ni activation** n'est accepté
tant que la région n'est pas reprovisionnée. Le cas le plus probable est une
coupure d'alimentation pendant le tout premier commit d'un `ota_meta` vierge ; il
disparaît si `ota_meta` est publié en usine avec une première trame ou si on
accepte le reprovisionnement ci-dessous.

## Reprovisionner `ota_meta`

Seulement quand l'état de boot est **confirmé** : le journal indique
`boot: running ota_<n> status Stable` et `otadata` porte l'entrée `Valid` du
slot en cours, sans candidat `Pending`. Sinon, attendre la fin de la probation.

```sh
python3 -m esptool --chip esp32s3 read-flash 0xd000 0x2000 otadata-avant.bin   # sauvegarde
head -c 8192 /dev/zero | tr '\0' '\377' > ota_meta-vierge.bin
python3 -m esptool --chip esp32s3 write-flash 0xf000 ota_meta-vierge.bin
```

Effet : transaction en cours oubliée, identité du dernier déploiement oubliée,
slot démarré inchangé. Relire `0xf000` : 8 192 octets à `0xff`.

## Cartes de développement existantes (migration par réinitialisation)

Il n'y a pas de migration des anciens enregistrements OTM1 stockés dans EKV, et
aucun double backend. Les anciennes cartes ont besoin de :

1. **Table des partitions** avec `ota_meta` (voir les `partitions.csv` des
   exemples, générés par `prepare.py`). Une table plus ancienne fait échouer la
   découverte de `ota_meta` au démarrage. Les partitions inconnues sont tolérées
   par le bootloader : il n'est pas modifié.
2. **`config` remise à zéro.** Le nouveau firmware ne déclare plus l'espace
   `ota` ; une `config` qui le contient est refusée au montage (`InvalidSpace`).
   Le Wi-Fi est ré-amorcé par les identifiants de compilation.
3. **`ota_meta` vierge.**

Installation d'usine complète, sans rebooter entre les écritures (sauvegarde
préalable obligatoire, voir [le reprovisionnement du boot](boot-metadata-reprovisioning.md)) :

```sh
python3 -m esptool --chip esp32s3 write-flash \
  0x8000 partitions.bin 0xd000 otadata.bin 0xf000 ota_meta.bin \
  0x20000 app.bin 0x3c0000 config.bin
```

`prepare.py` produit ces fichiers. Avant tout effacement : lire `otadata` et
vérifier qu'un firmware confirmé existe ; un `otadata` produit par `prepare.py`
déclare `ota_0` confirmé.

## Échec de la configuration

La configuration (Wi-Fi) est montée **après** `ota_meta`, et son échec n'empêche
plus la décision de boot :

* image en probation, configuration non montée ou non saine : l'auto-vérification
  échoue, l'image est rejetée puis le bootloader revient au dernier firmware
  confirmé ;
* image déjà confirmée, configuration non montée : l'erreur est journalisée, le
  Wi-Fi et le service OTA ne démarrent pas ; `ota_meta` et `otadata` restent
  intacts.

Une application dont la configuration est invalide n'est donc jamais confirmée
automatiquement.

## Qualification

* Hôte : la trame et le store sont testés avec une coupure simulée à **chaque
  octet** d'effacement et de programmation (deux modèles d'effacement
  interrompu), plus le cycle publication → activation → confirmation avec coupure
  à chaque octet flash du cycle complet. Voir
  `common/firmware/update/src/frame_store.rs` et `nor_frames.rs`.
* Matériel : à qualifier sur ESP32-S3 après accord (table des partitions
  incluse). Les coupures physiques réelles ne sont pas couvertes par cette
  campagne.
