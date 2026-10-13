# Qualification matérielle OTA A — ESP32-S3

## État actualisé au 13 octobre 2026

La chaîne A est validée fonctionnellement sur l’ESP32-S3 du banc avec les corrections de qualification. Sa robustesse complète aux interruptions de persistance n’est pas qualifiée : le défaut ConfigSpace/NVS est suivi dans [#68](https://github.com/iobewi/rust-mcu-libs/issues/68#issuecomment-6151095751). [#61](https://github.com/iobewi/rust-mcu-libs/issues/61) reste ouverte ; [#65](https://github.com/iobewi/rust-mcu-libs/pull/65) reste Draft, non fusionnée.

Les cinq corrections ont été publiées sur `port/61-write-authority`, après `3e53dd0` :

| Commit | Correction |
| --- | --- |
| `a0c1b3b` | Packaging DIO |
| `212a650` | Visibilité des logs boot/réseau/OTA |
| `9f6d203` | RDID S3 : neutralisation temporaire puis restauration de SPI1 USER |
| `322724f` | Configuration picoserve ; timeout de requête OTA 120 s |
| `a85db0c` | Relecture et comparaison des commits ConfigSpace |

Révision logicielle corrigée : [a85db0c3e4089a78a5f10ca30c4a28a202b0cfcb](https://github.com/iobewi/rust-mcu-libs/commit/a85db0c3e4089a78a5f10ca30c4a28a202b0cfcb).
CI : [run 38437006487](https://github.com/iobewi/rust-mcu-libs/actions/runs/38437006487), terminé **SUCCESS**. Ce résultat ne constitue pas une nouvelle campagne matérielle après publication.

La campagne ci-dessous a été conduite par Claude sur le banc du mainteneur, avec `3e53dd0 + patches sous test`. Ses résultats sont repris du compte rendu fourni ; aucune nouvelle mesure carte n’est revendiquée par cette actualisation. Les artefacts/hashes abrégés ne sont pas une preuve que chaque binaire du HEAD publié a été reflasher et retesté. Dumps, reproductions, patches d’instrumentation et hashes complets restent à joindre/référencer pour reproduction indépendante.

### Synthèse de couverture

- **PASS S3** : premier boot/reboot stable, OTA dans les deux sens, upload sans activation, mauvais ID, digest incorrect, image invalide, transfert interrompu, authentification, rollback sur refus/timeout, watchdog physique, reset pendant écriture, reset avant confirmation et vraie coupure pendant écriture suivie d’une nouvelle OTA réussie.
- **Partiel** : publication OTM1 sous resets EN (fenêtre exacte non garantie) ; exclusion pendant probation prouvée au niveau transport, refus internes du service testés sur hôte.
- **Défaut démontré** : interruption pendant confirmation → corruption NVS/perte OTM1. La relecture ajoutée dans `a85db0c` empêche le faux succès ; elle ne répare pas la persistance. Ce correctif est validé par rejeu hôte, pas par un nouvel essai carte sur NVS corrompue.
- **NON TESTÉ** : I3 (interruption ciblée d’activation/otadata), I5 (échec injecté de finition OTM1 après confirmation), vraies coupures ciblées publication/activation/confirmation, carte C3, mesures heap/pile.
- **B** : composition matérielle et qualification `ota-supervised` restent à réaliser ; aucune validation A ne vaut validation B.

Le prochain chantier décidé par le mainteneur est #68. Les essais de persistance concernés devront être rejoués après sa correction. Une éventuelle fusion/clôture demande une décision explicite sur les critères restants ; cette mise à jour ne les supprime pas.

## Compte rendu original de campagne — état avant publication des corrections

Le texte ci-dessous est conservé pour traçabilité. Ses mentions « local/non commité », « CI non relancée », « rien poussé » et ses prochaines étapes décrivent l’état au moment du rapport, remplacé pour la publication/CI par l’état actualisé ci-dessus. Les observations et limites matérielles restent inchangées.

### Qualification matérielle OTA A — compte rendu intermédiaire (ESP32-S3)

**Statut : campagne S3 largement exécutée, 5 défauts démontrés et corrigés localement (non commités, CI non relancée). Rien n'a encore été poussé, aucun commentaire sur #61/#65.**

## Contexte
- Dépôt `iobewi/rust-mcu-libs`, branche `port/61-write-authority`, HEAD distant vérifié `3e53dd02642ca66e289e8bc27f309d09c59509d6` (inchangé).
- Carte : ESP32-S3 DevKitC-1 N16R8 — QFN56 rév. v0.2, flash 16 Mo (ID 0x184068), PSRAM 8 Mo, pont CH343. **C3 : NON TESTÉ sur carte** (aucune carte ; compilation seulement).
- Outils : esp toolchain 1.98.1.0 (rustc 1.98.1-nightly), nightly 1.101 + riscv32imc, esptool 5.5.0, `gen_esp32part.py` ESP-IDF v5.5.1, ESP Web Tools 10.4.0 (livraison usine), esptool-js 0.7.0 (relecture flash en lecture seule).
- Banc : serveur local + console Web Serial (UART CH343 et USB-JTAG) capturant les logs et injectant des resets EN (RTS) sur motif de log. TLS vérifié par `client.py` avec une CA d'essai, SAN `iobewi-ota.test`. SSID masqué dans tout ce qui sera publié.
- Installation usine via ESP Web Tools (sans effacement ; manifeste `new_install_prompt_erase: true`). Ensuite toutes les mises à jour par OTA, sauf réinstallations de l'app seule en 0x20000 tant que le firmware ne pouvait pas recevoir d'OTA (documentées).

## Défauts démontrés sur carte et corrections (locales)
| # | Défaut | Preuve | Correction |
|---|---|---|---|
| 1 | `prepare.py` génère des en-têtes **QIO** : la ROM S3 n'arrive pas à charger le 2ᵉ étage (`ets_loader.c 78`, boucle watchdog) sur cette flash | Même ELF repackagé en DIO : la ROM charge les 4 segments | `prepare.py` : `--flash-mode dio` + contrôle de l'octet de mode |
| 2 | Logs Info de l'exemple filtrés (cible du binaire `firmware_ota_system_esp32` ≠ `example_firmware_ota_system`) : slot exécuté, IP, `ota:` invisibles | Seuls les `log_status` explicites sortaient | `main.rs` : politique de logs, 2 cibles ajoutées (binaire, `iobewi_esp_wifi`) |
| 3 | `esp-storage` 0.10 dimensionne la flash par RDID matériel, faussé par `SPI_MEM_USER` laissé par la ROM DIO : **capacité 0**, toute lecture `OutOfBounds`, panique `NVS unavailable: Discovery(TableUnreadable)` | Mesure in situ : USER brut → `0x…860184`, USER=0 → `0x184068`. USER=0 permanent casse les lectures ; mise à 0 côté bootloader inefficace (retirée) | `iobewi_esp_flash::init` (S3) : sauvegarde USER, 0 le temps de `FlashStorage::new`, restauration. Mesuré : `capacity 0x1000000`, table lue |
| 4 | picoserve 0.20.1 borne **toute la requête, corps compris**, par `read_request` = 3 s : upload coupé à 3,09 s, 409 | `body read error after 169984 bytes` à 3,09 s | `iobewi-http-server` : ajout `serve_forever_tls_with(listener, router, &config)` (existants inchangés) ; exemple : `read_request` 120 s (mesuré ~52 Ko/s → slot 1,5 Mio ≈ 28 s). Contrepartie : serveur mono-connexion occupable 120 s |
| 5 | Après resets EN pendant des écritures NVS : **NVS corrompue** (en-tête de page corrompu, page active sans entrée valide), OTM1 perdu ; upload répond « staged » mais activation 409 persistante | Rejeu hôte `esp-nvs` 0.5 sur le dump réel : `set` → `Ok` puis `get` → `KeyNotFound`. Injection de coupures hôte : écriture **déchirée** → perte silencieuse permanente ; coupure **propre** → `Nvs::new` panique à la réouverture (non observé sur carte) | Notre backend ConfigSpace : relecture comparée après `set`, sinon erreur (plus de faux « staged »). **Vérifié par rejeu hôte uniquement** : sur carte, la NVS s'est rétablie d'elle-même avant le test. Défaut `esp-nvs` amont : **ouvert, à suivre** |

Diff complet sous test : `fixes-under-test.patch` (SHA256 `2a1cc7c0…`), 6 fichiers, +74/−6. `cargo fmt --check` OK ; tests hôte `iobewi-http-server` 8/8. **Clippy, tests hôte complets, builds C3/S3 et CI non encore relancés.**

Instrumentation de qualification (patches séparés, jamais destinés à la fusion) : impression des paniques, lectures flash/RDID, motifs de refus d'upload, auto-test bloquant l'executor (`qual-spin-self-check.patch`).

## Artefacts finaux (commit 3e53dd0 + patch sous test)
- bootloader `8719f258…57ce7` (code d'origine, en-tête DIO), partitions `71b95aca…cbf8`, otadata usine `d3137c5c…865d`
- app nominale `dbd82b4a…5da1`, reject `e7e3b2ba…0bac8`, hang `065d920c…36b6`, spin (qualification) `0a181e3b…3ba6`

## Matrice des essais (S3)
| Essai | Résultat | Preuve clé |
|---|---|---|
| 1 Premier boot + reboot à froid | **PASS** | `rst:0x1 POWERON` → `running ota_0 status Stable`, `boot: stable` |
| 2 Upload nominal sans activation | **PASS** | `staged` en 21 s ; après reset, toujours ota_0 Stable |
| 3 Activation avec mauvais deployment ID | **PASS** | 409 `activation refused`, pas de reboot |
| 4 Activation ota_0 → ota_1 | **PASS** | SW reset 0,4 s → `ota_1 PendingVerify` → `confirmed` (1,0 s) → reboot `ota_1 Stable` |
| 5 Mise à jour ota_1 → ota_0 | **PASS** | même séquence, confirmé en 1,2 s, reboots Stable |
| 6 Mauvais digest | **PASS** | 409, jamais `publishing`, activation 409 |
| 7 Transfert interrompu | **PASS** | coupure à 500 000 o ; activation 409 ; nouveau transfert complet `staged` |
| 8 Image non bootable, digest correspondant | **PASS** | 409, jamais `publishing` |
| 9 Sans token / token faux | **PASS** | 401 upload et activation, aucune écriture flash |
| 10 `reject-self-check` | **PASS** | `ota_1 PendingVerify` → reset 0,4 s → `ota_0 Stable` |
| 11 `hang-self-check` (timeout coopératif) | **PASS** | PendingVerify → SW reset à **15,2 s** → fallback Stable |
| 12 Transfert pendant Activating/PendingVerify | **PASS (transport)** | serveur non démarré pendant la probation, connexion impossible, rien écrit, fallback intact. Refus service (`Conflict`/`PendingConfirmation`) : **tests hôte seulement** |
| I1 Reset EN pendant écriture | **PASS** | reset à +5,27 s → ota_0 Stable, ID non activable, nouvel upload OK |
| I2 Reset EN à la publication OTM1 (0/30/80 ms) | **Cohérent, fenêtre non garantie** | fallback intact, upload suivant OK ; `i2-pub-80` non publié |
| I3 Reset pendant activation / écriture otadata | **NON TESTÉ** | aucun point de log avant l'écriture otadata côté serveur |
| I4a Reset EN avant confirmation | **PASS** | `PendingVerify` → reset → `ota_0 Stable`, ID non activable |
| I4b Reset EN pendant confirmation (950/1050/1150 ms) | **FAIL → défaut #5** | otadata sain et confirmé, mais NVS corrompue : OTM1 perdu, OTA bloquée jusqu'à résorption spontanée |
| I5 Échec de finition OTM1 après confirmation (Degraded) | **NON TESTÉ** | pas d'injection maîtrisée ; jamais observé |
| W1 Expiration **physique** du watchdog | **PASS** | executor bloqué (spin) → `rst:0x7 TG0WDT_SYS_RST` → bootloader : fallback `slot=1`, écrit `state=4` (ABORTED) → `ota_1 Stable` |
| P1 **Vraie coupure d'alimentation** pendant écriture du candidat | **PASS** | coupure USB (2 câbles) à +11 s d'écriture ota_0 → `POWERON` → `ota_1 Stable` ; ID coupé 409 ; mise à jour suivante p1-next staged → PendingVerify → confirmed → Stable |
| Vraie coupure pendant publication OTM1 / activation / confirmation | **NON TESTÉ** | fenêtres de quelques ms, non ciblables à la main ; seuls des resets EN y ont été injectés |

## Mesures disponibles
- Upload : ~52 Ko/s (1 026 448 o en 19,8 s, effacements 64 Kio compris) ; aller-retour client 21–25 s.
- Activation → reboot : 0,2–0,4 s ; boot → `confirmed` : 1,0–1,2 s ; timeout coopératif : 15,2 s (consigne 15 s) ; watchdog physique : ~24 s au timestamp hôte (consigne 20 s, horodatage navigateur imprécis).
- Heap / pile : **non mesurés** (seules les réservations de lien sont connues : heap 160 Kio).

## Observations hors chaîne A
- Wi-Fi : la 1ʳᵉ association échoue souvent (`DisassociatedDueToInactivity`, environ 1 boot sur 2) ; l'exemple ne tente qu'une fois puis panique (watchdog déjà désarmé en Stable, donc la carte reste figée). Laissé tel quel sur décision du mainteneur ; risque de robustesse à documenter.
- Correction #3 et #5 non appliquées/mesurées sur C3.

## Verdict A provisoire
Chaîne fonctionnelle de bout en bout sur S3 **avec les 5 corrections** : boot, transfert, vérification, publication, activation, probation, confirmation et rollback (refus, timeout coopératif, watchdog physique) qualifiés.
**Non qualifiable en l'état** sur la robustesse aux interruptions de la persistance ConfigSpace/OTM1 : défaut `esp-nvs` 0.5 démontré (perte silencieuse, panique possible à l'ouverture). Restent ouverts : I3, I5, coupures d'alimentation ciblées sur publication/activation/confirmation, C3, mesures heap/pile.

## Prochaines étapes proposées
1. Vérifications locales complètes (fmt, clippy, tests hôte, builds C3/S3), commit des 5 corrections sur `port/61-write-authority`, CI.
2. Compte rendu dans le dépôt + mise à jour #65 / avancement #61 (aucune fusion ni fermeture).
3. Issue de suivi : robustesse `esp-nvs` aux écritures interrompues (repro hôte fournie) ; décider correctif amont ou contournement.
4. Si souhaité : essais I3/I5 avec instrumentation, qualification C3.
