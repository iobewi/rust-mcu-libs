# Migration des applications A/B vers l'adaptateur SHA corrigé (#91, M0)

M0 est entièrement hors matériel : audit, reconstruction, contrôles et protocole. Rien n'est programmé.
Le contexte est dans [`verified-boot-sha.md`](verified-boot-sha.md) (contrat de la ROM) et
[`verified-boot-qualification.md`](verified-boot-qualification.md) (P3-E, P3-F).

## 1. Exposition des images installées à l'ancien SHA

Établie sur les ELF exacts (`firmware-ota-system-esp32` de A et de B, désassemblés ; les images ne contiennent
pas les adresses ROM des routines mais celles de leurs trampolines `__call_ets_sha_*` à `0x40001cec`–`0x40001d40` :
une recherche des adresses des fonctions ROM elles-mêmes ne prouve donc rien).

* Le code défectueux est **présent** dans A et dans B : `RomSha256::{new, update, finish, digest}` aux mêmes adresses
  (`digest` `0x4209f458`). `new` réserve 112 octets de cadre et y place un contexte de 80 octets (`movi 80`,
  `memset`, `ets_sha_enable`, `ets_sha_init`, `memcpy` de 80 octets) ; `digest` a un cadre de 208 octets avec le
  contexte à `sp+4` et une copie de 80 octets à `sp+84`.
* La ROM travaille sur 216 octets : `ets_sha_init` met à zéro 104 octets au-delà du cadre de `new` (dans le cadre de
  l'appelant) ; le compteur de bits à `ctx+200` tombe à `sp+204`…`sp+220` pour un cadre de 208 octets.
* **Chemins atteignables** (appels `RomSha256` → `admit` → `with_policy`, `boot_proof`, feature `signed-rsa`) :
  `boot_proof::publish` (fin d'un upload OTA, après le contrôle de digest, avant la publication) et
  `boot_proof::bootable` (activation). `invalidate` n'utilise pas le SHA. Aucun autre appelant : ni le démarrage de
  l'application, ni `/health`, ni la connexion Wi-Fi.
* **Conditions de déclenchement** : un upload OTA qui va jusqu'au bout, ou une activation. Une application A
  qui ne fait que démarrer et servir n'exécute pas ce code.
* **Effet** : écriture de zéros et de compteurs hors du contexte, dans des cadres de pile de l'appelant ; selon la
  disposition, plantage (comme en P3-D), condensé faux (donc refus d'une clé épinglée correcte ou d'une preuve
  valide) ou comportement indéterminé. Non exécuté sur matériel : l'effet exact n'est pas démontré, l'exposition l'est.
* **Conséquence** : ne jamais migrer A par un OTA exécuté par l'A actuel.

## 2. Contrat réel de `ota_meta`

Source : `common/firmware/update/src/compat/otm1.rs`, `arch/esp32/firmware/storage/src/ota_meta.rs`,
[`ota-meta-reprovisioning.md`](ota-meta-reprovisioning.md).

* Deux copies de 4 Kio, trame `OTSF` (version 1, séquence, CRC-32) autour de l'enregistrement **OTM1** : étape
  (`none`/`written`/`activating`), slot, **chaîne de digest**, identifiant de déploiement et taille du candidat
  (`staged`), plus le digest et l'identifiant du dernier déploiement confirmé (`active_*`).
* **Digest d'image** : oui, sous forme de chaîne de l'artefact **annoncé** à l'upload ; il n'est jamais comparé à
  l'image en flash au démarrage.
* **Identité cryptographique** : non. Le CRC détecte les écritures déchirées, il n'authentifie rien. Aucun lien
  avec IAP1, avec la clé épinglée ou avec le bootloader, qui **ne lit pas** `ota_meta`.
* **État installé** : copie 0 = `OTSF` séquence 1 portant un `OTM1` vide, copie 1 effacée (`58dd5c3c…`) ;
  aucune transaction, aucun `active_*`. `reconcile` compare le **nom du slot** démarré au slot attendu, pas un digest.
* **Le changement d'image n'exige pas de modifier `ota_meta`** (ni `otadata`) : la table, `otadata` et `ota_meta`
  produits par `prepare.py` pour A″/B″ sont identiques octet pour octet à ceux installés
  (`5e609d0b…`, `1170d1af…`, `58dd5c3c…`).
* **Invariants à conserver** : `otadata` inchangé (séquence 1, `ota_0`, `Stable`, donc pas de probation ni de
  rollback en cours) ; `ota_meta` vide ; aucune écriture de ces secteurs pendant la migration (relevé avant/après chaque étape).

## 3. Images reconstruites hors carte

Les sources de l'A installé sont celles du commit `2e62840` : sa reconstruction (`Aold`) a la même taille
(1 049 840 octets) et ne diffère de l'image installée que de 71 octets, tous dans le descripteur d'application
(date et heure de compilation, empreinte de l'ELF) et dans la somme finale. Les constructions sont donc
reproductibles **à l'horodatage près** : deux constructions successives d'une même source donnent des `app.bin`
qui diffèrent de 68 octets (heure, empreinte ELF, somme finale), et **chaque `app.bin` a donc sa propre preuve**.
Les artefacts ci-dessous sont figés ; ne jamais les reconstruire pour les installer.

| Jeu | Source | Rôle |
|---|---|---|
| **A″ / B″** (recommandé) | `2e62840` + correctif SHA seul (`git diff 2e62840 04372d5` limité à `rom_verify.rs`, `boot/lib.rs`, `hal_sha.rs`, `boot/Cargo.toml`, `bootauth/{soft.rs,lib.rs,Cargo.toml}`, `ota.rs`) | différence **strictement due à SHA** par rapport aux images installées |
| A′ / B′ (alternative) | `04372d5` (tête de branche) | inclut en plus la fusion de la console partagée (#102/#103) |

Mêmes entrées dans tous les cas : clé et épingle `4b452a4c…07ae`, produit `iobewi-ota-sys`, MCU `esp32s3`,
variante `0`, version de sécurité `0`, table `0x10000` (`partitions-s3-16mb-wide.csv`), Wi-Fi, jeton, certificat TLS
(A : `server2`, B : `server`, comme les images installées), profil `signed-rsa`, bootloader `verified`.

| Image | Octets | SHA-256 de `app.bin` | SHA-256 de `app.iap1` |
|---|---|---|---|
| installée A | 1 049 840 | `76de8229…` | `97ccb726…` |
| installée B | 1 049 840 | `d93299f9…` | `cb681dbc…` |
| **A″** | 1 050 112 | `a8ca0ed7671293694750cfa3b8f8d05759e28c5fedba008703a01246bfdabeb3` | `7d376f51bda8a69f0da2901ee01f75d4e9bcbc75adeac7050c75a3f5b23e1761` |
| **B″** | 1 050 112 | `1a36e280eabd4610677c0ae14fcfe01b1824c0655c0f887db08dc0d3c138a5f2` | `d1d5843caa241281ff82398041540cb2567e7a032a67525d468a0b604d35d7ad` |
| A′ | 1 051 232 | `d34d6b7bb52c4c56af44da6d8d0dff0fb23a31a4e203bfb2af2dcc72d7cf3223` | `2d2ae3651179dd22a4fc8f91449a512aff4a52a82be0f04fc1b63cd3c31db67c` |
| B′ | 1 051 232 | `ab85c881bf7f2d70de5137b7659189b45af6f70d1a9be06838a700b6b241332e` | `aebabde759bab48ed2aabfda697b5caf19144cde258c004aa7112337f6c2cd52` |

**Différences dues à SHA seul (A″ contre la reconstruction `Aold` de la source d'origine).** Un symbole disparaît
(`RomSha256::new`), un apparaît (`Drop for RomSha256`), sept changent de taille : `digest` 53→84, `finish` 54→162,
`update` 97→162, les deux `with_policy` 365→509, `RomRsaPss3072::verify` 562→272, `VerifiedBoot::admit` 785→849 ;
`.text` +96 octets, `.rodata` +168, `.data` inchangé en taille. Rien d'autre ne change (mêmes autres symboles et tailles). Dans `digest`,
le contexte est désormais de 212 octets plus deux mots sentinelle (`0x53484143`, `0x414E4152`) et un octet d'état,
copié en 228 octets, initialisé par la ROM à son emplacement. Aucun `movi 80` ne reste dans l'adaptateur.
A″ contre l'A installé : +272 octets ; mêmes table, `otadata`, `ota_meta`.

## 4. Contrôles hors matériel

* Correspondance image / manifeste / preuve : `iap1_pack verify` (la décision `VerifiedBoot::admit` de la
  production, sur un slot reconstruit en mémoire) : **authentique** pour A″, B″, A′, B′ (et pour les images
  installées), clé `4b452a4c…`, produit, MCU, variante, version, taille de slot `0x1b0000`.
  Limite : cet outil hache en logiciel ; le chemin ROM n'est exercé que sur carte.
* Dimensions : A″/B″ 1 050 112 octets ≤ zone utile d'un slot (`0x1b0000 − 0x1000` = 1 769 472) ;
  `ota_0` `0x20000`–`0x120600`, preuve `0x1cf000` ; `ota_1` `0x1d0000`–`0x2d0600`, preuve `0x37f000` ;
  aucun chevauchement entre eux, avec la table (`0x10000`–`0x15000`) ni avec le bootloader (`0x0`–`0x9000`).
* `validate_image` (segments de l'image contre la carte mémoire) : accepté, 6 segments, entrée `0x4037bc7c`.
* Compatibilité avec le bootloader nominal : même épingle, même cible ; le bootloader reconstruit par
  `prepare.py` depuis `04372d5` est identique au nominal (`5ca06b7c…`).
* Reproductibilité : voir §3 (horodatage seul) ; bootloader reproductible octet pour octet.
* CI : aucun code du dépôt n'a changé en M0 ; dernier état vérifié : 59 contrôles verts à `04372d5`.

## 5. Protocole M1–M5 (non exécuté ; chaque étape exige son autorisation)

Principes : l'ordre garde toujours une image récupérable ; `otadata` et `ota_meta` ne sont jamais écrits (empreintes
relevées avant et après chaque étape) ; **aucun démarrage avec un slot partiellement écrit** (un slot invalide
ferait basculer le bootloader et écrire `otadata`) ; toutes les tailles de dump en **hexadécimal** (`benchctl`
lit le décimal comme de l'hexadécimal) ; écritures et relectures avec `--stay` ; une seule opération d'état à la fois ;
reset automatique du banc laissé à `off`.

* **M1, sauvegardes (lecture seule).** Identité, `/health`, bootloader non signé, A `Stable`. Dump avec empreintes de
  `0x0`–`0xA000`, `0x10000`–`0x15000`, `ota_0` `0x20000`–`0x1d0000` (preuve incluse), `ota_1` `0x1d0000`–`0x380000`
  (attendu vierge), `config` `0x380000` et `0x3c0000`. Relecture de contrôle d'une seconde passe sur les secteurs
  écrits par la suite. *Arrêt* si une empreinte diverge de la référence (`bd7c4e20…`, `da4bfb3d…`, `97ccb726…`, `0b6821f0…`, `3b874d3b…`).
* **M2, B″ dans `ota_1` (A intact).** Écrire `B2/app.bin` à `0x1d0000` puis `B2/app.iap1` à `0x37f000`, relire. Pas de démarrage
  nécessaire ; en option un démarrage sous le bootloader non signé : doit rester A `Stable`, `/health` 200, `otadata`
  inchangé. Récupération : A intact. *Critères* : relectures identiques, `otadata`/`ota_meta` inchangés.
* **M3, A″ dans `ota_0`.** Écrire `A2/app.bin` à `0x20000` et `A2/app.iap1` à `0x1cf000` **dans la même session**,
  relire les deux, puis un seul démarrage sous le bootloader non signé. *Critères* : `running ota_0 status Stable`,
  `OTA ready`, `/health` 200 (TLS vérifié), `otadata`/`ota_meta` inchangés. Récupération : A d'origine en sauvegarde M1
  (réécriture directe), B″ valide en `ota_1`. *Arrêt* : toute écriture interrompue ou relecture divergente : réécrire, jamais démarrer.
* **M4, bootloader nominal.** Écrire `verified-iram36-uart0-sha216.bin` (`5ca06b7c…`, `0x0`–`0x9000`), relire, un démarrage :
  `VERIFIED`, `slots checked` (durée à noter : deux slots valides au lieu d'un), `plan slot=0`, `image ok`, `jump`, A″ `Stable`,
  `/health` 200 ; aucun refus, aucune écriture de métadonnées. *Arrêt* : exception ou refus. Restauration indépendante :
  écriture du bootloader non signé de référence (`bd7c4e20…`) par le loader.
* **M5, décision finale.** Soit conserver l'état migré (nouvelles références à enregistrer), soit **retour arrière** complet
  par écriture directe des sauvegardes M1 (bootloader, A, preuve, `ota_1` vierge), puis relecture et démarrage non signé.
  Arrêt avant P4 dans les deux cas.

## 6. Risques

1. **Premier passage de `boot_proof` corrigé sur matériel** : A″ n'a jamais exécuté `publish`/`bootable` sur carte ; ce
   chemin est exercé par P4, pas par M1–M5 (qui ne font pas d'OTA).
2. **Preuve liée à un `app.bin` unique** (horodatage dans l'image) : ne jamais reconstruire une image installée sans
   régénérer sa preuve ; utiliser exclusivement les fichiers figés (`MANIFEST.sha256`).
3. **Deux slots valides** changent la durée de `slots checked` et peuvent changer le comportement de repli si A est refusée
   (écriture d'`otadata` : c'est P4, pas M4).
4. **Rupture d'écriture en M2/M3** : récupérable par le loader et les sauvegardes M1 ; pas de reconstruction à chaud.
5. **Dérive de sources** : A′/B′ embarquent la console partagée (#102/#103) en plus du correctif ; A″/B″ non. Choisir A″/B″
   pour une migration sans autre changement.
6. **Entrées hors dépôt** (Wi-Fi, jeton, certificats, clé du banc) : les mêmes que les images installées ; ne jamais les publier.
7. **Format et politique** : IAP1, VerifiedBoot, RSA-PSS non modifiés ; la clé épinglée est inchangée, donc le bootloader nominal
   reste valable.

## 7. Scénarios négatifs P4 (préparés, non exécutés)

Après M4 seulement, un cas par cycle, avec restauration indépendante : preuve absente, tronquée, CRC altéré ; manifeste altéré
(digest, taille, produit, variante, version de sécurité) ; signature altérée ; mauvaise clé ; algorithme non autorisé ; image
altérée avec preuve intacte (`DigestMismatch`) ; refus d'un slot avec repli sur l'autre (écriture `otadata` attendue) ; refus des
deux slots (HALT, pas d'écriture) ; OTA A→B complète avec publication de preuve ; coupures pendant la publication de la preuve.

## 8. M1 : état initial et sauvegardes (lecture seule, 2026-10-31)

Aucune écriture sur la flash. Carte ESP32-S3 QFN56 v0.2, flash 16 Mio, MAC `9c:13:9e:ac:4e:8c`. Bootloader non signé
UART0, A Stable, `/health` 200 avec TLS vérifié. Reset automatique désactivé (un appel sans argument l'a activé 9 s par
erreur, sans reset ni écriture ; remis à `off`).

| Région | Plage | SHA-256 | État |
|---|---|---|---|
| bootloader | `0x0`–`0xA000` | `bd7c4e20…a165` | identique à la référence P3 (non signé UART0) |
| table / `otadata` / `ota_meta` | `0x10000`–`0x15000` | `da4bfb3d…c51f2` | table `6bf9d714`, `otadata` `1170d1af` (copie 0 seq 1, état valide ; copie 1 vierge), `ota_meta` `58dd5c3c` : identiques aux installés |
| `ota_0` | `0x20000`–`0x1d0000` | `f6ea354e…9f2e` | A installé (`76de8229…`, 1 049 840 o) à `0x20000` ; résidus d'anciennes images après lui (`0x130000`–`0x164000`, `0x170000`–`0x17f000`) |
| preuve `ota_0` | `0x1cf000`–`0x1d0000` | `97ccb726…` | preuve installée de A |
| `ota_1` | `0x1d0000`–`0x380000` | `ca19ea3a…c826` | ancienne image non signée (`example-firmware-ota-system`, 2026-10-13, jusqu'à `0x2da000`, identique à un artefact d'avant #91), ni A ni B |
| preuve `ota_1` | `0x37f000`–`0x380000` | `f47a8ec3…` | vierge (donc `ota_1` n'est pas admissible sous VerifiedBoot) |
| `config` (EKV) | `0x380000`–`0x400000` | `8b911b67…bb5d` | inchangée ; les deux secteurs de référence valent un secteur vierge |

Sauvegardes : `bench/backups/m1-20261031/` (lecture seule, `MANIFEST.sha256`), plus les originaux A/B dans `q91/`.
Le bootloader n'a pas été recopié (même hash que la sauvegarde P3 vérifiée).

Artefacts figés A″/B″ : `MANIFEST.sha256` vérifié (17/17). `iap1_pack verify` : authentique pour A″ et B″ (clé `4b452a4c…`,
produit `iobewi-ota-sys`, `esp32s3`, variante 0, taille signée 1 050 112 = `0x100600`) ; contrôles négatifs refusés
(image A″ + preuve B″ : `DigestMismatch` ; mauvaise épingle : `UnknownKey` ; mauvaise variante, produit, MCU : `Target(...)`).
Limite inchangée : hachage logiciel, le chemin ROM n'est exercé que sur carte.

## 9. M2 : B″ et sa preuve dans `ota_1` (2026-10-31)

Écrit : image B″ (`B2/app.bin`, `1a36e280…`) à `0x1d0000`, effacement `0x1d0000`–`0x2d1000` ; preuve (`B2/app.iap1`,
`d1d5843c…`) au secteur `0x37f000`. Relecture de `ota_1` : image exacte, preuve exacte, `ota_1` entier `8c93ea6e…b34c`
(avant : `ca19ea3a…`). Résidus `0x2d1000`–`0x2da000` et écart jusqu'à la preuve inchangés. `iap1_pack verify` sur la relecture :
authentique (taille signée 1 050 112 = longueur de B″, les résidus n'en font pas partie). `ota_0` (`f6ea354e…`), bootloader
(`bd7c4e20…`), `0x10000`–`0x15000` (`da4bfb3d…`) et `config` (`8b911b67…`) relus identiques avant le retour à la console. Aucun démarrage de B″.

## 10. M2-B : un essai de démarrage de B″ sous le bootloader non signé (2026-10-31)

`otadata` avant : secteur 0 = seq 1 `Valid` (→ `ota_0`), secteur 1 vierge. Candidat construit hors carte avec le codec
`iobewi-firmware-boot` (`activate(target=ota_1)`) : secteur 1 = seq 2 `New` (→ `ota_1`), secteur 0 conservé ; il se
décode à l'identique. Écrit seul à `0x12000` (4 Kio, `afff01be…`), relu et décodé conforme ; table, `ota_meta`, secteur 0 inchangés.

Un démarrage : le bootloader choisit `ota_1` seq 2, écrit `Pending`, `image ok`, saute ; B″ affiche
`running ota_1 status Rollback` puis redémarre de lui-même (`RTC_SW_SYS_RST`, 0,3 s) ; le bootloader revient à `ota_0`
seq 1. Cause : `reconcile` classe « slot en attente de confirmation sans transaction `Activating` dans `ota_meta` » en
`RollbackUnaccounted` (`transaction.rs:107`) ; l'application refuse volontairement une sélection qui ne vient pas d'une
activation OTM1. `otadata` après : secteur 1 = seq 2 `Invalid`. `ota_meta` inchangé. Ni Wi-Fi ni `/health` de B″ atteints.
Restauration : `0x10000`–`0x15000` réécrit depuis M1, relu `da4bfb3d…` ; `ota_0`, `ota_1`, bootloader inchangés ;
A Stable, `/health` 200. Conséquence : confirmer B″ exige un enregistrement OTM1 `Activating` pour `ota_1` en plus d'`otadata`.
