# Premier démarrage vérifié sur matériel : protocole minimal proposé (#91)

**Rien de ce document n'a été exécuté.** Chaque étape d'écriture exige une autorisation distincte ; arrêt à la
première divergence. Il se lit avec `geometry-reprovisioning.md` (comment fabriquer et écrire) et
`flash-geometry.md` (la disposition). Les campagnes de corruption, de rollback et de coupures ne font **pas**
partie de ce premier essai.

## Ce que l'essai doit établir

1. la ROM charge le second étage (plus de 32 Kio) et celui-ci trouve la table à `0x10000` ;
2. le second étage vérifié accepte une image portant sa preuve, et le temps que cela coûte au démarrage ;
3. l'application signée en RSA met à jour le slot inactif par OTA **en écrivant elle-même la preuve**, et le
   second étage vérifié démarre la nouvelle image ;
4. la `config`, les métadonnées et le Wi-Fi sont intacts.

## Jeu d'artefacts

Construit par `prepare.py --flash-size 16MB --auth signed-rsa --rsa-key KEY --boot {unsigned,verified}` avec
`ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET=0x10000`. Les fichiers contiennent des identifiants
(Wi-Fi, jeton, clé TLS) : ils restent hors Git (`0600`) ; leurs SHA-256 sont dans un manifeste du jeu, à
relever dans le compte rendu de la campagne.

| Fichier | Rôle |
|---|---|
| `boot/unsigned-iram{64,36}.bin` | second étage non signé (première écriture) |
| `boot/verified-iram{64,36}.bin` | second étage vérifié, clé épinglée du banc |
| `A/app.bin`, `A/app.iap1`, `A/app.env` | image A (installée à la main dans `ota_0`), sa preuve, son enveloppe |
| `B/app.bin`, `B/app.env` | image B (envoyée par OTA), enveloppe pour `X-Image-Manifest` |
| `B/app.iap1` | la preuve que l'appareil doit écrire lui-même : sert de référence de comparaison |
| `A/partitions.bin`, `otadata.bin`, `ota_meta.bin` | table large, `otadata` et `ota_meta` d'usine |

**Politique de confiance** : algorithme RSA-PSS 3072 / SHA-256, clé de banc dédiée (PKCS#8, hors Git), épinglée
dans l'application (`IOBEWI_OTA_SIGNING_RSA_KEY`) et dans le bootloader (SHA-256 de son encodage, 64 hex).
Produit `iobewi-ota-sys`, MCU `esp32s3`, variante `0`, version de sécurité minimale `0`. Les deux images
font la même taille et ne diffèrent que par l'identité TLS. `iap1_pack verify` a confirmé que A+sa preuve et
B+sa preuve sont acceptées par la logique du bootloader, et que A+preuve de B, une image modifiée d'un octet
et une mauvaise clé épinglée sont refusées.

`iram64` est l'état du dépôt ; `iram36` utilise les anciennes adresses de chargement (voir `flash-geometry.md`).
La première écriture utilise la variante décidée ; l'autre sert de repli sans reconstruire.

## Référence du banc, géométrie large

Après le reprovisionnement : second étage `0x0`–`0x10000` ; table `0x10000` ; `otadata` `0x11000` (Valid,
`ota_0`, séquence 1) ; `ota_meta` `0x13000` (trame d'usine) ; `ota_0` `0x20000` = A ; preuve `0x1cf000` ;
`config` `0x380000` inchangée. Les SHA-256 de ces régions, relues, forment la nouvelle référence : `p91-wide`.

## Étapes

**P0, lecture seule.** Identité (MAC, 16 Mio), `/health` 200 ; sauvegardes et SHA-256 de `0x0`–`0x9000`,
`otadata`, `ota_meta`, `0x11000`–`0x20000` (**vierge**), les deux slots, `config`.

**P1, preuve de récupération.** Entrée en mode téléchargement ROM par le banc (EN et IO0 par le pont UART),
réécriture à l'identique du second étage actuel, démarrage de l'ancien firmware. Sans ce succès, rien n'est
écrit. *L'entrée en téléchargement ROM est exercée à chaque opération du banc et ne dépend pas du contenu de la
flash ; ce qui n'est pas encore démontré, c'est la réécriture de `0x0` après un second étage qui ne démarre
pas.* Aucun test destructif d'effacement du second étage ici.

**P2, reprovisionnement avec le second étage non signé.** Ordre et attendus : `geometry-reprovisioning.md` R3.
Image A dans `ota_0` avec sa preuve (écrite à la main). Journal attendu :
`boot: table at 0x00010000 otadata at 0x00011000`, `boot authentication: UNSIGNED`, `running ota_0 status Stable`,
`OTA authentication: SIGNATURE_REQUIRED`, Wi-Fi (config conservée), `OTA ready`, `/health` 200. Si la ROM ne charge
pas ce second étage, retour au mode téléchargement et restauration (P1).

**P3, second étage vérifié.** Écrire `boot/verified-iram*.bin` à `0x0` ; un seul reset. Attendus :
`boot authentication: VERIFIED`, `boot: slots checked, microseconds N` (**le coût de la vérification à chaque
démarrage**, à relever), `running ota_0 status Stable`, application fonctionnelle. Un diagnostic ROM sans
journal applicatif, ou un HALT, arrête l'essai : revenir au second étage non signé (P2) sans toucher aux
régions.

**P4, mise à jour OTA complète.** `client.py` envoie `B/app.bin` avec `--manifest B/app.env` (HTTPS, TLS
vérifié), puis active avec la même enveloppe. Attendus : `ota: writing …`, activation, redémarrage en
`PendingVerify` puis confirmation, `running ota_1 status Stable` sous `VERIFIED`, et **le secteur de preuve
d'`ota_1` (`0x37f000`), relu, identique octet pour octet à `B/app.iap1`** (preuve que l'appareil écrit la
même preuve que l'outil). Relever aussi : durée de l'upload et de l'activation, `ota_meta` (déploiement actif
= B), `otadata` (Valid, `ota_1`).

**P5, contrôles finaux.** `config` identique à la sauvegarde ; table, second étage, `ota_0` (A et sa preuve)
inchangés par l'OTA ; `/health` 200 TLS vérifié ; relevé de l'ensemble en référence `p91-wide`.

## Arrêts

Toute divergence de relecture, tout HALT du second étage, un secteur de preuve d'`ota_1` différent de la
référence, une pile (panique ou reset) pendant la vérification dans la tâche HTTP, une `config` modifiée.
Retour : `geometry-reprovisioning.md` R5. Aucune restauration directe d'`otadata` ou d'`ota_meta` sans nouvelle
autorisation.

## Hors périmètre

Corruption d'un slot, preuve effacée ou altérée, rollback, coupures de courant sur matériel, ESP32-C3 matériel,
Secure Boot V2 et eFuses, anti-rollback (#92).
