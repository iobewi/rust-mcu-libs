# SHA-256 du second étage vérifié : contrat de la ROM, correction, choix du backend (#91)

Le premier essai matériel du second étage vérifié (P3) s'est arrêté sur une exception `LoadProhibited` à l'adresse 0
(`verified-boot-p3-diagnosis.md`). Ce document établit **pourquoi**, sur les ROM elles-mêmes, décrit la correction
de l'adaptateur et compare les trois façons de calculer un SHA-256 dans le second étage.

## Ce qui s'est passé

L'adaptateur déclarait le contexte SHA de la ROM sur **80 octets** (`state[16]`, `total_bits[4]`). Le contexte de la
ROM en fait **216**. Trois défauts en découlaient :

1. **Débordement.** `ets_sha_init` met à zéro 216 octets à partir du pointeur reçu : 136 octets du cadre de pile de
   l'appelant étaient écrasés. Dans le second étage, cela a mis à zéro une référence sauvegardée dans la
   fermeture d'admission, relue juste après l'appel (`policy.rs:155`, déréférencement de 0).
2. **Compteur de bits faux.** `ets_sha_update` et `ets_sha_finish` tiennent la longueur du message à
   `contexte + 200`. `digest` copiait le contexte sur 80 octets entre `init` et `finish` : la ROM lisait la longueur dans
   la mémoire voisine, donc un condensé faux, même sans plantage.
3. **Périphérique laissé actif** si un hash était abandonné avant `finish`.

La sonde A3, qui avait validé les routines de la ROM dans une application, utilisait la même structure de
80 octets : son contexte n'était jamais déplacé et la pile au-dessus de lui était libre, donc rien ne se voyait.

**L'application OTA contient le même défaut** : `ota.rs` hache la clé épinglée avec le même adaptateur (`publish`,
`gate`). Les images A et B de la qualification, construites avant la correction, devront être reconstruites (nouveaux
condensés, nouvelles preuves) avant tout P4.

## Le contrat de la ROM, vérifié

Preuve : les **images de ROM** publiées par Espressif (`espressif/esp-rom-elfs`, version 20260528, archive
`caa463d3cbef2430a5a35847c1d9f2f152403b17a802050927ff60c8da54fe46`), désassemblées, et non les seuls en-têtes
(`esp_rom/<puce>/include/<puce>/rom/sha.h`, identiques pour l'ESP32-S3 et l'ESP32-C3). La ROM de la carte est
`esp32s3_rev0` (bandeau `ESP-ROM:esp32s3-20210327`).

| ELF de ROM | SHA-256 |
|---|---|
| `esp32s3_rev0_rom.elf` | `c0ce0f338d1de1bd…` |
| `esp32c3_rev0_rom.elf` | `11d565922e0a405b…` |
| `esp32c3_rev101_rom.elf` | `9495e1453f36f7ee…` |
| `esp32c3_rev3_rom.elf` | `19ac22e08707df92…` |

**Le contexte `SHA_CTX` : 216 octets, alignement 4.**

| Champ | Décalage | Taille |
|---|---|---|
| `start` | 0 | 1 octet |
| `in_hardware` | 1 | 1 octet |
| (remplissage) | 2 | 2 octets |
| `type` | 4 | 4 octets |
| `state[16]` | 8 | 64 octets (SHA-256 : 8 mots utilisés) |
| `buffer[128]` | 72 | 128 octets (SHA-256 : 64 utilisés) |
| `total_bits[4]` | 200 | 16 octets |

**Les routines** (désassemblées sur l'ESP32-S3 rév. 0 ; sur les trois révisions du C3, `ets_sha_init` contient la
constante 216, `ets_sha_update` accède aux décalages 200 à 212 et `ets_sha_finish` à 200 et 204) :

| Routine | Comportement |
|---|---|
| `ets_sha_init(ctx, type)` | refuse un pointeur nul ou `type ≥ 8` (code `0x102`) ; sinon `memset(ctx, 0, 216)`, `ctx.type = type`, retourne 0 |
| `ets_sha_update(ctx, entrée, n, update_ctx)` | ajoute `8·n` au compteur 128 bits (`+200`), complète `buffer` (`+72`), appelle `ets_sha_process` par bloc de 64 octets ; avec `update_ctx = false` l'état reste **dans le périphérique** |
| `ets_sha_process(ctx, bloc)` | écrit le bloc dans le périphérique ; au premier bloc démarre, ensuite continue ; recharge `state` si `in_hardware` est faux |
| `ets_sha_finish(ctx, sortie)` | lit le compteur, bourre avec `sha_padding`, ajoute la longueur, dernier `update` avec `update_ctx = true`, puis copie `sha_result_bytes[type]` octets de `state` : **32 pour SHA-256** |
| `ets_sha_enable` / `ets_sha_disable` | horloge et remise à zéro du périphérique (`SYSTEM_PERIP_CLK_EN1` / `RST_EN1`), **sans compteur de références** |

**Stabilité d'adresse : aucune contrainte.** Aucune routine ne mémorise l'adresse du contexte (les seules constantes
qu'elles chargent sont des registres du périphérique et des tables de la ROM). Déplacer un contexte **entier** est sûr ;
le tronquer ne l'est pas. L'état du hash est *global* tant que `in_hardware` est vrai : rien d'autre ne doit utiliser le
périphérique SHA entre `new` et `finish`.

**Le RSA-PSS de la ROM utilise ce même périphérique** : `ets_rsa_pss_verify` appelle `ets_emsa_pss_verify`, qui appelle
`ets_sha_enable`, ses propres hash puis `ets_sha_disable`. Un hash de notre côté ne doit donc jamais être vivant pendant
cet appel ; `RomRsaPss3072::verify` termine le sien avant. `verified_digest` reçoit exactement 32 octets (la boucle
finale de `ets_emsa_pss_verify` fait 32 itérations) : le tampon de l'adaptateur RSA est correct. La pile de la ROM pour
le RSA-PSS est d'environ 816 + 624 + 288 octets plus un contexte SHA : bien dans la région de 26 à 29 Kio.

**Re-vérifier** : télécharger l'archive ci-dessus, `xtensa-esp32s3-elf-objdump -d esp32s3_rev0_rom.elf` sur `ets_sha_init`
(`movi a12, 216 ; call8 memset`), `ets_sha_update` (accès `+200..+215`), `ets_sha_finish`.

## La correction

Dans `arch/esp32/firmware/boot/src/rom_verify.rs`, en Rust seulement (aucune dépendance à ESP-IDF, l'en-tête n'est qu'une
référence documentaire) :

* `ShaContext` est la structure de la ROM champ par champ, `#[repr(C)]`, avec des **assertions à la compilation** sur la
  taille (216), l'alignement (4) et chaque décalage : une dérive ne compile pas ;
* le contexte est **initialisé au premier usage, à l'endroit où il vivra** (`new()` ne touche pas la ROM) : il n'est
  jamais initialisé dans un temporaire puis déplacé ;
* deux mots sentinelle suivent le contexte et sont vérifiés après l'initialisation et après `finish` : une ROM dont le
  contexte serait plus grand (une autre révision) arrête le démarrage au lieu de corrompre la pile ;
* `Drop` libère le périphérique, même pour un hash abandonné ;
* un seul alias, `Sha256Backend`, est utilisé par la politique de démarrage, par le condensé du message du RSA-PSS et
  par l'application OTA : il n'y a plus de chemin qui hache avec un autre adaptateur.

## Trois backends

Sélection à la compilation, `iobewi-esp-boot` : la ROM par défaut, `sha-hal` (pilote `esp-hal`), `sha-soft` (logiciel).
L'interface portable `Sha256Stream` reste dans `common/firmware/bootauth` ; le SHA logiciel (`SoftSha256`, `sha2` en
`no_std`, sans `unsafe`) y est aussi, puisqu'il ne dépend d'aucune architecture ; les implémentations matérielles sont
dans `arch/esp32`.

Mesures sur les images liées, profil vérifié (octets ; S3 : table à `0x10000`, C3 : `0x8000` ; fenêtre de code de 36 864) :

| | ROM corrigée | `esp-hal` | logiciel |
|---|---|---|---|
| **S3** image | 34 896 | 36 368 (+ 1 472) | 38 768 (+ 3 872) |
| S3 IRAM utilisée | 31 147 | 32 607 | 34 975 |
| S3 IRAM libre | **5 717** | 4 257 | 1 889 |
| S3 DRAM statique ; région de pile | 3 748 ; 29 020 | 3 748 ; 29 020 | 3 780 ; 28 988 |
| **C3** image | 27 392 | 28 448 (+ 1 056) | 31 120 (+ 3 728) |
| C3 IRAM utilisée ; libre | 25 196 ; 11 668 | 26 250 ; 10 614 | 28 888 ; 7 976 |
| cadre de pile de l'admission (S3) | 2 128 | 1 712 | 1 776 |
| autres cadres (S3) | `digest` 496 | `finish` 128 | `compress256` 496 |
| code `unsafe` | 7 lignes (SHA et RSA) | 1 (`SHA::steal`) | 0 |
| dépendances ajoutées | aucune (`esp-rom-sys`, déjà là) | `esp-hal/unstable`, `nb` | `sha2` (déjà dans l'arbre via `rsa`) |
| temps par Mio haché | **204 ms**, mesuré en phase A3 (application) | non mesuré | environ 4 900 instructions par bloc, soit **77 par octet** (comptage statique) : de l'ordre de **1 s à 80 MHz** (1 instruction par cycle supposée), soit environ cinq fois la ROM |
| initialisation matérielle | `ets_sha_enable` / `disable` | garde à compteur de références ; remise à zéro à l'activation ; ne doit pas vivre pendant le RSA de la ROM | aucune |
| portabilité | ESP32-S3 et C3 (4 ROM vérifiées) | toutes les puces `esp-hal` avec pilote SHA | toute architecture |

Faits qui comptent :

* le bootloader tourne à **80 MHz** (`esp_hal::init(Config::default())` : `CpuClock::_80MHz`) ; le SHA logiciel, qui
  hache tout le slot à chaque démarrage, y coûte de l'ordre de la seconde par Mio (estimation, non mesurée) là où un
  accélérateur ne fait que transférer les blocs, et ne laisse que 1,9 Kio d'IRAM sur S3, où la variante de
  diagnostic (+ 0,7 Kio) ne tiendrait plus ;
* le pilote `esp-hal` n'existe qu'avec la fonctionnalité `unstable` ; `Sha::new` active l'horloge et **pulse la remise à
  zéro** au premier usage, donc il repart proprement après la ROM, mais une garde laissée vivante pendant un RSA-PSS de
  la ROM (qui coupe le périphérique à son insu) le rendrait inutilisable ;
* aucun des trois n'a tourné correctement sur la carte : la ROM corrigée n'est prouvée que sur les images de ROM et un
  modèle, `esp-hal` et le logiciel ne sont que compilés et dimensionnés.

### Choix

**La ROM corrigée est le backend du second étage vérifié.** Son contrat est maintenant établi sur les images de ROM
elles-mêmes ; c'est l'accélérateur, celui qui coûte le moins de code (5,7 Kio d'IRAM libres sur S3 contre 4,3 et 1,9),
et il partage avec le RSA-PSS de la ROM, que nous employons de toute façon, une seule façon de gérer le périphérique
(aucune garde à compteur de références à faire cohabiter avec une ROM qui l'ignore). `esp-hal` reste une alternative
compilée et dimensionnée par la CI, utilisable si le contrat de la ROM devait changer ; le SHA logiciel est le repli
portable (autre architecture, ou ROM sans SHA). Le choix définitif attend la qualification sur la carte.

## Qualification logicielle

| Contrôle | Résultat |
|---|---|
| `tests/rom_sha.rs` (modèle de la ROM : 216 octets, compteur à `+200`, état dans le périphérique ; chaque accès au contexte est enregistré, l'adaptateur est encadré de deux bandes de garde) | 10 tests : vecteurs NIST, un million de `a` en morceaux irréguliers, toutes les longueurs de 0 à 200 et autour de 256, 512, 4 096, un message de la taille d'un slot lu par 256 octets, entrée non alignée, hash déplacé en cours de route, périphérique libéré par `finish` et par un hash abandonné, **portée de la ROM ≤ 216 octets et bandes de garde intactes** |
| le même test sur **l'ancien adaptateur** | le processus meurt (`malloc(): corrupted top size`) : la suite détecte le défaut d'origine |
| `bootauth/tests/soft_sha.rs` | 4 tests : NIST, un million de `a`, toutes les longueurs et tous les découpages, un slot lu par 256 octets |
| Clippy, hôte et cible Xtensa | propre pour les quatre combinaisons de backends et la variante de diagnostic |
| Compilation S3 et C3 des trois backends | OK, dans les limites ; ajoutées à la CI (`verified_boot.py --sha`) |
| Reproductibilité | deux constructions du vérifié corrigé donnent le même binaire |

**Limite de ces preuves.** Le modèle est une lecture de la ROM, pas la ROM : il rend le contrat exécutable et attrape la
classe de défaut vue en P3, mais il ne remplace pas un essai sur la carte. Le SHA logiciel est comparé en partie à la
même bibliothèque (`sha2`) ; ses vecteurs NIST sont indépendants.
