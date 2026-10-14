# Issue #68 — ConfigSpace sur EKV : intégration ESP32

## Incrément de raccordement — 14 octobre 2026

- `common/config/space::storage::Store<F>` utilise EKV directement, manifest
  complet, admission avant I/O, générations persistantes, tombstones, writer
  logique couvrant lecture de génération/commit/readback, self-check en lecture
  seule. Nouveau framing CSP2, aucune importation CSM1/NVS.
- `EspConfigFlash` découvre data/0x40 et fournit les callbacks bornés/alignés
  sur le SharedFlash existant. Pas de nouveau propriétaire ni mutex physique.
- ota-system compose un Store statique avec ota2048 ; config-manager compose
  demo32 et propose le mode `exercise` commit/clear/recommit avec logs/assertions.
- Crates common/fs/nvs, arch/esp32/fs/nvs, dépendance esp-nvs et codec CSM1 retirés.
  Les erreurs HTTP TLS de persistance se nomment storage_write_failed.
- OTA A/B conserve ses offsets d'images/otadata. Partition config nouvelle :
  0x3c0000, 0x40000 (256 Kio), sur une flash de 4 Mio minimum. Le scénario B
  garde ses trois régions supplémentaires jusqu'à 0x3b0000. La table standalone
  config-manager utilise config0x9000/0x40000 et factory0x50000/0x180000.
- `prepare.py` produit config.bin vierge pour installation/reset explicite
  seulement ; il n'est jamais envoyé par OTA. Première région entièrement
  vierge initialisée ; toute région nonblank qui échoue au mount reste intacte.
- Le seed est fourni par le RNG HAL, pseudo-aléatoire avant activation RF.
  Ce n'est pas une promesse de seed cryptographique ni une endurance garantie.

La limite de reprise après coupure reste acceptée pour le développement et
suivie dans #70 ; seul son test connu en échec reste ignored. Les autres tests
et les liens MCU restent obligatoires. Les compilations ne remplacent pas les
tests carte. Les résultats de ce raccordement sont consignés ci-dessous ; les sections
archivées décrivent les incréments antérieurs.

---


## Validation locale du raccordement — 14 octobre 2026

ConfigSpace : 8 tests existants PASS ; banc : 10 PASS/1 ignored (#70), dont
backend réel, quotas, tombstones, concurrence, overflow et région minimale.
Adaptateur : 2 tests de bornes/géométrie PASS. TLS HTTP : 4 PASS. Service OTA
runtime+config-space : 70 unitaires et 2 intégration PASS. Clippy de ces
combinaisons all-targets -D warnings, formatage et découverte CI PASS.

Liens release config-manager normal/exercise et OTA-system nominal : C3
nightly-2026-10-13 (e9bbadf68), S3 esp1.98.1.0 PASS. S3 garde le warning RWX
amont. La tentative locale C3 avec nightly du 14 octobre (822182db4) a échoué
sur des métadonnées core corrompues ; la validation locale est datée, tandis
que la CI conserve nightly et doit vérifier la compatibilité actuelle.

Les trois CSV passent le générateur officiel ESP-IDF v5.5.1. Préparation S3
réelle exécutée : images DIO, bootloader S3 29 744 octets (marge 3 024 avant
0x8000), C3 22 800 octets (marge 9 968) ; applications C3/S3 acceptées aux
deux offsets, otadata via le codec commun,
config.bin vierge 262 144 octets. Aucun flash matériel. Les builds locaux sans
credentials servent à la validation logicielle ; reconstruire les images OTA
avec l'identité réseau/TLS de la carte pour la campagne.

Usure du backend réel CSP2/readback, 800 OTA de 2 Kio et TLS/Wi-Fi/log froids :
3 732 432 octets programmés /1 638 400 utiles, amplification 2,278096 ;
1 658 effacements, 64/64 secteurs utilisés, min18/max29. Pics :12 effacements,
28 556 octets programmés par commit. Génération801 et valeurs conservées au
reboot. Formatage/initialisation exclus ; ce n'est pas une durée de vie MCU.

[Procédure de campagne carte](issue-68-board-campaign.md) : config-manager
normal/exercise, reboots nominalement après opérations terminées, refus de
région non reconnue puis OTA A complète avec deux images EKV. Les coupures de
commits restent dans #70. Aucun test physique déclaré PASS. Les résultats CI
de cet incrément sont à vérifier après publication.

---

## Décision du mainteneur

EKV est le socle retenu pour la prochaine persistance ConfigSpace, indépendante
du MCU. Le projet est en développement : **aucune migration NVS, aucune
compatibilité CSM1, aucun double moteur dans la cible**. Cette décision remplace
les propositions de migration et la comparaison ouverte du premier incrément
bb8cbc833fd8ed87af19e408722362408abb2fee de la PR #69.

La cible conserve le manager et les handles ConfigSpace, les quotas, les valeurs
opaques, les générations et les tombstones. EKV fournit les transactions, le
recyclage et la rotation des pages ; les adaptateurs matériels fournissent son
trait `ekv::flash::Flash`. ESP32 doit réutiliser `SharedFlash`, son propriétaire
physique unique et son verrou, sans nouvelle instance de FlashStorage.

**Décision du 14 octobre 2026 : poursuivre le raccordement en développement
avec le défaut connu ; reprise après coupure non qualifiée, suivie dans
[#70](https://github.com/iobewi/rust-mcu-libs/issues/70).**
Le nouvel incrément fournit un banc hôte utilisant directement EKV. EKV est une
`dev-dependency`, aucun backend EKV n'est déployé et aucune table de partitions
n'est modifiée. Le code NVS déjà publié reste exécuté jusqu'au remplacement
effectif ; il ne constitue pas une couche de compatibilité de la cible.
Aucune fusion/clôture de #65, #68 ou #69.

## Consommateurs et responsabilités

| Élément | Responsabilité dans la cible |
| --- | --- |
| common/config/space | Propriété/quotas, générations persistantes et tombstones, contrat load/commit/clear |
| EKV | Persistance transactionnelle, valeurs multi-pages, intégrité CRC, recyclage/rotation |
| arch/esp32/config/space | Adaptateur Flash borné et découverte d'une partition dédiée |
| arch/esp32/flash | Propriétaire et verrou physiques existants, partagés avec OTA |
| examples | Manifest des espaces, géométrie/partition, seed aléatoire par boot et initialisation explicite |

Budgets observés : Wi-Fi 128 octets, TLS 6 154, logs 1 024, OTA 2 048,
demo config-manager 32. Seuls ota-system et config-manager composent le backend
NVS ; aucun autre consommateur direct des deux crates fs/nvs n'a été trouvé.
Les tables OTA actuelles réservent 16 Kio à NVS. Les **256 Kio du banc EKV ne
sont ni une nouvelle table publiée ni une preuve d'admission des quotas**.

## Profil testé et reproductibilité

- Version publiée `ekv = "=1.0.0"`, tag
  [v1.0.0 / e9b3ff3](https://github.com/embassy-rs/ekv/tree/e9b3ff36d29ada21dba4f440a214305b7d965abc).
- Features : `crc`, `max-value-size-8192`, `max-chunk-size-256`.
- Profil vérifié par assertion de compilation : pages 4 096 octets, alignement
  4, maximum 256 pages, clé 64, valeur 8 192, chunk 256, effacement 0xff,
  branching factor 2, scratch pages 4. Version/configuration font partie du
  format persistant ; toute modification exige une nouvelle qualification.
- Banc : 64 pages, soit 256 Kio, Linux x86_64, Rust stable
  `1.99.0 (b940084d7 2026-09-28)`, profil debug.
- Appels directs à Database, ReadTransaction et WriteTransaction ; **aucun
  framing CSM1/CSP2, manager ConfigSpace ou pilote ESP dans le reproducteur**.

```sh
cargo run -p iobewi-config-space --example qualify_ekv -- wear
cargo run -p iobewi-config-space --example qualify_ekv -- cuts
cargo run -p iobewi-config-space --example qualify_ekv -- reproduce
cargo test -p iobewi-config-space
cargo clippy -p iobewi-config-space --all-targets -- -D warnings
```

Le simulateur conserve les octets et l'historique de programmation à travers
les réouvertures. Il contrôle bornes, alignements et programmation unique par
octet entre effacements, y compris les écritures de 0xff. À la coupure, seul un
préfixe de la zone ciblée est programmé/effacé ; **toutes** les opérations
suivantes échouent jusqu'à restauration de l'alimentation. Les autres octets
restent intacts. Cela respecte le contrat Flash d'EKV. Le banc ne reproduit
pas toutes les configurations de bits d'une coupure matérielle.

## Usure mesurée

Charges froides : TLS 6 154 octets, Wi-Fi 128, logs 1 024 ; puis 800 remplacements
OTA de 2 048 octets. La dernière OTA et les trois valeurs froides restent
lisibles après remontage. Formatage et initialisation des valeurs froides sont
exclus des compteurs ci-dessous ; recyclage et transactions du workload inclus.

| Mesure | Résultat |
| --- | ---: |
| Octets utiles | 1 638 400 |
| Octets programmés | 3 706 464 |
| Amplification de programmation | 2,262246 |
| Effacements totaux | 1 658 |
| Secteurs effectivement effacés | 64 / 64 |
| Effacements par secteur, min / max | 18 / 29 |
| Effacements moyens par mise à jour | 2,0725 |
| Pic d'effacements sur un commit | 12 |
| Pic d'octets programmés sur un commit | 28 356 |
| Pic d'opérations write/erase sur un commit | 775 |

Histogramme par numéro de secteur :

```text
[21,21,21,26,23,25,26,27,28,28,26,25,27,28,29,29,
 25,25,25,26,24,22,24,26,27,27,28,28,29,29,28,25,
 26,28,29,29,29,29,29,26,26,28,25,25,25,25,25,18,
 19,19,24,26,26,27,28,27,27,28,27,27,28,28,24,23]
```

100 boots avec mount/lecture TLS seuls : **0 programmation et 0 effacement**.
Ce résultat concerne EKV direct, pas encore le self-check de l'application.
La cible doit supprimer le canary écrit à chaque boot. Aucun gain comparatif
NVS, temps MCU, RAM MCU ou durée de vie matérielle n'est revendiqué. Le taux
réel de commits et l'endurance de la puce restent nécessaires pour estimer
une durée de vie. Les coûts d'usure après recovery restent à qualifier.

## Coupures : gate échouée

1 972 cas, avec préfixes partiels et complets sur chaque opération d'un
remplacement ou d'un delete EKV, avant et après 200 mises à jour recyclées.
Le delete teste le moteur ; la future suppression ConfigSpace devra utiliser
un tombstone transactionnel pour préserver la génération.

| État initial | Action | Frontières write/erase |
| --- | --- | ---: |
| 4 clés initialisées | Remplacement | 62 |
| 4 clés initialisées | Delete | 44 |
| Après 200 mises à jour | Remplacement | 84 |
| Après 200 mises à jour | Delete | 66 |

**32 cas FAIL**, lors du remontage : `Corrupted`. Une valeur précédemment
confirmée devient inaccessible. Les cas qui montent sont vérifiés pour old/new
atomique, conservation des valeurs froides et possibilité de recommit.
Ces préfixes sont un échantillonnage déterministe, pas une exploration exhaustive
de toutes les coupures. Aucun recovery général n'est déclaré PASS.

Reproducteur minimal : région vierge, format(seed=1), commit `ota=previous`,
réouverture(seed=4), remplacement par 2 048 octets `0x09`, coupure à l'opération
25 après 2 octets, abandon de Database, réouverture(seed=77).

```text
torn operation=Operation { erase: false, page: 19, offset: 36, len: 8 }
prefix=2 write_result=Err("commit: Flash(PowerOff)")
torn header bytes=[c5, 59, ff, ff, ff, ff, ff, ff]
FAIL: reboot: Corrupted; previously acknowledged value is inaccessible
```

Analyse de la source 1.0.0 : PageWriter::commit écrit le ChunkHeader (magic,
longueur, CRC) en une opération. ChunkIter::open_chunk reconnaît le magic après
2 octets, puis refuse la longueur 0xffff. FileManager::mount propage cette erreur
en parcourant les métadonnées. Le commit interrompu n'a pas réussi, mais il
empêche l'accès au commit antérieur. Aucun formatage automatique n'est tenté.

Le **même reproducteur direct** a été exécuté localement sur main amont
[a0e2c7e16cf790e2fa7e34fac9f9dac1557824aa](https://github.com/embassy-rs/ekv/tree/a0e2c7e16cf790e2fa7e34fac9f9dac1557824aa)
avec embassy-sync 0.8 : même sortie et exit 1. Cette substitution locale n'est
pas publiée ; la campagne complète et l'usure sur main ne sont pas revendiquées.

## Décision de CI — 14 octobre 2026

Le mainteneur demande de poursuivre le développement avec ce défaut connu,
sans engager maintenant un fork/correctif EKV. Suivi dédié :
[#70](https://github.com/iobewi/rust-mcu-libs/issues/70), lié aux signalements
amont [#26](https://github.com/embassy-rs/ekv/issues/26) et
[#25](https://github.com/embassy-rs/ekv/pull/25). La PR amont #25 fournit un
reproducteur sans correctif et signale aussi la publication trop précoce d'une
nouvelle page de métadonnées, pouvant produire KeyNotFound.

`[[example]] test=true` conserve les tests nominaux/usure/boots dans la CI.
**Seul `power_fail_qualification` est marqué ignore**, avec la référence #70.
Il n'est ni supprimé ni transformé en succès attendu. Commande manuelle :

```sh
cargo test -p iobewi-config-space --example qualify_ekv power_fail_qualification -- --ignored --nocapture
```

Les CLI `cuts` et `reproduce` restent inchangées et retournent un échec.
Une CI verte avec ce test ignoré valide les contrôles actifs ; elle ne corrige
pas le défaut et ne qualifie pas la reprise après coupure. Le développement et
l'intégration ESP peuvent continuer selon cette décision. La PR reste Draft
pour le chantier #68 encore incomplet.

## Résultats de l'incrément 892fc6c et suite

- ConfigSpace existant : 10 tests PASS.
- Banc : valeurs froides/usure PASS ; 100 boots en lecture seule PASS ;
  coupures FAIL (32/1 972). `cargo test` retourne 101.
- Reproducteur minimal : exit 1, version publiée et main.
- Clippy `--all-targets -D warnings` PASS ; rustfmt et diff check PASS.
- Aucun nouvel adaptateur ESP, lien du backend EKV ou test carte exécuté.
  La CI verte du commit préparatoire bb8cbc8 ne valide pas cet incrément.

Le défaut amont et la réactivation de sa qualification sont désormais suivis
dans #70. Ne pas masquer les corruptions des records déjà validés ; ne pas
reformater automatiquement en cas de mount échoué.

Prochain travail #68 : figer admission/réserves et format neuf, backend commun avec
générations/tombstones, adaptateur ESP SharedFlash, tables/exemples, suppression
des crates NVS et du codec CSM1, liens C3/S3, qualification matérielle OTA.
Les ports RP2350/Teensy restent futurs ; leurs géométries doivent respecter le
profil Flash choisi, sans read-erase-modify-write caché.

## Vérification locale de la suspension — 14 octobre 2026

Rust 1.99.0 : `cargo test -p iobewi-config-space` PASS (10 tests existants,
2 tests du banc PASS, 1 seul ignored avec référence #70). Rustfmt check,
Clippy all-targets -D warnings et diff check PASS. Exécution explicite de
`power_fail_qualification -- --ignored --nocapture` : FAIL, exit 101,
32/1 972 cas en échec. Le reproducer est conservé sans changement de logique.
La CI distante de cette suspension reste à vérifier après publication.
