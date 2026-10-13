# Issue #68 — ConfigSpace sur EKV : décision et qualification

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

**État : raccordement bloqué par la qualification de reprise après coupure.**
Le nouvel incrément fournit un banc hôte utilisant directement EKV. EKV est une
`dev-dependency`, aucun backend EKV n'est déployé et aucune table de partitions
n'est modifiée. Le code NVS déjà publié reste exécuté jusqu'au remplacement
qualifié ; il ne constitue pas une couche de compatibilité de la cible.
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

`[[example]] test=true` fait exécuter le banc par le `cargo test` habituel de la
CI hôte. La gate `power_fail_qualification` échoue réellement : **ni ignore,
ni transformation en succès attendu, ni suppression du contrôle**. La PR reste
Draft et non fusionnable tant que cette garantie ne passe pas.

## Résultats locaux et suite

- ConfigSpace existant : 10 tests PASS.
- Banc : valeurs froides/usure PASS ; 100 boots en lecture seule PASS ;
  coupures FAIL (32/1 972). `cargo test` retourne 101.
- Reproducteur minimal : exit 1, version publiée et main.
- Clippy `--all-targets -D warnings` PASS ; rustfmt et diff check PASS.
- Aucun nouvel adaptateur ESP, lien du backend EKV ou test carte exécuté.
  La CI verte du commit préparatoire bb8cbc8 ne valide pas cet incrément.

Prochain travail : corriger/qualifier le problème de publication interrompue
sans masquer les corruptions de records déjà validés. Rejouer également
coupures pendant effacement/recyclage et boots répétés après interruption.
Puis seulement : figer admission/réserves et format neuf, backend commun avec
générations/tombstones, adaptateur ESP SharedFlash, tables/exemples, suppression
des crates NVS et du codec CSM1, liens C3/S3, qualification matérielle OTA.
Les ports RP2350/Teensy restent futurs ; leurs géométries doivent respecter le
profil Flash choisi, sans read-erase-modify-write caché.
