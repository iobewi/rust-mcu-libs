# Campagne carte — OTA système A + composant supervisé B (#73, #61)

Bilan du 14 octobre 2026, sur une seule carte ESP32-S3. Il suit la section
« Campagne matérielle restante (#61) » du README `examples/firmware/ota-supervised`.

## Périmètre et conditions

- **Commit testé :** `906648781dbade1ac4360bf01d7bcbb94e37c47c` (`feat/61-ota-supervised`, PR #73), sans modification locale. La CI de ce commit est verte (42 jobs sur 42).
- **Carte :** ESP32-S3 DevKitC-1 N16R8, QFN56 rév. v0.2, flash ID 0x184068 (16 Mio), MAC 9c:13:9e:ac:4e:8c ; images à en-tête DIO 4MB.
- **Toolchain :** esp 1.98.1.0 (rustc 1.98.1-nightly), esptool 5.5.0, `gen_esp32part.py` ESP-IDF v5.5.1.
- **Banc :** Web Serial. Une page tient le port UART et bascule entre console et chargeur esptool-js 0.7.0. Écritures et relectures sont pilotées par commande. Un reset EN automatique se déclenche sur échec du premier join Wi-Fi (#72).
- **Clients :**
  - `examples/firmware/ota-system/client.py` pour les OTA A et B nominales ;
  - un client d'essai pour les requêtes B refusées et les transferts tenus ou coupés.

  TLS est toujours vérifié par la CA d'essai.
- **Secrets :** identifiants Wi-Fi, Bearer et clé TLS sont intégrés au build et ne sont joints nulle part ; le SSID est masqué dans les traces.
- **Hors périmètre :** #70 (coupures EKV) et #72 (Wi-Fi) restent différées. C3 n'a pas de chargeur natif.

### Construction

Les variables Wi-Fi/TLS/Bearer du banc sont chargées comme pour `ota-system`, puis :

```sh
python3 examples/firmware/ota-system/prepare.py --chip esp32s3 --scenario ota-supervised \
  --partition-tool /chemin/esp-idf/components/partition_table/gen_esp32part.py
python3 examples/firmware/ota-supervised/build-component.py --variant a
python3 examples/firmware/ota-supervised/build-component.py --variant b
python3 examples/firmware/ota-supervised/build-component.py --variant a --fault panic        # idem ignore-stop, log-flood
# Garde anti-boucle : build-component.py ne propose pas reset-chip ; même commande, feature ajoutée.
cargo +esp build -Z build-std=core,alloc --target xtensa-esp32s3-none-elf -p example-supervised-component \
  --features esp32s3,fault-reset-chip --release --bins
cargo run -p iobewi-workload-pack -- target/xtensa-esp32s3-none-elf/release/supervised-component \
  -o target/ota-supervised/esp32s3/native-a-reset-chip.iwni
```

| Image | SHA256 |
| --- | --- |
| bootloader | `8719f258fcf11a0f97fd83dff98f47d1cd80c93b315ad2b47b382b4384557ce7` |
| table de partitions | `264c0df1dec49d2200fe30d6f55a61c87d80bd674eb522fde93f1a58e3edf220` |
| otadata usine | `d3137c5ca187cb7a334d791ba830663cbef8ffaa9d387ce5e1cb63e1c9ac865d` |
| app A+B (`app.bin`, 1 134 128 o) | `dffe2d271d52cd1543e60d6ba3cbc4f71e7811d75fe934eb9ac79acf4f4e7bcd` |
| `wl_meta.bin` vierge | `7d2c7ac4888bfd75cd5f56e8d61f69595121183afc81556c876732fd3782c62f` |
| `config.bin` vierge | `3b874d3ba46c638fc3094f8e92fb744ca974893873f8885f54e23760f9b6311b` |
| `native-a.iwni` (4 108 o) | `41150ac5a9708296a7a6178da08c388f280765bd36c931a066733981734d7b2e` |
| `native-b.iwni` | `20800fcb5f752477a74f6143ac102c5459dab92c07ab05daf26e4318ed4ead8a` |
| `native-a-panic.iwni` | `721a9cf32a674d478e7399833491759702a0685622b081ab00ccf656fa524bb7` |
| `native-a-ignore-stop.iwni` | `3f1b618022315ad8e5ba47380e681ae1024469acc53ec612fca053370ab691d3` |
| `native-a-log-flood.iwni` | `800beaf282b1308f4287c65109d77bcba6c590b5dd47d2c9b70a485921a6bba2` |
| `native-a-reset-chip.iwni` | `74d882ddd64a2bdcfbcad5776aa5df9fe2efd6a84837de7549c10ae0f6b858fb` |

### Installation

1. **Sauvegarde préalable :** relecture des 4 premiers Mio de flash, SHA256 `f4e45e8af6dbb44a5614a0a1a7d3b2cb5c9e2fd11ee8d4d4f65b301821666b90`.
2. **Installation usine, une seule fois :** effacement complet, puis bootloader @0x0, table @0x8000, otadata @0xd000, app @0x20000, `wl_meta` @0x320000, config @0x3c0000. Le SHA256 de chaque partie est vérifié avant l'écriture.
3. **Ensuite, seulement des OTA :** les fichiers usine n'ont plus été réécrits. Les seules écritures directes ont servi aux essais de B invalide ; elles sont décrites plus bas et ont été restaurées.

## Résultats

| # | Essai | Résultat | Trace |
| --- | --- | --- | --- |
| 1 | Premier boot sans B | **PASS** | `ota_0 Stable`, `B: real stop NothingRunning`, `OTA ready`, `/health` 200 ; `/workload/status` → `metadata=Ok(None); running=None` (401 sans jeton) ; `wl_meta` relu identique à l'image vierge |
| 1b | A/HTTPS avec B invalide : image B corrompue (un secteur du slot actif écrasé) | **PASS** | `B: recovery failed; system A remains available` ; sélection `Valid` conservée telle quelle, rien en exécution, `/health` 200 ; restauration du slot depuis sa relecture → B relancé |
| 1c | A/HTTPS avec métadonnées B non reconnues (les deux copies de `wl_meta` écrasées) | **PASS** | `recovery failed`, `/workload/status` → `Err`, envoi B refusé (409), `/health` 200 ; `wl_meta` relu identique aux octets écrits : ni réécriture, ni formatage ; restauration depuis la relecture → B relancé |
| 2 | Envoi B seul, puis reboot | **PASS** | `staged` ; après reset EN : `Staged`, candidat dans le slot A, `running=None`, aucun log `B[...]` |
| 3 | Activation explicite, A↔B dans les deux sens | **PASS** | `hello-a` : `pending` 0,7 s, `B[3]: native-workload-a`, progression `alive, elapsed_ms=4750`, `B: confirmed` après 5 s, **aucun reboot système** ; `hello-b` dans le slot B puis `hello-a2` dans le slot A : arrêt `Cooperative` du précédent, confirmation, `/health` 200 |
| 4 | Envoi pendant l'exécution, refus, transfert interrompu | **PASS** | identité courante inchangée pendant tous les envois. Refus avant toute écriture (409/401, `Staged` existant conservé) : ID invalide ou trop long, version absente, API mal formée ou absente, digest absent, transfert par blocs, jeton faux ou absent, activation d'un autre ID. Refus après remplacement (API 2.0, mauvais digest) : `Staged` précédent retiré, retour à `Valid`. Transfert coupé à 2 048/4 108 o : pas de publication, ancien staged retiré, envoi suivant `staged` |
| 5a | Candidat panic | **PASS** | panic environ 1,8 s après le démarrage → `real stop AlreadyHalted` → précédent relancé, `Valid` ; aucun reboot système, `/health` 200 |
| 5b | ignore-stop | **PASS** | remplacement : `real stop Forced`, activation en 2,3 s (au lieu de 0,8) ; `POST /workload/stop` : `Forced` en 2,2 s, `running=None`, sélection conservée |
| 5c | log-flood | **PASS** | vidage borné à 20 lignes/s (4 lignes par cycle de 200 ms ; 127 lignes en 7,6 s) ; `/health` 200 en 0,7 s pendant le flot ; candidat confirmé, puis arrêt forcé au remplacement |
| 5d | Garde anti-boucle (`reset-chip`) | **PASS** | confirmé, puis `RTC_SW_SYS_RST` environ 6 s après chaque démarrage ; 3 démarrages de B, puis `B: auto-start suppressed after repeated unclean boots` au 4ᵉ boot ; A disponible ; nouvelle activation (`hello-b8`) confirmée ensuite |
| 6a | Reset pendant un envoi tenu (2 048/4 108 o), après remplacement d'un `Staged` | **PASS** | après boot : ancien staged retiré, `Valid` précédent relancé |
| 6b | Reset juste après l'intention `Activating` (ligne `real stop`) | **PASS** | après boot : rollback, précédent `Valid` relancé |
| 6c | Reset 2 s après `PendingConfirmation` | **PASS** | après boot : rollback vers le précédent |
| 6d | Reset pendant un rollback (`real stop AlreadyHalted` d'un candidat panic) | **PASS** | rollback repris au boot, précédent `Valid` |
| 6e | Balayage de délai autour de la confirmation et de la fin d'activation | **partiel** | 9 essais valides (confirmation 4,70 à 4,95 s, activation 0 à 250 ms), tous revenus au `Valid` précédent. Les autres essais sont invalides à cause d'un défaut de banc, sans lien avec la carte. Balayage arrêté à la demande du mainteneur (pas d'écritures répétées) |
| 6f | Coupures pendant l'effacement ou la programmation de `wl_meta` | **non exécuté** | non visé avec une précision suffisante, et pas d'essais répétés d'usure, sur décision du mainteneur |
| 7 | Images A, otadata, config inchangées par les OTA B | **PASS** | relectures avant et après plus de 30 envois B, 13 activations et 5 reboots de crash : ota_0 identique à l'`app.bin` installée, otadata = usine (`d3137c5c…`), config = un en-tête de page EKV de 20 o, et tout identique entre avant et après ; seul `wl_meta` change |
| 7b | OTA A avec B présent, dans les deux sens | **PASS** | `a73-1` → ota_1 (1 134 128 o écrits en 22,7 s pendant que B tourne), `PendingVerify` → `confirmed` en 1 s, B relancé ; puis l'image instrumentée → ota_0, et retour à l'image nominale → ota_1, tous confirmés |
| 8 | Mesures mémoire et temps du nouveau binaire | **mesuré** (instrumenté) | voir ci-dessous |

**Reboots non provoqués par l'essai :** le premier join Wi-Fi a échoué 62 fois (`DisassociatedDueToInactivity`, #72, non investigué). Ces échecs ont été levés par un reset EN automatique du banc, 29 fois, ou par un reset manuel. Ils n'ont modifié aucun état B.

## Mesures (code instrumenté, qualification seulement)

**Méthode :** la sonde de #71 (pile principale cpu0 peinte au démarrage, statistiques esp-alloc `internal-heap-stats`) est appliquée sur `9066487` par un patch hors dépôt. Ce patch ajoute aussi des horodatages `now_ms` dans `launch` et `activate`. L'app instrumentée (`b2a6c202edf9ab8c49b3e721dbc7b13f40fc537bb818d5ef22e68a4e33956628`) est installée par OTA A, puis la carte est revenue à l'image nominale. Les valeurs sont des maxima cumulés depuis le boot.

| Point | Tas courant | Tas max / min libre | Pile max / marge |
| --- | --- | --- | --- |
| après montage EKV | 0 | 8 192 / 155 648 | 26 800 / 31 608 |
| `boot: stable` ou `confirmed`, puis relance de B | 104–112 | 8 296 / 155 544 | 27 808–28 032 / 30 376 |
| `OTA ready` | 47 884 | 49 608 / 114 232 | 27 808 / 30 600 |
| envoi B (`staged`) | 84 476 | 94 700 / 69 140 | **47 280 / 11 128** |
| activation B (`PendingConfirmation`) | 75 932–79 340 | 94 700 / 69 140 | 47 280 / 11 128 |
| B confirmé | 49 404–51 108 | 94 700 / 69 140 | 47 280 / 11 128 |
| envoi A vérifié / activation A | 84 508 | 97 812 / **66 028** | 47 280 / 11 128 |

- **Pile principale :** il ne reste que **58 408 o réservés** (contre 73 912 pour A seul en #71), car la région native de 32 Kio est prise sur la RAM. L'envoi B la fait monter à 47 280 o, ce qui laisse **11 128 o de marge (19 %)**, contre 52 % pour A seul. Aucun débordement observé, mais c'est la marge la plus faible de la campagne.
- **Tas :** au moins 66 028 o libres (40 %) pendant un envoi A avec B en exécution.
- **Temps :**
  - vérification du digest d'une image de 4 108 o : 7 ms ;
  - chargement et démarrage : 7–8 ms ;
  - activation complète (vérification, préliminaires, intention, arrêt, chargement, démarrage, fin d'activation) : 102–106 ms ;
  - arrêt forcé (ignore-stop) : environ 2,2 s.
- **Impact sur le réseau et la flash :** OTA A de 1,13 Mo en 22,7 à 26 s pendant que B tourne, soit environ 45–50 Ko/s (#68 sans B : 53,6 Ko/s). `/health` répond en 0,6–0,7 s, avec ou sans B, y compris sous log-flood.
- **Non mesuré :** la pile propre de B, soit 8 Kio sur APP_CPU dans la région native, et les piles des tâches radio, allouées dans le tas.
- **Coût de l'instrumentation :** l'app instrumentée fait 1 072 o de plus.

## Constats

1. **Marge de pile principale à 19 %** pendant l'envoi B, avec 58 408 o réservés. Ce n'est pas un défaut observé, mais elle est nettement plus faible que pour A seul. À suivre (#61) avant d'ajouter du code sur ce chemin.
2. **Diagnostic trompeur :** quand rien ne tourne, `B: real stop` réaffiche le dernier motif d'arrêt (`Forced`) au lieu de `NothingRunning`. Ce n'est qu'un message, sans effet fonctionnel.
3. **`build-component.py` ne propose pas `reset-chip` ni `null-jump`**, alors que le README demande d'observer la garde anti-boucle. La variante a été construite avec la commande équivalente ci-dessus.

Aucune correction de code n'a été faite pendant cette campagne.

## Verdict

Validé sur ESP32-S3, en fonctionnement nominal et sous interruptions ciblées :
- A reste disponible sans B, avec B invalide et avec des métadonnées B non reconnues ;
- l'envoi, la conservation au reboot, l'activation sans reboot système et la confirmation fonctionnent ;
- le remplacement marche dans les deux sens, ainsi que les refus et le transfert interrompu ;
- les comportements panic, ignore-stop, log-flood et la garde anti-boucle sont conformes ;
- les reboots entre les intentions OTM2 et leurs effets aboutissent à un état explicite ;
- les OTA B laissent A, otadata et config intacts ;
- l'OTA A fonctionne dans les deux sens avec B présent.

**Non validés :** les coupures en plein effacement ou programmation de `wl_meta` (non exécutées), la pile de B, C3, #70 et #72.

Traces, relectures, images, patchs et sorties client sont archivés hors dépôt ; le SSID y est masqué.
