# Étalon A + B natif — incrément logiciel

Référence réutilisée : `iobewi_old@345378ed1ae11a4814c32ee513f20438291cfba7`.

Le nouveau besoin est concret : compléter l'étalon système A par un composant
B réellement compilé, chargé, démarré/arrêté et observé. Aucun second moteur
OTA, propriétaire flash, protocole boot ou orchestration applicative globale.

| Référence | Destination / adaptation |
| --- | --- |
| workload/abi | common/firmware/component/abi : ABI v1, tests de layout conservés |
| workload/image | common/firmware/component/image : IWNI v1, tests conservés ; RuntimeApi réutilise OTM2 existant |
| workload/native | common/firmware/component/native : gate/load/start/stop/health/quarantaine et boot_guard ; contrats d'exécution extraits, sans importer l'ancien service OTA/flash |
| workload/sdk | common/firmware/component/sdk : SDK/entry/panic handler et linker IWNI inchangés |
| workload/esp32/native | firmware/supervisor/native : backend S3 sur APP_CPU ; constructeur unsafe explicite pour la réservation RAM/confiance des artefacts |
| workload/pack | tools/component-pack : packer ELF32/IWNI et gate existants |
| examples/workloads/hello | composant séparé dans ota-supervised/component, features nominales/fautes historiques |
| OTM2 | codec/store/politique déjà portés réutilisés ; nouveau NorMetadata emprunte le flash verrouillé, readback avant succès |

La composition B appartient à l'exemple ; ses routes et sa probation ne sont
pas imposées aux applications. La composition A est compilée depuis les mêmes
sources, avec les ajouts B conditionnels seulement dans le package sibling.
Le profil CI S3 natif lie les exécutables ; il ne simule pas une cible C3 native.

Différences explicites : constructeur du backend natif unsafe (invariants de
RAM et code de confiance) ; admission stricte et preflight obligatoire ; une
probation B de 5 s avant confirmation, plus longue que la fenêtre sans progrès
3 s ; arrêt réel en cas d'impossibilité de persister une intention de rollback.
Les octets IWNI/ABI/OTM2 ne changent pas. Tous les effets start/stop se font sans
verrou physique flash. La RAM n'est réutilisée qu'après halt.

Vérifications logicielles et campagne : voir le README ota-supervised. Les tests
hôte prouvent la politique/runtime sur modèle et le backend NOR sous injections,
pas le chargement ni les exceptions du deuxième cœur physique. #61 reste ouverte.

## Vérifications locales — 14 octobre 2026

- Rust stable 1.99.0 : 40 tests ciblés PASS (admission B 1, NOR OTM2 3,
  SDK 4, ABI 4, IWNI 12, runtime natif/garde 11, packer 5). Le backend NOR
  explore les 4 276 préfixes interrompus erase/program et les faux succès.
  Le doctest historique de macro SDK reste ignoré (pas un scénario matériel).
- Service OTA commun avec runtime/config-space : 70 tests + 2 fixtures PASS.
- Clippy all-targets -D warnings, rustfmt, diff check PASS.
- Profil/discovery : 50 crates, tous avec ci.json ; les deux packages natifs
  ont host/S3 et un vrai lien release, sans inventer un chemin natif C3.
- S3 esp 1.98.1.0, GCC 16.2.0, LLVM étendu 21.1.3 : A+B lié, région RAM
  réservée/assertions PASS. A seul également lié sur S3.
- A seul lié sur C3 nightly 1.101.0 (822182db4), sans modification de sa politique.
- Packaging complet S3 A+B : table officielle ESP-IDF v5.5.1 vérifiée,
  bootloader 29 744 octets (marge 3 024 avant 0x8000), application 1 132 064
  octets (marge 440 800 dans un slot), image Structure/Full acceptée aux deux offsets.
- Deux composants nominaux (4 108 octets chacun) et trois variantes de faute
  (panic, ignore-stop, log-flood) réellement compilés/packagés et validés par le
  gate IWNI. Ces builds ne démontrent aucune exécution sur APP_CPU.
- Warning de segment RWX S3 upstream conservé. Images de validation sans
  credentials réseau : reconstruire avec les paramètres du banc avant flash.

Empreintes des artefacts locaux (pas d'images binaires committées) :

| Artefact | SHA256 |
| --- | --- |
| app.bin | `cb28adfb80980ea5d0b7806088fb0511716114c6286ad5f3e90f6a77eb849aba` |
| bootloader.bin | `5a1ea1d4b9e57223e1a5d030774e4adbdaa54627b98d413cad131877f050a0dd` |
| partitions.bin | `264c0df1dec49d2200fe30d6f55a61c87d80bd674eb522fde93f1a58e3edf220` |
| native-a.iwni | `41150ac5a9708296a7a6178da08c388f280765bd36c931a066733981734d7b2e` |
| native-b.iwni | `20800fcb5f752477a74f6143ac102c5459dab92c07ab05daf26e4318ed4ead8a` |
| native-a-panic.iwni | `721a9cf32a674d478e7399833491759702a0685622b081ab00ccf656fa524bb7` |
| native-a-ignore-stop.iwni | `3f1b618022315ad8e5ba47380e681ae1024469acc53ec612fca053370ab691d3` |
| native-a-log-flood.iwni | `800beaf282b1308f4287c65109d77bcba6c590b5dd47d2c9b70a485921a6bba2` |

Aucun flash ni essai carte dans cet environnement. La PR reste en brouillon
jusqu’au bilan matériel approprié. #70/#71/#72 inchangées.
