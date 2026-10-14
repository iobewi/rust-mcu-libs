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
| Premier boot sur région vierge | **PASS (indirect)** | carte effacée puis installée ; la ligne `initialized generation 1` n'a pas été capturée (boot exécuté avant reconnexion de la console) ; tous les boots suivants : `retained`/`verified generation 1` |
| 10 reboots logiciels | **PASS** | variante de qualification non commitée (`software_reset()` 3 s après verified, lecture seule) : 71 boots `RTC_SW_SYS_RST` ; les 83 boots de CM2–CM3 (logiciels et alimentation) sont tous `retained`/`verified generation 1` |
| 10 cycles d'alimentation | **PASS** | 10 boots `POWERON` après débranchement des deux câbles USB, mode normal, tous `generation 1` |
| Variante exercise | **PASS** | génération 1 → 4 (premier boot non capturé, déduit), puis `commit/clear/recommit PASS generation 7` capturé |
| Reboot exercise (reset EN) | **PASS** | `retained generation 7`, puis `PASS generation 10` |
| Retour au mode normal | **PASS** | `retained`/`verified generation 10`, reset EN supplémentaire identique ; aucune réinitialisation |
| Refus d'une région non reconnue (optionnel) | **non exécuté** | — |

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
- Heap et pile : non mesurés.
- C3 : non exécuté sur carte.
- La première association Wi-Fi échoue environ une fois sur deux
  (`DisassociatedDueToInactivity`) ; l'exemple ne retente pas et nécessite un
  reset. Comportement déjà constaté avec le backend NVS, sans rapport avec EKV.
