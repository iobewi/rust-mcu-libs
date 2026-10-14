# Campagne ESP32 — ConfigSpace EKV et OTA A (#68)

## Périmètre

S3 principal, C3 en confirmation. Tester le nouveau backend nominal, pas les
anciennes images NVS. Code/compilations et carte sont deux validations distinctes.
Le défaut de reprise EKV après interruption de commit reste suivi dans #70 ;
cette campagne ne le déclare pas corrigé. Les essais ci-dessous coupent/reset
uniquement après la fin des opérations. Pas de migration ni de rescue implicite.

Noter pour chaque essai : commit, chip, capacité flash, variantes, toolchains,
checksum SHA256 de l'image, commande de flash et logs série. Les credentials
Wi-Fi/Bearer/clé TLS ne doivent pas être joints aux rapports publics.

## 1. ConfigSpace isolé — config-manager

Charger export-esp.sh, installer les outils indiqués dans le README et compiler :

```sh
cargo +esp build -Z build-std=core,alloc --target xtensa-esp32s3-none-elf \
  -p example-config-manager --features esp32s3 --release --bins
```

La table standalone contient config à 0x9000/0x40000 et factory à 0x50000.
Pour une installation neuve volontaire sur la carte de développement, effacer
la carte, puis flasher avec un bootloader ESP-IDF compatible. espflash 4.6.0
ne peut pas convertir cette table : son analyseur (esp-idf-part 0.6.0) panique
sur le sous-type data `0x40`, en CSV comme en binaire. Générer la table avec
`gen_esp32part.py` officiel et assembler avec esptool ; le bootloader ESP-IDF
fourni par espflash (`resources/bootloaders/esp32s3-bootloader.bin` de son
paquet) convient, esptool y règle DIO/4MB et recalcule son SHA :

```sh
python3 /chemin/esp-idf/components/partition_table/gen_esp32part.py \
  examples/config-manager/partitions.csv partitions.bin
python3 -m esptool --chip esp32s3 elf2image --flash-mode dio --flash-size 4MB \
  -o app.bin target/xtensa-esp32s3-none-elf/release/config-manager-esp32
python3 -m esptool --chip esp32s3 merge-bin --flash-mode dio --flash-size 4MB \
  -o merged.bin 0x0 esp32s3-bootloader.bin 0x8000 partitions.bin 0x50000 app.bin
python3 -m esptool --chip esp32s3 --port /dev/ttyUSB0 erase-flash
python3 -m esptool --chip esp32s3 --port /dev/ttyUSB0 write-flash 0x0 merged.bin
```

Les variantes suivantes (exercise, retour au mode normal) ne réécrivent que
`app.bin` à 0x50000, sans effacement. Adapter le port réel ; les validations
logicielles n'exécutent pas ces commandes. Ne pas utiliser le bootloader OTA A avec cette
table factory. Si CARGO_TARGET_DIR est défini, adapter le chemin de l'ELF.

| Essai | Attendu |
| --- | --- |
| Premier boot sur région vierge | initialized generation 1 ; verified generation 1 |
| 10 reboots logiciels après verified | retained/verified generation 1, valeur config-ready |
| 10 débranchements/rebranchements après verified | même valeur et même génération |
| Variante exercise, sans effacer config | commit/clear/recommit PASS ; génération précédente +3 |
| Reboot exercise, sans effacer config | génération précédente retenue, puis +3 à nouveau |
| Retour à la variante normale | dernière génération conservée ; aucune réinitialisation |

Compiler exercise avec `--features esp32s3,exercise`. Ce mode réalise trois
commits par boot volontairement, puis laisse config-ready comme valeur finale.
Le mode normal n'écrit qu'à la première initialisation. Les assertions contrôlent
remplacement, absence après clear, readback et générations ; les logs donnent
la preuve observable. Aucun test de clear sur carte n'est déclaré réalisé avant
ces essais.

## 2. Refus d'une région non reconnue (optionnel, carte d'essai)

Après sauvegarde d'un dump utile, programmer délibérément des octets non vierges
sans format valide dans la région config. Démarrer le mode normal : attendu
mount failed, aucune réinitialisation automatique, région identique au dump.
Restaurer explicitement la région avant de poursuivre. Ce scénario est un test
fonctionnel de refus ; il ne corrige pas les corruptions dues à #70.

## 3. OTA A avec ConfigSpace EKV

Réinstaller le profil OTA complet : sa table et les offsets diffèrent du profil
standalone. Lire `examples/firmware/ota-system/README.md`, fournir SSID/password,
Bearer et certificat/clé TLS avec le SAN approprié, puis :

```sh
python examples/firmware/ota-system/prepare.py --chip esp32s3 \
  --partition-tool /absolute/path/esp-idf/components/partition_table/gen_esp32part.py
```

Le build valide les images aux deux offsets ; il produit bootloader, application,
otadata, table et config.bin vierge. Pour cette première installation/reset
volontaire, suivre la commande de flash complète du README : bootloader0,
table0x8000, otadata0xd000, app0x20000, config0x3c0000. **Config.bin et otadata
usine ne sont jamais reflashés entre les essais de conservation ou les OTA.**
Les images locales de validation sans credentials ne sont pas des images de
campagne réseau opérationnelles ; reconstruire avec les paramètres de la carte.

| Essai | Attendu |
| --- | --- |
| Premier boot / 10 reboots après stable | config montée, pas de canary écrit, boot stable et HTTPS disponible |
| Upload nominal sans activation | staged uniquement après digests, validation d'image et persistance OTM1 |
| Reboot après staged | état staged récupérable ; aucune activation automatique |
| Activation du même identifiant | reboot, PendingVerify puis confirmed, métadonnées terminées |
| Deuxième mise à jour dans l'autre sens | même résultat avec deux images issues du nouveau backend |
| Mauvais digest / identifiant d'activation | refus, slot confirmé conservé |
| Requête interrompue avant publication, alimentation maintenue | pas de publication incomplète ; nouveau transfert possible |
| Variante reject-self-check | rejet et rollback vers une image nominale EKV |
| Variante hang-self-check | timeout/watchdog et rollback nominal EKV |
| Reboot après confirmation | stabilité, metadata/config intactes |

Utiliser client.py et les variantes prepare.py décrits dans le README. Ne pas
utiliser d'ancienne image NVS comme fallback. Ne pas qualifier les coupures
pendant les commits ConfigSpace dans ce bilan nominal : les résultats connus
et leur réactivation restent dans #70. B/supervised n'a toujours pas d'exécutable.

## Décision de qualification

Joindre logs/dumps/checksums et marquer chaque ligne PASS/FAIL/non exécutée.
Ne pas confondre CI verte (test de coupures ignoré) et recovery validé. Relever
sur la carte les timings de mount/self-check/commit, le heap/pile et les éventuels
resets. Les mesures d'usure hôte accompagnent la conception mais ne donnent pas
une durée de vie physique sans l'endurance de la puce et la fréquence métier.
#68 reste ouverte jusqu'au bilan matériel prévu ; #70 reste ouverte séparément.

## Bilan de campagne — 14 octobre 2026

Commit testé : `4c00423a2a992ec43e0831d635d15fbe2d0e6ea5` (branche
`refactor/68-configspace-record`, PR #69), sans modification locale hors
variante de qualification signalée. **S3 seulement ; C3 : non exécuté** (aucune
carte C3 au banc).

### Matériel et outils

- ESP32-S3 DevKitC-1 N16R8 : QFN56 rév. v0.2, flash ID 0x184068 (16 Mio),
  PSRAM 8 Mio, pont UART CH343 ; images à en-tête DIO 4MB.
- esp 1.98.1.0 (rustc 1.98.1-nightly), nightly 1.101.0, esptool 5.5.0,
  `gen_esp32part.py` ESP-IDF v5.5.1, espflash 4.6.0 (bootloader seulement).
- Livraison : ESP Web Tools 10.4.0 servi localement (écriture/effacement via
  esptool-js), relectures flash en lecture seule par esptool-js 0.7.0. Console
  série Web Serial capturant l'UART et injectant les resets EN (RTS) ; OTA par
  `client.py` et un client d'injection, TLS vérifié par CA d'essai.
- Identifiants Wi-Fi, Bearer et clé TLS intégrés au build, jamais joints.

### Résultats hôte et builds (rappel, non rejoués sur carte)

Tests hôte ConfigSpace/intégration 24 PASS + 1 ignoré (#70), OTA 72 PASS, liens
C3/S3 et CI verte : constatés sur ce commit avant la campagne. Ils ne
qualifient pas la reprise après coupure pendant un commit EKV (#70).

Builds de campagne (S3, release) : config-manager normal, `exercise` et
`ota-system` nominal/`reject-self-check`/`hang-self-check` construits sans
erreur ; seul l'avertissement upstream RWX du linker S3 apparaît.

| Image | SHA256 |
| --- | --- |
| config-manager : bootloader ESP-IDF espflash, DIO 4MB | `4c76d5583d5b0de5cec001ab48e3197b0e215f93b3630d524a346dcf3f61083a` |
| config-manager : table standalone | `0d6004ddfa921f4e29fc0b28b14b4bd631eabf5c1f82634f3ed83428bd24f272` |
| config-manager normal `app.bin` | `37d431dbd4ef708479cfa3badd8840be7fb3fe3ecb185bd85df713ebd27d3357` |
| config-manager `exercise` `app.bin` | `ab5b61c18325d8807b0b635aad3ab091d7ef3f017ddb0716ae9f1dd4dbb74fe7` |
| config-manager image fusionnée d'installation | `8ae5f2305462d59cf00ac9146dce30ec84ed4da7235a87005b072fd8b1f22d1b` |
| ota-system bootloader | `8719f258fcf11a0f97fd83dff98f47d1cd80c93b315ad2b47b382b4384557ce7` |
| ota-system table | `9d5adf22f31983531ea87bd265bbdb52be649469c06bb74663bfb37ebf12d339` |
| ota-system otadata usine | `d3137c5ca187cb7a334d791ba830663cbef8ffaa9d387ce5e1cb63e1c9ac865d` |
| ota-system config.bin vierge | `3b874d3ba46c638fc3094f8e92fb744ca974893873f8885f54e23760f9b6311b` |
| ota-system nominal `app.bin` | `6d3b67493db29ec6cafa656076fd4d3d90a2a94401130a1226f8fb474421cf08` |
| ota-system `reject-self-check` `app.bin` | `a58420bff31ead28a5a2f6ca7812d64b9f54dcffdcfb91a3a8421c4337f9a9c2` |
| ota-system `hang-self-check` `app.bin` | `607be3dcd9bed89c8ef2d982dcb022eb9acd824b209d2fee856b2b589bd30d7a` |

### Résultats matériels — ConfigSpace isolé (config-manager)

| Essai | Résultat | Trace |
| --- | --- | --- |
| Premier boot sur région vierge | **PASS** | rejoué avec capture : effacement complet + image fusionnée (config à 0xFF dans l'image elle-même) écrits sans reset final, port gardé ouvert ; un seul `rst:0x1` capturé depuis la ROM, puis `config: initialized generation 1` et `verified generation 1`. Premier passage (ligne `initialized` non capturée) et essai intermédiaire (boot coupé 0,4 s après le démarrage par un reset EN superflu) écartés |
| 10 reboots logiciels | **PASS** | variante de qualification décrite ci-dessous (`software_reset()` 3 s après verified, lecture seule) : 71 boots `RTC_SW_SYS_RST` ; les 83 boots de CM2–CM3 (logiciels et alimentation) sont tous `retained`/`verified generation 1` |
| 10 cycles d'alimentation | **PASS** | 10 boots `POWERON` après débranchement des deux câbles USB, mode normal, tous `generation 1` |
| Variante exercise | **PASS** | génération 1 → 4 (premier boot non capturé, déduit), puis `commit/clear/recommit PASS generation 7` capturé |
| Reboot exercise (reset EN) | **PASS** | `retained generation 7`, puis `PASS generation 10` |
| Retour au mode normal | **PASS** | `retained`/`verified generation 10`, reset EN supplémentaire identique ; aucune réinitialisation |
| Refus d'une région non reconnue (optionnel) | **non exécuté** | exécuté ensuite : voir complément #71 |

Variante de reboots logiciels : l'exemple publié n'a pas de commande de
reboot. Cette modification de qualification, non destinée à la fusion, a été
appliquée sur `4c00423` puis retirée :

```diff
--- a/examples/config-manager/src/platform/esp32.rs
+++ b/examples/config-manager/src/platform/esp32.rs
@@ -31,7 +31,10 @@ async fn config_task(flash: FlashPeripheral<'static>) {
             panic!("config mount failed");
         });
     crate::app::run(store.backend()).await;
-    core::future::pending::<()>().await;
+    // QUALIFICATION ONLY (not committed): software reboot loop, read-only after init.
+    log::info!(target: "example_config_manager", "qual: software reset in 3 s");
+    esp_hal::delay::Delay::new().delay_millis(3000);
+    esp_hal::system::software_reset();
 }
```

Build : même commande que le mode normal (`--features esp32s3`), puis
`esptool elf2image --flash-mode dio --flash-size 4MB` ; installation de l'app
seule à 0x50000, sans effacement. Image testée SHA256
`c24bfc144875cc26f46087a92cf89f9e848bab29006a7d4fbc31a436152296c2`. Une
reconstruction ultérieure depuis ce patch ne diffère que par l'heure de
compilation de `esp_app_desc`, le SHA de l'ELF qu'il contient et la somme/SHA
finale de l'image (71 octets) ; toutes les images de ce bilan portent cet
horodatage.

Mesure (horodatage console, résolution ~0,2 s) : trois commits `exercise`
entre `verified` et `PASS` : 1,2 à 2,2 s.

### Résultats matériels — OTA A avec ConfigSpace EKV

Installation usine unique après effacement complet (table OTA, bootloader,
otadata usine, app nominale en ota_0, config.bin vierge). Ensuite, uniquement
des OTA : ni config.bin ni otadata usine n'ont été reflashés.

| Essai | Résultat | Trace |
| --- | --- | --- |
| Premier boot / reboots après stable | **PASS** | 20 resets EN : 20 × `ota_0 Stable`, 10 × `OTA ready` + `/health` 200 (TLS vérifié) |
| Pas de canary écrit au boot stable | **PASS** | relectures D0/D1 encadrant deux boots complets : table, otadata et config identiques ; config = un seul en-tête de page EKV (20 octets) |
| Upload nominal sans activation | **PASS** | `c68-n1` : écriture ota_1, `flash digest and image verified; publishing`, réponse `staged` (1 051 504 o en 19,6 s) |
| Reboot après staged | **PASS** | 2 boots `ota_0 Stable`, aucune activation ; activation ensuite acceptée |
| Activation du même identifiant | **PASS** | reboot 0,45 s → `ota_1 PendingVerify` → `confirmed` 0,77 s après boot ; réactivation de `c68-n1` : 409 |
| Reboot après confirmation | **PASS** | `ota_1 Stable`, HTTPS disponible ; idem après `c68-n2` et `c68-n5` |
| Deuxième mise à jour dans l'autre sens | **PASS** | `c68-n2` → ota_0 : PendingVerify → confirmed (1,3 s) → Stable |
| Mauvais digest | **PASS** | 409, aucun `publishing`, activation 409 |
| Identifiant d'activation inconnu | **PASS** | 409 |
| Requête interrompue, alimentation maintenue | **PASS** | `c68-tr2` coupé à 500 000 o : aucun `publishing`, aucun reboot, serveur rétabli en 12 s, activation 409, transfert complet suivant `staged` |
| `reject-self-check` | **PASS** | `ota_1 PendingVerify` → reset 0,15 s → `ota_0 Stable` (nominal EKV `c68-n2`) ; réactivation 409 |
| `hang-self-check` | **PASS** | PendingVerify → reset à 15,3 s (deadline 15 s) → `ota_0 Stable` ; réactivation 409 |
| Mise à jour nominale après rollbacks | **PASS** | `c68-n5` → ota_1 : confirmed 1,2 s, puis `ota_1 Stable` après reboot |
| Relecture finale config/otadata | **PASS** | D2 : table inchangée ; otadata ota_0 seq 3 VALID, ota_1 seq 8 VALID (slot courant, jamais remis à l'usine) ; config : 34 secteurs EKV écrits, en-tête initial conservé, records OTM1 des identifiants de campagne |

Remarque : un premier essai d'interruption a été invalidé par l'outil de banc,
qui a injecté un reset EN 3,4 s après le début de l'écriture ; il est consigné
comme reset pendant écriture (fallback intact, transfert suivant `staged`),
puis l'essai a été rejoué sans reset.

### Non qualifié par cette campagne

- Coupures/resets pendant un commit ConfigSpace, une publication, une
  activation ou une confirmation : hors campagne nominale, suivi #70.
- Heap et pile : non mesurés (mesurés ensuite sur S3 : complément #71).
- C3 : non exécuté sur carte.
- La première association Wi-Fi échoue environ une fois sur deux
  (`DisassociatedDueToInactivity`) ; l'exemple ne retente pas et nécessite un
  reset (suivi #72). Même symptôme et même fréquence relevés le 13 octobre avec le backend
  NVS (`docs/firmware/COMPTE-RENDU-A.md`, observations hors chaîne A), donc
  indépendant du changement de backend. À suivre séparément.

### Verdict

Validé fonctionnellement sur ESP32-S3 en fonctionnement nominal : ConfigSpace
EKV isolé et OTA A avec ConfigSpace EKV, sans correction de code. Non validés :
C3, heap/pile, refus d'une région non reconnue, et toute interruption pendant
une écriture EKV, une publication, une activation ou une confirmation (#70).

## Complément #71 — 14 octobre 2026

Commit testé : `e6e274d762642d328c7e1496dfae14b9de773824` (`port/61-write-authority`,
fusion de #69). Son code est identique à `4c00423` (seul ce document diffère) :
les images nominales du bilan ci-dessus restent celles testées. Même carte
ESP32-S3 et mêmes outils ; écritures ciblées par esptool-js `writeFlash`
(adresse donnée, sans effacement global, sans reset final), puis réouverture
du port 115200 : le reset d'ouverture produit le seul boot, capturé en entier.

| Vérification | Résultat | Code |
| --- | --- | --- |
| Refus d'une région non reconnue | **PASS** | nominal, aucune instrumentation |
| Heap et pile | **mesurés** (marges ci-dessous) | instrumenté, qualification seulement |
| C3 : identité, capacité, table | **non exécuté** | aucune carte C3 au banc ; le correctif SPI1 USER reste mesuré sur S3 seulement |

### Refus d'une région non reconnue (code nominal)

Carte en config-manager normal (`37d431db…3357`), table standalone, config à
0x9000/0x40000, génération 1. Relectures en lecture seule (esptool-js
`readFlash`, région 0x9000/0x40000) :

| Étape | Action | Constat |
| --- | --- | --- |
| R0 | relecture de sauvegarde | `0f47cdc89672f05d2dd86e97d082a0f9661d20edec9fc2645f94210c32af49b4` ; deux pages EKV (0x36000, 0x37000), reste 0xFF |
| W1 | écriture de `not-ekv-256k.bin` à 0x9000 : motif `IOBEWI-QUAL-NOT-EKV-` répété sur 256 Kio, SHA256 `5c1bd73e9dfa9c8b2b2f651960d51e1afb4a50f8075bd119c8ad24bd405ce5cc` | boot capturé : `ERROR - config: mount failed Mount(Corrupted); explicit reset required`, puis plus aucune sortie (panic silencieux) |
| R1 | relecture (suivie du reset matériel de fin de relecture : nouveau boot sur la région) | SHA256 `5c1bd73e…ce5cc`, identique octet pour octet à l'image écrite |
| R2 | relecture après ce boot supplémentaire | identique à l'image écrite |
| W2 | restauration explicite : écriture de R0 à 0x9000 | boot capturé : `retained generation 1`, `verified generation 1` |
| R3 | relecture finale | identique à R0 (`0f47cdc8…49b4`) |

Verdict : **PASS**. Deux boots sur la région non reconnue l'ont refusée sans
écriture, sans formatage et sans réinitialisation automatique ; la restauration
explicite rend le fonctionnement nominal, et ce boot nominal n'écrit rien.
Ce test ne qualifie pas la reprise après coupure (#70).

### Heap et pile (code instrumenté)

Moyens existants : esp-alloc fournit `HEAP.stats()`/`free()` ; pour la pile,
esp-hal n'offre que la détection de débordement (garde `__stack_chk_guard`,
contrôle de plage du pointeur de pile), pas de mesure d'usage. Une sonde de
qualification, non destinée à la fusion, a donc été ajoutée hors dépôt :

- **heap** : feature esp-alloc `internal-heap-stats` (compteurs `current_usage`
  et `max_usage` mis à jour à chaque allocation/libération) ; `min_free =
  size − max_usage`, minimum depuis le boot ;
- **pile** : au début de `main`, avant `esp_hal::init`, la pile principale cpu0
  (`_stack_end_cpu0` .. `_stack_start_cpu0`) est peinte en `0xA5A5A5A5` de
  `_stack_end_cpu0 + 256` (garde à +60 non touchée) jusqu'à 1 Kio sous le cadre
  du peintre ; chaque sonde cherche le premier mot modifié. Valeur = maximum
  depuis le boot (haute eau cumulée), pas l'usage instantané.

Piles couvertes : la pile principale cpu0, qui porte la tâche principale
esp-rtos, l'exécuteur Embassy (tâches config, boot, OTA, HTTP/TLS) et les
interruptions prises pendant leur exécution. **Non couvertes** : les piles des
tâches radio esp-rtos, allouées dans le heap esp-alloc (comptées dans l'usage
heap, mais leur profondeur n'est pas mesurée) ; le cpu1 n'est pas démarré par
ces exemples. Limites : la zone non peinte en haut (1 152 o pour
config-manager, 2 304 o pour ota-system) est comptée comme utilisée ; un
appel profond bref entre deux sondes est capté par la sonde suivante (haute
eau) ; la mesure peut sous-estimer si les mots les plus profonds d'un cadre
ne sont jamais écrits ou reçoivent exactement le motif ; aucune sonde n'a
signalé l'épuisement de la zone peinte.

Coût de l'instrumentation : app `exercise` +1 216 o (173 440 contre 172 224) ;
app ota-system −448 o (1 051 056 contre 1 051 504, effet de mise en page du
compilateur) ; aucune allocation ajoutée ; coût CPU des compteurs heap et durée
du marquage au boot non mesurés. Les valeurs ci-dessous valent pour ces images
instrumentées ; elles ne remplacent pas une mesure du code nominal, qui n'est
pas instrumentable sans modification.

**config-manager** (`--features esp32s3,exercise` + patch, app
`ca62b749a03cf2ac6eeeb65180f6440b650cadadc395f24cd2cc4ca76e14ef4b`) : heap
65 536 o, pile principale réservée 246 564 o.

| Point (cumulé depuis le boot) | Région existante (gén. 1 → 4) : heap max / min libre | pile max | Région vierge (format, gén. 1 → 4) : heap max / min libre | pile max |
| --- | --- | --- | --- | --- |
| début de tâche | 0 / 65 536 | 5 900 | 0 / 65 536 | 5 900 |
| après `Store::new` | 0 / 65 536 | 10 048 | 0 / 65 536 | 10 048 |
| après `format` | — | — | 0 / 65 536 | 10 240 |
| après `mount` | 8 192 / 57 344 | 10 368 | 8 192 / 57 344 | 10 240 |
| après commit/clear/recommit | 8 192 / 57 344 | 10 976 | 8 192 / 57 344 | 10 976 |

Usage heap courant à 0 à chaque sonde (tampon de montage libéré). Marges
minimales : heap 57 344 o (87 %), pile 235 588 o (95 %).

**ota-system** (prepare.py, variante sans défaut + patch, app
`7d62f29dd0036fa5f8a6aacd481b9ade85b875b346d62ce9ec64387941485bd0`, bootloader,
table, otadata usine et config.bin identiques au bilan #68) : heap 163 840 o,
pile principale réservée 73 912 o. Installation usine après effacement
(ota_0), puis upload de la même app vers ota_1 (`c71-mem-1`), activation et
confirmation.

| Point (cumulé depuis le boot) | heap courant | heap max / min libre | pile max / marge |
| --- | --- | --- | --- |
| début de `boot_task` | 0 | 0 / 163 840 | 15 980 / 57 932 |
| après `mount` | 0 | 8 192 / 155 648 | 20 272 – 22 304 / 51 608 – 53 640 |
| `boot: stable` | 104 | 8 296 / 155 544 | 23 312 / 50 600 |
| `OTA ready` (Wi-Fi, TLS prêts) | 47 868 | 49 560 – 50 160 / 113 680 | 23 312 / 50 600 |
| upload : écriture, digest flash et validation | 84 496 | 99 504 / 64 336 | 35 360 / 38 552 |
| après publication OTM1 | 84 484 | 99 504 / 64 336 | 35 360 / 38 552 |
| activation acceptée (avant reboot) | 84 412 | 99 504 / 64 336 | 35 360 / 38 552 |
| reboot : `ota_1 PendingVerify` → `confirmed` | 104 | 8 296 / 155 544 | 23 536 / 50 376 |

Marges minimales observées (pendant l'upload HTTPS de 1 051 056 o) : heap
64 336 o libres (39 %), pile principale 38 552 o (52 %). Publication et
activation n'élèvent aucun maximum. Les piles des tâches radio sont incluses
dans l'usage heap mais leur marge propre n'est pas mesurée.

Commandes : sonde `qual_mem.rs` incluse par `#[path]`, patchs de qualification
appliqués sur `e6e274d` puis retirés (dépôt propre après build) ;
config-manager compilé par la commande de la section 1 avec
`--features esp32s3,exercise` puis `esptool elf2image --flash-mode dio
--flash-size 4MB` ; ota-system par `prepare.py --chip esp32s3`. Écritures :
app instrumentée à 0x50000 (région existante), puis 256 Kio de 0xFF à 0x9000
(région vierge) ; ota-system par installation usine avec effacement, puis
`client.py --activate`. Pendant cette mesure, deux premières associations Wi-Fi
ont échoué (`DisassociatedDueToInactivity`) et ont été levées par un reset
manuel : symptôme #72, non investigué ici. Aucune coupure pendant écriture
(#70) n'a été provoquée.

Traces, dumps, images et patchs archivés hors dépôt (identifiants Wi-Fi
masqués) ; empreintes : sonde `8d5eb7d9…048e`, patch config-manager
`b8f54033…f2be4`, patch ota-system `577b5763…87f4cc`.

### Verdict du complément

Sur ESP32-S3 : refus d'une région non reconnue **PASS** (code nominal) ;
heap et pile principale mesurés avec marges positives sur les scénarios
nominaux (code instrumenté), piles radio non mesurées. **C3 non exécuté** :
#71 reste ouverte pour cette vérification.
