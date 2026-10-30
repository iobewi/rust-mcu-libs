# Premier démarrage vérifié sur matériel : protocole minimal proposé (#91)

**État (30 octobre 2026).** P0, P1 et P2 ont été exécutés sur la carte de qualification et validés. Une première
tentative de P3 a échoué (voir « Déroulé réel ») ; ce document décrit la **reprise de P3**, qui n'est pas
exécutée. Chaque étape d'écriture exige une autorisation distincte ; arrêt à la première divergence. Il se lit
avec `geometry-reprovisioning.md` (comment fabriquer et écrire) et `flash-geometry.md` (la disposition). Les
campagnes de corruption, de rollback et de coupures ne font **pas** partie de ce premier essai.

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
| `boot/unsigned-iram36.bin` | second étage non signé (P2) |
| `boot/verified-iram36.bin` | second étage vérifié, clé épinglée du banc (tentative 1 de P3) |
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

La fenêtre de code est de 36 Kio (`0x403cb000`), l'état du dépôt ; les variantes `iram64` ont été abandonnées (voir
`flash-geometry.md`). Ces deux bootloaders ont la console sur le port USB natif ; la reprise de P3 utilise de
nouveaux bootloaders dont la console est UART0 (voir « Images de la reprise de P3 »).

## Déroulé réel

| Étape | Résultat |
|---|---|
| P0, lecture seule | PASS : carte ESP32-S3 16 Mio, table historique à `0x8000`, Ns2 `Stable` en `ota_1`, `ota_0` = prototype A3 (Aborted), sauvegarde de 16 Mio en 16 morceaux de 1 Mio, `/health` 200 |
| P1, preuve de récupération | PASS : le second étage historique (29 808 octets) a été réécrit à l'identique à `0x0`, relu identique, démarrage normal ; table, métadonnées, slots et `config` inchangés |
| P2, reprovisionnement | PASS : six écritures (A, sa preuve, `otadata`, `ota_meta`, table à `0x10000`, second étage non signé `iram36`) relues identiques ; démarrage `running ota_0 status Stable`, `/health` 200 TLS vérifié, `config` conservée |
| P3, tentative 1 | **ÉCHEC** : le second étage vérifié `iram36` est chargé par la ROM jusqu'à son point d'entrée (`entry 0x403cb624`), puis plus rien : pas de reset, application non lancée, `/health` muet. Cause non établie. Les messages du second étage partaient alors sur le port USB natif, que le banc ne lit pas |
| Restauration | PASS : second étage non signé `iram36` réécrit (relu identique), A de nouveau `Stable`, `/health` 200 |
| Qualification du port natif | Le port USB natif capture la ROM et le second étage (ouvrir le port redémarre la carte : `rst:0x15 USB_UART_CHIP_RESET`). Le pont UART perd des caractères dans la copie relevée |

Depuis, le second étage écrit sur UART0 par le connecteur commun (#102) : le banc le lit sans port supplémentaire.

## Référence P2 : état initial de la reprise de P3

Relevée le 30 octobre 2026 (lectures ciblées, jamais de sauvegarde complète). Les redémarrages qui ont suivi n'ont
rien écrit (A est `Stable`) ; P3-0 le reconfirme.

| Région | SHA-256 |
|---|---|
| `0x0`–`0x7540`, second étage non signé de P2 (console USB natif) | `ac323ea468802b8f4d401eb98899d01e0797d7fefc2a619fd3ae02902dadd70e` |
| `0x0`–`0x9000` (avec la queue inerte du second étage vérifié en `0x8000`–`0x8830`) | `16c9d7ad13d93e548b5cfaaac96d4c3d108805aa4515ef794ded5c5378d86202` |
| table `0x10000` (3 072 octets) | `5e609d0b07792a1515f2c4331bb8aa0b79a7a54a65d7027fa1b5783c29f91b08` |
| `otadata` `0x11000`–`0x13000` | `1170d1af5a2bef83547dcd22f3faf993f5255c79eae16318a73478a13af1a0bc` |
| `ota_meta` `0x13000`–`0x15000` | `58dd5c3c3deb6df820ab6c7b99e1b5c47e79285670a13e0e7485606044dd2afc` |
| `ota_0` : A (1 049 840 octets) | `76de82293c5ea2555516e98b90535f28a0840f6b58e866706a61909714151c9d` |
| preuve de A `0x1cf000` (4 096 octets) | `97ccb7262c4609c3c6bc543403ca74687b28524248c0a27f03c13f5fb3ecbefc` |
| `config` `0x380000`–`0x400000` | `8b911b6784dd6d854cfc5fe1f2635f2bf3a8e033701c31bacdd4ca8e58debb5d` |
| `ota_1`, 4 premiers Kio (Ns2) | `eaf430ab1074a16300c46a8e8708a27cc5cbce83de8febb68191a6c647a52936` |

## Images de la reprise de P3

Construites par `cargo build --profile bootloader -p iobewi-esp-bootloader --features esp32s3[,verified-boot]` puis
`esptool elf2image --flash-mode dio --flash-size 16MB`, avec la table à `0x10000`, la console `uart0` (défaut), et
pour le vérifié `IOBEWI_BOOT_PINNED_KEYS` (clé du banc), `IOBEWI_BOOT_PRODUCT=iobewi-ota-sys`,
`IOBEWI_BOOT_VARIANTS=0`. Arbre à `00eb94f`. **Reproductible** : trois constructions dans des répertoires de
build différents donnent le même binaire.

| Fichier (hors Git, `0600`) | Octets | SHA-256 |
|---|---|---|
| `unsigned-iram36-uart0.bin` | 29 984 | `38b162ac9e54e8fef881b9c3663f2f4c7140d15e3086156063fd16a01251b0dc` |
| `verified-iram36-uart0.bin` | 34 816 | `ddeccf27c96a16733776e85e7b832dbbf2b1f96428134bb7cec8686e8e91bf1a` |

Pour le vérifié, constantes **lues dans le binaire** : condensé de clé épinglé
`4b452a4c558aec346bf2f481177e99d0c8c02543495ff4c33229793c60b607ae`, variantes `[0]`, produit `iobewi-ota-sys`,
estampille de géométrie `0x10000`, estampille de console `uart0`. La version de sécurité minimale (`0`) et le MCU
(ESP32-S3) viennent de la configuration du build, non extractibles du binaire. Segments chargés :
`0x3fce4000` (0x5cc), `0x3fce4628` (0x87c), `0x403cb000` (0x1828), `0x403cc828` (0x612c).

L'image A, sa preuve et la table sont **inchangées** (référence P2) : le second étage est la seule variable. A date
d'avant le connecteur de console (pas d'estampille de console) ; elle écrit sur UART0 comme le nouveau second
étage, ce qui suffit pour le banc. Le contrôle d'estampille bootloader/application est un contrôle de
construction, fait sur une application construite avec le connecteur.

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

**P3, reprise : second étage vérifié.** Une seule variable change par rapport à P2 : le second étage.

*P3-0, état initial (lecture seule).* Identité (MAC `9c:13:9e:ac:4e:8c`, flash 16 Mio), `/health` 200 en TLS
vérifié, démarrage `running ota_0 status Stable`. Lectures ciblées `0x0`–`0x9000` et `0x10000`–`0x15000` : tout
doit être égal à la référence P2. Pas de sauvegarde de 16 Mio. Arrêt à la moindre différence.

*P3-1, fichiers avant programmation.* SHA-256 du fichier à écrire égal à celui du tableau ci-dessus ;
estampilles de géométrie (`esp_geometry.py stamp` = `0x10000`) et de console (`esp_console.py stamp` = `uart0`) ;
`iap1_pack verify` sur A et sa preuve avec le condensé, le produit, le MCU, la variante et la version minimale du
second étage : « authentic ». Le serveur du banc ne liste que les `*.bin` : copier le second étage sous ce nom.

*P3-2, capture.* **Un seul onglet** du banc (pont UART, CH343), au premier plan, PC sans mise en veille ; accès
« complet » accordé pour la durée. Le journal du banc n'a pas de date : relever le **numéro de ligne** de
`serial.log` avant chaque opération et ne lire que ce qui suit ; ne conclure sur l'état de la flash que par
relecture. Avant d'écrire, un repère (`mark`) doit apparaître dans la capture : un `NetworkError` côté page, ou une
page qui ne répond pas, arrête l'essai. Le pont a perdu des caractères le 30 octobre : en cas de ligne tronquée,
recouper avec `rxhex on`. Le port USB natif n'est pas nécessaire (le second étage écrit sur UART0).

*P3-3, écriture unique.* Écrire `verified-iram36-uart0.bin` à `0x0` **uniquement**, avec `--stay` (aucun démarrage
intermédiaire). Relire `0x0`–`0x9000` : `0x0`–`0x8800` égal à l'image, `0x8800`–`0x9000` vierge.

*P3-4, un seul redémarrage.* `console` : le retour à la console réinitialise la carte par EN, la capture part de la
ROM.

*P3-5, verdict.* Attendu, dans l'ordre : bandeau ROM, `load:` des quatre segments ci-dessus, `entry 0x403cb624`,
`iobewi-esp-bootloader 0.1.0`, `boot: wdt …`, `boot: flash size 0x01000000`,
`boot: table at 0x00010000 otadata at 0x00011000 ota_0 0x00020000 ota_1 0x001d0000`,
`boot authentication: VERIFIED`, **`boot: slots checked, microseconds N`** (le coût de la vérification, à relever
tel quel), `boot: plan slot=0x00000000 seq=0x00000001`, `boot: image ok, segments=0x00000006`, deux lignes
`boot: map … rc=0x00000000`, `boot: jump`, puis `running ota_0 status Stable`, `OTA authentication`, Wi-Fi,
`OTA ready`. Aucune ligne `boot: otadata write` : un slot `Valid` de séquence 1 n'écrit rien. Le temps entre
`entry` et `OTA ready` se relève à la résolution de la page ; ne rien en déduire de plus fin.

Ce qu'on lit si la vérification ne mène pas à l'application (comportement attendu en cas d'échec, **à ne pas
provoquer** : aucun essai de corruption). Le second étage affiche ses nombres en **hexadécimal sur 8 chiffres**
(`0x00000065`), pas en décimal :

| Dernier message | Signification |
|---|---|
| `boot: slot 0x00000000 refused, reason 0x00000065` à `0x0000006f` | preuve refusée, code = 0x64 + n : 1 absente, 2 tronquée, 3 corrompue, 4 mal formée, 5 algorithme non autorisé, 6 clé inconnue, 7 clé invalide, 8 mauvaise signature, 9 cible, 10 (`0x6e`) taille, 11 (`0x6f`) digest |
| `boot: slot 0x00000000 refused, reason 0x00000001` à `0x0000000d` | structure de l'image refusée (1 à 13) |
| `reason 0x00000000` répété puis `boot: HALT, flash read errors while verifying, nothing written` | erreurs de lecture flash après trois essais |
| `boot: HALT, nothing safe to boot, reason 0x00000001/2/3` | aucune image démarrable / `otadata` inutilisable / format inconnu |
| `boot: FAILED …` (lecture, table, `otadata`, image, MMU) | échec physique, avec la valeur en cause |
| `boot: PANIC` | panique du second étage (sans emplacement, voir `examples/firmware/bootloader/README.md`) |
| rien après `boot authentication: VERIFIED` | blocage dans la vérification (ROM SHA ou RSA, ou pile) |
| rien après `entry` | blocage avant la première ligne : initialisation |

*P3-6, après l'observation.* `/health` 200 en TLS vérifié. Si un second passage en téléchargement est autorisé :
relire `0x10000`–`0x15000`, la preuve `0x1cf000` et A par empreinte, `config` ; tout égal à la référence P2.

*Arrêts.* Divergence en P3-0, capture muette ou en erreur, plus aucune ligne 30 s après le dernier marqueur
(noter ce marqueur et s'arrêter), bandeau ROM qui se répète (boucle de reset), panique, refus, `/health` KO après
`jump`. Un seul démarrage avec ce second étage, sauf autorisation contraire.

*Restauration (autorisation distincte).* Réécrire le second étage non signé : celui de la référence P2
(`ac323ea4…`, 30 016 octets) ou, pour avoir les diagnostics sur UART0, `unsigned-iram36-uart0.bin`
(`38b162ac…`). Relire, un redémarrage, `running ota_0 status Stable`, `/health` 200. **Ne pas** restaurer le second
étage historique de 29 808 octets : il correspond à l'ancienne géométrie.

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
