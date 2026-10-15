# Migration USB / flux — issue #66

## Inventaire avant portage (15 octobre 2026)

Base cible : `f6db4eacd1fa16e3a48943cf90d6a7d0aea6455f`.
Source historique : `iobewi/iobewi_old`, commit
`345378e` (les chemins ci-dessous sont figés par ce commit).
Consommateur consulté : `iobewi/streambewi`, `app/src/usb.rs`
(blob `2ff5effa6ad095afdeceb2cb54b8c116bf1161fb`) et `app/src/stream.rs`
(blob `ad58bd0265dfef008ffd830d341f2b886770ec7b`).

| Source historique | Cible | Contrat et preuve à conserver |
| --- | --- | --- |
| `fs/block` | `common/fs/block` | Secteurs de 512 octets, capacité inclusive stable, Ready/Pending/Expired, sortie indisponible nulle, début/fin de session |
| `fs/fat16` | `common/fs/fat16` | Un fichier contigu virtuel, géométrie validée, métadonnées et mapping identiques ; six tests `volume.rs` et fixture golden |
| `stream/rolling` | `common/stream/rolling` | Fenêtre bornée sans allocation, positions absolues, une session rebasée, contre-pression ; quatre tests historiques |
| `drivers/usb/msc` | `common/usb/msc` | Classe Embassy USB, BOT/SCSI limité, identité injectée, politique indisponible fournie par appelant ; cinq tests unitaires et cinq dialogues `session.rs` |
| `drivers/usb/esp32` | `arch/esp32/usb` | Construction OTG FS S3 sur GPIO20/19, buffer OUT fourni par appelant ; compilation S3 avec pilote Embassy réel |
| StreamBeWI `app/src/usb.rs`, `app/src/stream.rs` | Hors de #66 ; #67 | VID/PID, identité radio, prebuffer, délai 5 s/poll 5 ms, URL, reconnexion HTTP, verrouillage partagé, provisioning et bascule JTAG restent produit |

## Réutilisation et décision

Aucune réécriture des algorithmes historiques. Embassy reste le runtime et
l'interface USB ; `esp-hal` fournit le pilote OTG, sans nouvelle couche HAL.
`embassy-usb` stable 0.6.0 n'expose pas de classe MSC dans sa liste de classes :
<https://docs.embassy.dev/embassy-usb/0.6.0/default/class/index.html>.
La branche git d'Embassy annonce maintenant MSC ; elle n'est pas une version
stable sélectionnée ici. `usbd-storage` cible `usb-device`, pas le pilote async
Embassy retenu : <https://docs.rs/usbd-storage/latest/usbd_storage/>.
Les systèmes FAT qui montent un disque existant ne remplacent pas le générateur
virtuel historique dont les octets constituent le contrat à préserver.
Une queue FIFO ne remplace pas les lectures aléatoires/rebasées de la fenêtre.

Le petit contrat bloc est conservé car FAT16 et MSC l'utilisent réellement : une
simple API de lecture de disque immuable ne porte ni Pending/Expired ni sessions.
Le flux circulaire reste synchrone et indépendant de ce contrat et d'Embassy.
Il n'a aucun lien avec `common/log/stream` ou le streaming du serveur HTTP.

## Parité attendue et limites

Conserver les APIs et noms de packages. Adapter uniquement chemins de dépendances,
édition workspace, documentation et CI ; consigner toute correction nécessaire.
Conserver la fixture FAT de 133632 octets, SHA256
`41ec718b804d5e317f8c46670856d6abfe9ebfb8fd38f59964134654426c6eed`.
Pas de conformité BOT/SCSI complète : ne pas modifier les réponses, le parser,
la gestion des commandes non supportées ni les limites historiques au passage.
Une déconnexion/reconnexion de l'endpoint termine/recrée la session. Une future
annulée ou une attente illimitée imposée par la politique ne garantit pas ce cleanup.

Le portage logiciel ne qualifie pas l'énumération physique, la PHY après reset,
le lecteur Metronic ni une autre famille de MCU. Aucun changement OTA.
Les résultats effectivement exécutés seront ajoutés après vérification.

## Vérification du lot portable

- Outils : `rustc 1.99.0 (b940084d7 2026-09-28)` sur x86_64 Linux.
- Dépendances effectivement résolues : Embassy USB 0.6.0, USB driver 0.2.2,
  Embassy time 0.5.1, log 0.4.34. Aucun Cargo.lock suivi.
- Les 20 tests historiques passent : FAT16 6, rolling 4, MSC unitaires 5,
  MSC dialogues 5. Un sixième dialogue ajoute l'assemblage rolling/FAT16/MSC,
  vérifie les paquets de données, la sous-alimentation et une nouvelle session
  après progression du producteur et bouclage de la fenêtre : PASS.
- Exemples README exécutables FAT16/rolling et exemple MSC compilé par rustdoc : PASS.
- Tests et Clippy strict de MSC avec `usb-debug` : PASS.
- Clippy strict `--all-targets` des quatre crates, Rustfmt global et `git diff --check` : PASS.
- Fixture golden : empreinte identique à celle de l'inventaire.
- Comparaison des quatre implémentations avec la source : identiques hors
  espaces/formatage et écarts listés ci-dessous ; les assertions historiques restent.

Écarts sans changement de politique : `is_multiple_of` remplace les modulos
équivalents (Clippy stable), `as_chunks` remplace `chunks_exact` dans le test FAT,
`State::default()` délègue à `new()`, et les READMEs sont inclus dans rustdoc.
Les nouveaux tests et les dépendances FAT/rolling/embassy-futures de MSC sont
uniquement des dépendances de développement. Les chemins workspace et la CI
sont ceux du nouveau dépôt ; aucune infrastructure de l'ancien framework n'est portée.

Le lot portable fournit les briques ; l'adaptateur S3 est livré dans une PR séparée.
La qualification USB physique et le portage de StreamBeWI restent à réaliser.
