# Échec de P3 : ce qui est établi, ce qui ne l'est pas, et la build de diagnostic (#91)

Le 30 octobre 2026, le second étage vérifié (`verified-iram36-uart0.bin`, SHA-256 `ddeccf27…1bf1a`) a affiché
`boot authentication: VERIFIED` puis `boot: PANIC` et s'est arrêté (`verified-boot-qualification.md`, « Déroulé
réel »). Ce document reprend l'analyse sur l'ELF exact de cette image, sans rien écrire sur une carte. **La cause
n'est pas établie.**

## Démontré

1. **Le dernier point atteint.** Le message `VERIFIED` est le premier de `verified_plan`, une fois la politique
   construite ; `read_otadata` l'a précédé (sinon `boot: FAILED flash read` aurait été affiché). Le message suivant,
   `boot: slots checked, microseconds N`, n'a jamais paru. Entre les deux s'exécutent : la première lecture de
   l'horloge (`micros()`, `esp_hal::time::Instant::now`), puis `select::plan` : la structure du slot 0 (le même code
   que le profil non signé, déjà qualifié), et l'admission de la preuve (lecture du secteur de preuve, SHA-256 et
   RSA-PSS de la ROM, SHA-256 de l'image par la ROM). L'horloge et l'admission sont **ce que le profil non signé n'exécute pas**.
2. **`boot: PANIC` n'est pas une preuve de panique Rust.** Dans l'ELF, `__default_user_exception`,
   `__default_double_exception`, `__default_interrupt` et `EspDefaultHandler` appellent tous
   `core::panicking::panic_fmt` (désassemblage, et source `xtensa-lx-rt` 0.23 : `panic!("Exception: {:?}, {:08x?}",
   cause, frame)`). Une **exception CPU**, une **exception double** ou une **interruption sans gestionnaire** arrivent
   donc dans le même handler et produisent la même ligne. Ce message ne dit pas laquelle.
3. **L'emplacement et le message ne sont pas dans l'image.** Le handler ignore `PanicInfo` : la compilation
   (LTO) retire les `Location`, et l'image P3 ne contient aucun chemin de fichier. Rien ne se lit après coup.
4. **Aucun site de panique ne survit dans le code d'admission.** Les fichiers qui gardent des sites de panique
   (carte lue sur la build de diagnostic, dont les chemins sont conservés ; les sources sont les mêmes que P3) :
   `bootauth/src/select.rs`, `auth/src/manifest.rs`, `boot/src/hw.rs`, `image/src/esp.rs`, `boot/src/lib.rs`,
   `layout/src/lib.rs`, `partitions/src/raw.rs`, `bootloader/src/main.rs`, et côté dépendances `esp-hal`
   (`time.rs`, `timer/systimer.rs`, `clock`, `interrupt`, `gpio`, …), `esp-rom-sys`, `esp-sync`, `xtensa-lx-rt`
   (`exception/context.rs`). **Absents** : `bootauth/src/policy.rs` (l'admission), `bootauth/src/iap1.rs`,
   `esp32/firmware/boot/src/rom_verify.rs` (l'adaptateur SHA et RSA de la ROM), `auth/src/{verify,policy,profile}.rs`.
   Les indices de l'admission (`payload[..1236]`, `[76..460]`, `[460..1236]`) sont constants, et les longueurs
   passées à l'adaptateur ROM (776 et 384) sont vérifiées avant l'appel.
5. **Les sites qui restent dans la fenêtre sont bornés par construction** : l'indexation de `layout.images[slot]` et
   `slots[slot]` avec `slot` dans {0, 1} (`SLOT_COUNT = 2`), et le formatage hexadécimal des nombres du journal
   (`out[2 + i]`, `i < 8`, dans un tableau de 10). `key_shape_ok` ne s'exécute qu'après avoir vérifié que la clé fait
   776 octets ; `hw::flash_read` lit au plus 252 octets dans une fenêtre de 256.
6. **Overflow et panique.** Le profil `bootloader` est `panic = "abort"` et `overflow-checks = false` : un
   débordement arithmétique ne panique pas ; une division par zéro ou un indice hors bornes, si.

Limite de ces preuves : elles tiennent à une lecture du code et à une carte de sites lue sur une build différente de
P3 par l'instrumentation (mêmes sources, inlining voisin, non identique). Ce sont des preuves **par inspection**, pas des mesures.

## Hypothèses, des plus au moins vraisemblables d'après ce qui précède

Rien ne désigne RSA-PSS plus qu'autre chose : la fenêtre contient au moins quatre éléments jamais exécutés sur matériel en
contexte bootloader (l'horloge, la lecture de la preuve, le SHA-256 par la ROM, le RSA-PSS par la ROM).

| Hypothèse | Pour | Contre ou inconnu |
|---|---|---|
| **H1.** Exception CPU ou interruption sans gestionnaire, dans un appel ROM (SHA, RSA) ou dans la lecture de l'horloge | seule classe que l'inspection n'écarte pas ; le handler la transforme en `PANIC` ; les ROM n'ont été exercées qu'en contexte application (sonde A3) | aucune mesure ; l'exception exacte est inconnue |
| **H2.** Débordement de pile (devient une exception) | la pile utilisée par la ROM n'est pas déterminable statiquement | 29 Kio disponibles, environ 3 Kio de tampons connus |
| **H3.** Panique Rust dans `esp-hal` (`time.rs`, `timer/systimer.rs`) autour de `Instant::now` | ces fichiers gardent des sites de panique | rien n'indique un chemin d'échec ; une horloge arrêtée **boucle** (`read_count` attend `value_valid` sans fin) plutôt qu'elle ne panique |
| **H4.** Panique Rust par indice dans `select.rs`, `main.rs` ou `manifest.rs` | sites présents | indices bornés par construction, voir 5 |
| **H5.** Autre : un état matériel que le contexte bootloader n'a pas (clocks, interruptions activées par la ROM) | différence de contexte avec la sonde A3 | pure conjecture |

Un **blocage** (boucle sans fin dans la ROM, horloge arrêtée) n'est pas non plus écarté pour la fin de la vérification, mais
il ne produit pas `PANIC` : il n'explique pas P3.

## Build de diagnostic `diag-panic`

Option du bootloader, **désactivée par défaut**, jamais dans l'image nominale :

```sh
cargo +esp build -Z build-std=core,alloc --target xtensa-esp32s3-none-elf --profile bootloader \
  -p iobewi-esp-bootloader --features esp32s3,verified-boot,diag-panic --bins
python3 .github/scripts/verified_boot.py --chip esp32s3 --diag     # construit et contrôle les limites
```

Ce qu'elle ajoute (console UART0 synchrone directe, aucun anneau ni asynchrone, politique d'arrêt inchangée) :

* après `boot: PANIC`, l'emplacement de la panique : `boot: at <fichier> line 0x… col 0x…` (nombres en hexadécimal). Une
  exception ou une interruption sans gestionnaire y nomme le fichier du **runtime** (`xtensa-lx-rt …/context.rs`,
  ou `esp-hal …/interrupt…`), pas le code fautif, ce qui suffit à distinguer la classe ;
* sur Xtensa, les gestionnaires d'exception utilisateur et double sont remplacés (`__user_exception`,
  `__double_exception`, mécanisme de `#[exception]`) et impriment, sans `core::fmt` :
  `boot: EXCEPTION cause=0x… pc=0x… ps=0x… a0=0x… sp=0x… vaddr=0x…` (ou `DOUBLE EXCEPTION`), puis s'arrêtent comme le
  handler de panique. `cause` est le `ExceptionCause` de `xtensa-lx-rt` : 0 `Illegal`, 2 `InstrError`, 3 `LoadStoreError`, 5 `Alloca`,
  6 `DivideByZero`, 8 `Privileged`, 9 `Unaligned`, 12 `InstrDataError`, 13 `LoadStoreDataError`, 14 `InstrAddrError`,
  15 `LoadStoreAddrError`, 16 `ItlbMiss`, 20 `InstrProhibited`, 24 `DtlbMiss`, 28 `LoadProhibited`, 29 `StoreProhibited`,
  32 à 39 coprocesseur désactivé (liste complète : `src/exception.rs` de la crate). `pc` se traduit par `xtensa-esp32s3-elf-addr2line -f -e <elf> <pc>` sur l'ELF de **cette**
  image ; `sp` sous la fin de `.bss` (`0x3fce…` : voir `esp_geometry.py`) signale un débordement de pile ; `vaddr` est
  l'adresse fautive.

Lecture :

| Dernier message | Signification |
|---|---|
| `boot: EXCEPTION cause=0x…` | exception CPU : la cause, le PC, la pile et l'adresse sont donnés |
| `boot: PANIC` puis `boot: at …/context.rs` | exception ou interruption sans gestionnaire passée par le runtime (cas non couvert par les remplacements : interruption) |
| `boot: PANIC` puis `boot: at …/bootloader/src/main.rs` ou un autre fichier du dépôt | panique Rust, avec sa ligne |
| `boot: PANIC` puis `boot: at …/esp-hal…` | panique dans `esp-hal` (H3) |
| rien après `VERIFIED` | blocage, ni panique ni exception |

### Coût

Octets, table à `0x10000` (S3) et `0x8000` (C3), console `uart0`. Le surcoût est essentiellement la donnée des noms
de fichiers (environ 2 Kio de chemins absolus de dépendances) et deux petits gestionnaires.

| Image | Nominale | Diagnostic | Écart |
|---|---|---|---|
| S3 vérifié : fichier | 34 816 | 38 400 | + 3 584 |
| S3 vérifié : IRAM / 36 864 | 31 059 | 31 771 (5 093 libres) | + 712 |
| S3 vérifié : DRAM statique / 32 768 | 3 748 | 6 616 | + 2 868 |
| S3 vérifié : région de pile | 29 020 | 26 152 | − 2 868 |
| S3 non signé : fichier | 29 984 | 33 296 | + 3 312 |
| C3 vérifié : fichier | 27 328 | 30 576 | + 3 248 |
| C3 vérifié : IRAM / 36 864 | 25 138 | 25 762 | + 624 |

Toutes tiennent dans la limite dure (S3 à `0x10000` : 61 440 ; C3 à `0x8000` : 32 768, marge 2 192 pour la vérifiée)
et dans la fenêtre de code de 36 Kio ; la disposition mémoire est contrôlée par `esp_geometry.py image` (la CI le fait
pour la build de diagnostic vérifiée). **Le non signé de diagnostic dépasse le budget de dérive du non signé nominal
(32 768) : la build de diagnostic n'est prévue que pour le profil vérifié.**

### Ce que la build de diagnostic ne change pas

* **L'image nominale est identique octet pour octet à celle de P3** avec ou sans les modifications de code : `ddeccf27…`
  (vérifié) et `38b162ac…` (non signé), reconstruits après l'ajout de la fonctionnalité.
* Aucun changement à VerifiedBoot, aux preuves IAP1, à la politique, aux décisions de sécurité, à la géométrie, au
  linker ni à la politique d'arrêt.

### Ce qu'elle ne garantit pas

* Elle n'a **jamais été exécutée sur une carte** : le remplacement des gestionnaires d'exception est vérifié par le
  désassemblage (le symbole `__user_exception` est celui du dépôt), pas par une exception réelle.
* Elle ne dit rien d'un blocage, ni du message de la panique (cela demanderait `core::fmt`, environ 10 Kio).
* Les interruptions sans gestionnaire ne sont pas décodées : elles se reconnaissent à l'emplacement (runtime), pas à leur niveau.
