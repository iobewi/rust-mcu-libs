# Qualification matérielle USB / flux — issue #66

Campagne du 15 octobre 2026. Elle qualifie les briques portées par #75 et #76 sur
l'ESP32-S3 du banc. Aucun correctif des briques n'a été nécessaire ; seul le firmware
de qualification (jetable) a été ajusté.

## Commit testé

| Élément | Valeur |
| --- | --- |
| Base | PR #76, HEAD `87949eb` (empilée sur PR #75, HEAD `9a3a22a`, ancêtre vérifié) |
| Firmware de qualification | `examples/usb-stream-qual`, branche `qual/66-hw-bench` |
| Premier firmware | `d5d61a2`, image `329517034bb2ecea8251c6368a38916dfdfaa9a3d0d56361ecdf988dded70da6` |
| Firmware de la campagne | `6d3fe40` (journaux MSC sur la console, correction du journal `RESUME`), image `b1798d43ed6ef47da77f1a0fb223b3213583995da72f2135f9c02b83be5ed775` |
| Variante redémarrage logiciel | même source, `QUAL_SOFT_RESTART_S=90`, image `435b46785d2d2013e44bd556cfab52bc617135a23cd1f55de51a2f4d27278a20` |
| Outils hôte | `verify.py` (continuité), `scsi.py` (SCSI pass-through, commit `2a6f09e`) |
| Toolchain | `cargo +esp`, `-Z build-std=core,alloc`, `xtensa-esp32s3-none-elf`, release |

Les résultats SCSI, de continuité à 10 minutes, de reconnexion et de redémarrage
logiciel viennent du firmware `6d3fe40`. Les essais FAT16 / continuité de 60 s et
l'observation du CSW hors limites avec phase de données viennent de `d5d61a2`.

## Banc

- Carte : ESP32-S3 (QFN56) rev v0.2, flash 16 Mio, MAC `9c:13:9e:ac:4e:8c`.
- UART : pont CH343, observé et piloté par le banc (page Web Serial, `bench_server.py`).
- OTG : prise USB native (GPIO19/20) branchée sur un PC Windows, lecture du volume brut
  `\\.\D:` en administrateur. Le banc ne pilote pas ce port ; les débranchements ont
  été faits à la main. Une seule pile hôte (Windows) a été exercée.
- Conservation : la carte portait la disposition A/B iOBEWi (bootloader, `otadata`,
  `ota_0` stable, `config`). Sauvegardes en lecture seule avant écriture :
  `boot` `118e03d5…3a04`, `otadata` `027ac920…3df2`, `ota_0` `883759f9…cb53`.
  Seule la zone `ota_0` a été écrite (sans effacement global) puis **restaurée**
  (sha identique, `boot: running ota_0 status Stable`).
- Le Wi-Fi n'est pas utilisé par le firmware de qualification.

## Composition et paramètres

Producteur déterministe → `iobewi-rolling-stream` → `iobewi-fat16` → `iobewi-usb-msc` →
`iobewi-esp-usb` (OTG FS S3).

| Paramètre | Valeur |
| --- | --- |
| Fenêtre rolling | 16 384 o ; rétention 4 096 o ; avance maximale 8 192 o |
| Producteur | 512 o toutes les 20 ms (≈ 25,6 Kio/s) |
| FAT16 | 4 secteurs/cluster, 20 000 clusters, 16 entrées racine ; 80 160 secteurs, dernier LBA 80 159, données au LBA 160, `LIVE.BIN` 40 960 000 o |
| Politique d'attente | nouvel essai toutes les 5 ms ; `ZeroFill` après 2 000 ms sans données |
| Suspensions du producteur | dès 20 s puis toutes les 30 s, alternant 1 s (courte) et 4 s (longue) |
| Identité | VID/PID `1209:0001` (essai), `IOBEWI  ` / `USB STREAM QUAL ` / `0066` |
| Motif | enregistrements de 16 o : `n` (u64 LE) puis `n × 0x9E3779B97F4A7C15` |

Un secteur entièrement nul est la réponse explicite `ZeroFill` ; il est compté à part.
Tout autre secteur doit suivre le motif ; perte, répétition et corruption sont détectées.

## Résultats

| Essai | Résultat | Preuve |
| --- | --- | --- |
| Énumération réelle | **PASS** | `session begin` / `msc: connected` ; le disque `D:` apparaît |
| Identité INQUIRY | **PASS** | disque (type 0), amovible, `IOBEWI  ` `USB STREAM QUAL ` `0066` |
| Capacité | **PASS** | READ CAPACITY : dernier LBA 80 159, 512 o, 41 041 920 o |
| Lecture seule | **PASS** | MODE SENSE(6) : `0x80`, protégé en écriture |
| FAT16 (métadonnées) | **PASS** | boot sector, racine, `LIVE.BIN` 40 960 000 o ; tous les champs égaux à `PARAMS` |
| Continuité (60 s) | **PASS** | 2 728 secteurs, 0 corrompu |
| Continuité (10 min) | **PASS** | 27 104 secteurs (13 877 248 o) en 600,1 s, ≈ 847 bouclages de la fenêtre, **0 corrompu** |
| Sous-alimentation, pause longue | **PASS** | 10 pauses de 4 s sur 10 min ⇒ 20 secteurs nuls, tous par paires, `ZeroFill` UART = 20 |
| Sous-alimentation, pause courte | **PASS** | 10 pauses de 1 s, aucun `ZeroFill`, reprise sans perte |
| Contre-pression | **PASS** | à l'arrêt de l'hôte, avance = 8 192 o exactement, producteur retenu |
| Erreur SCSI : LBA hors limites | **PASS** | CHECK CONDITION, sense `5 / 0x21` (sans phase de données hôte) |
| Erreur SCSI : opcode non pris en charge | **PASS** | CHECK CONDITION, sense `5 / 0x20`, `msc: unsupported SCSI opcode` |
| REQUEST SENSE | **PASS (par conception)** | la sense n'est rapportée qu'une fois ; Windows l'a consommée par sa commande automatique, la suivante renvoie « pas d'erreur » |
| Déconnexion/reconnexion ×3 | **PASS** | trois `session end` (`consumed` = octets lus par l'hôte) + `session begin base=B` sans `BOOT` ; `first record n × 16 = B` (584 704, 1 005 568, 1 483 264) ; 0 corrompu |
| Démarrage à froid | **PASS** | `rst:0x1 POWERON`, `BOOT reset_reason=ChipPowerOn`, énumération, motif correct |
| Redémarrage logiciel | **PASS** | `rst:0x3 RTC_SW_SYS_RST`, `reset_reason=CoreSw`, nouvelle session en < 1 s, `first record n=192` = 3 072 / 16, 0 corrompu |
| Reset PHY après reset | NON EXÉCUTÉ | pas d'outil du banc sur le port OTG |
| Lecteur Metronic | NON EXÉCUTÉ | hors #66, relève de #67 |
| Autre hôte (Linux, macOS), autre famille de MCU | NON EXÉCUTÉ | un seul hôte Windows |

## Observations et limites

1. **Limite BOT documentée, confirmée sur matériel.** Sur une lecture en échec avec phase
   de données attendue, le MSC envoie le CSW (`status=1`, `residue=512`) sans phase de
   données ni STALL. Windows reçoit le CSW à la place des données, expire (erreur 121)
   et réinitialise le périphérique ; la session se termine et une nouvelle commence, le
   disque reste utilisable. Le comportement historique n'a pas été modifié, conformément
   au périmètre. Les erreurs ont donc été observées avec des commandes sans phase de
   données hôte (valides en BOT).
2. **Déconnexion non détectée avant le rebranchement.** Sur ce montage, le retrait du
   câble n'est pas vu par le firmware : `session end` et `session begin` portent le même
   horodatage, celui du rebranchement. Le producteur continue entre-temps (la fenêtre se
   rebase à `written − 4096`) et aucune donnée périmée n'est rejouée. Il s'agit d'une
   propriété du montage (pas de détection de VBUS), pas de la brique.
3. **Un cycle s'est terminé par un `POWERON`.** La première
   série de « cycles » a fait trois démarrages à froid (un redémarrage par cycle,
   probablement le retrait de plus que le seul câble OTG, ou la réouverture du port UART
   par la page du banc, qui déclenche un reset). Ils ont été consignés comme démarrages
   à froid et non comme déconnexions ; trois déconnexions propres ont ensuite été
   exécutées. La cause exacte n'est pas établie ; anomalie du banc, extérieure au
   périmètre.
4. **WRITE(10).** Windows refuse la commande lui-même sur un volume protégé en
   écriture (erreur 5) : elle n'atteint pas le firmware. L'opcode 0x35 a servi à la place.
5. **Journaux MSC.** Les messages de la crate ne sortent sur la console que si le
   firmware autorise la cible `iobewi_usb_msc` ; la qualification le fait.
6. **Limites de la campagne.** Un seul hôte (Windows), un seul exemplaire de carte, une
   seule lecture de 10 minutes, pas de capture de bus USB (analyseur absent) : les preuves
   de protocole viennent de l'UART de la carte et des réponses vues par l'hôte.

## Corrections

Aucune correction des briques. Firmware de qualification : activation des journaux MSC,
correction de la valeur `written` affichée par `RESUME`, ajout de l'affichage du code
d'erreur Windows et des cas d'erreur SCSI sans phase de données dans `scsi.py`.

## Verdict

**#66 : qualifiée matériellement sur ESP32-S3 pour la chaîne producteur → rolling stream
→ FAT16 → MSC → OTG S3, avec un hôte Windows.** Énumération, identité, capacité, lecture
seule, FAT16, continuité sur 10 minutes et ~847 bouclages, sous-alimentation et reprise,
erreurs SCSI historiques, trois déconnexions/reconnexions, démarrage à froid et
redémarrage logiciel passent, sans corruption. Les deux limites BOT et la détection de la
déconnexion décrites ci-dessus sont des propriétés connues, pas des régressions.

Non qualifié par cette campagne : reset PHY après reset, autres hôtes, lecteur Metronic,
autre famille de MCU.

## Reste pour #67

Raccordement StreamBeWI : VID/PID produit, identité radio, préchargement, délai 5 s /
sondage 5 ms, URL et reconnexion HTTP, verrou partagé, provisioning, bascule JTAG,
lecture audio sur le Metronic. Il faudra y rejouer la lecture continue et une
déconnexion avec le vrai producteur réseau et le lecteur cible.
