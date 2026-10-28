# Transactional A/B Boot — format de démarrage A

Ce document décrit le **format de boot ESP32 de A** (métadonnées de boot, version 2), et non la politique générique de l'ensemble des firmwares. Voir [Firmware A/B](dual-ota.md) pour les modes A seul et A+B.

## Terminologie

**Transactional A/B Boot** est le nom fonctionnel du mécanisme de sélection, d'activation, de confirmation et de retour arrière des firmwares A/B. Le composant portable visé est `common/firmware/boot` et son API doit parler de **boot metadata**, **boot plan** et **boot state**.

Le format sur flash s'appelle simplement *métadonnées de boot* (`BootMetadata`), version 2, identifié par les quatre octets ASCII `BOOT`. Aucun sigle propre au projet ne le désigne.

La version 2 remplace la version précédente sans compatibilité : le projet est en développement et la rupture est assumée (#93). Une métadonnée d'une autre version ou d'un autre magic **n'est jamais lue ni convertie** ; elle arrête le boot explicitement (voir [reprovisionnement](boot-metadata-reprovisioning.md)). Le mécanisme transactionnel (mot commit, trois commandes flash, repli sur le dernier `Valid`) est inchangé.

## Objectif et périmètre

Une mise à jour ne doit jamais effacer l'unique image déjà confirmée pour activer une candidate. Le bootloader distingue **image écrite**, **image tentée**, et **image confirmée** ; un démarrage réussi n'est pas une confirmation.

La logique pure de boot ne possède ni flash ni driver : elle produit un plan d'opérations. Un adaptateur ESP32 exécute ce plan, relit et vérifie les écritures, puis démarre le slot choisi. L'application peut demander une activation, confirmer un démarrage après self-check, ou rejeter une candidate.

Ne sont **pas** couverts ici : téléchargement HTTP, digest/signature d'image, partition discovery, politique de timeout/watchdog, réseau, modèles d'agent/workload ou Secure Boot.

## Disposition flash v2

La partition ESP `otadata` comprend **deux secteurs de 4096 octets**. Une entrée de **32 octets** se trouve au début de chaque secteur. La disposition reprend la taille et les offsets de `esp_ota_select_entry_t`, mais **pas ses règles de validation**.

| Offset | Taille | Champ | Valeur / règle |
| --- | ---: | --- | --- |
| 0 | 4 | `ota_seq` | Séquence u32 little-endian, non nulle et différente de `0xffffffff` |
| 4 | 4 | `magic` | Octets ASCII `BOOT` |
| 8 | 4 | `version` | u32 = 2 |
| 12 | 4 | `ext_crc` | CRC-32 sur séquence, état, magic et version |
| 16 | 4 | `reserved` | `0xffffffff` |
| 20 | 4 | `commit` | `0x5AC3A53C`, programmé **en dernier** |
| 24 | 4 | `ota_state` | État u32 |
| 28 | 4 | `idf_crc` | CRC-32 sur `ota_seq` |

Les champs u32 sont little-endian. Le calcul CRC suit la fonction `crc32_le(u32::MAX, data)` de la crate (équivalente à `zlib.crc32(data, 0xffffffff)`) : il ne faut pas lui substituer une autre convention CRC sans vecteurs de tests.

Une entrée totalement effacée (`0xff` partout) est **Blank**. Une entrée dont un seul invariant échoue est **Corrupt** : écriture déchirée, format ESP-IDF standard, autre magic, autre version (y compris une version future). Il n'existe pas de mode de compatibilité implicite.

## États et transitions

| État | Valeur | Sens |
| --- | ---: | --- |
| `New` | 0 | Image sélectionnée mais jamais démarrée |
| `PendingVerify` | 1 | Image démarrée une fois, attend confirmation |
| `Valid` | 2 | Image confirmée, point de repli fiable |
| `Invalid` | 3 | Candidate rejetée après échec de vérification |
| `Aborted` | 4 | Candidate `PendingVerify` non confirmée lors du redémarrage |

**Activation :** conserver l'entrée `Valid` et écrire `New` dans l'autre secteur. Refuser l'activation s'il n'existe aucun `Valid` de référence. Choisir une nouvelle séquence compatible avec le slot cible.

**Démarrage :** convertir tout ancien `PendingVerify` non confirmé en `Aborted`. Parmi les candidates `Valid` et `New`, choisir par séquence décroissante en vérifiant d'abord que l'image est amorçable. Marquer `New` comme `PendingVerify` **avant** de transférer l'exécution. Une candidate non amorçable devient `Invalid`.

**Confirmation :** après self-check positif de l'application, écrire `PendingVerify → Valid`. **Rejet :** écrire `PendingVerify → Invalid`, puis redémarrer selon la politique de l'application. Si la candidate reste `PendingVerify` jusqu'au prochain reset, la logique de démarrage l'abandonne et peut revenir au dernier `Valid`.

**Premier boot :** uniquement si les **deux secteurs sont totalement effacés**, le slot 0 est initialisé en `Valid` **s'il est amorçable** ; sinon arrêt explicite (raison 1). Toute autre situation sans entrée acceptée, c'est-à-dire un secteur ni vierge ni valide (écriture déchirée, métadonnées corrompues ou de format inconnu), arrête le boot (raison 3, `UnknownMetadata`) : **aucun réensemencement silencieux**, car il pourrait effacer un état confirmé. Une entrée acceptée dont l'image n'est pas amorçable donne l'arrêt « aucune entrée utilisable » (raison 2). Un secteur corrompu à côté d'une entrée acceptée n'empêche pas le boot (cas d'une mise à jour interrompue). En production, `otadata` est écrit à l'usine ([procédure](boot-metadata-reprovisioning.md)) ; la fenêtre d'une coupure pendant le premier ensemencement mène à un arrêt récupérable par reprovisionnement.

## Atomicité de publication

Une modification d'entrée se fait par **trois commandes flash séparées** :

1. Effacer le secteur cible, jamais celui qui détient le dernier `Valid` à protéger.
2. Programmer les 32 octets du corps, `commit` restant à `0xffffffff`, puis **relire et comparer**.
3. Programmer les quatre octets du mot `commit` dans une commande distincte.

Le décodeur n'accepte que les entrées totalement cohérentes. Une coupure pendant les étapes 1 ou 2 ne doit pas faire reconnaître une nouvelle entrée. Une coupure lors du commit final ne doit aboutir qu'à une entrée absente/incomplète ou à une entrée totalement valide.

Cette propriété suppose que le pilote flash respecte les commandes séparées, la vérification de lecture et l'ordre des opérations. La bibliothèque de décision **ne garantit pas à elle seule** la durabilité matérielle.

## Invariants de migration

- **Disposition flash :** deux secteurs, 32 octets par entrée, offsets de `esp_ota_select_entry_t` conservés pour la séquence, l'état et le CRC `idf_crc`, mot commit et états inchangés.
- **Fallback :** ne jamais effacer l'unique entrée `Valid` pendant l'activation.
- **Un seul essai :** une candidate `New` devient `PendingVerify` avant le premier démarrage ; un `PendingVerify` non confirmé est abandonné après reset.
- **Aucune validation implicite :** seul l'application peut confirmer après ses propres contrôles.
- **Séparation des responsabilités :** logique pure d'un côté, application du plan flash et bootloader de l'autre.
- **Sécurité :** un CRC n'authentifie pas le firmware ; signature / Secure Boot relèvent d'un autre mécanisme.

## Vérifications requises

1. Tests hôte de décodage, états, sélection, séquences, activation, confirmation et rejet.
2. **Tests adversariaux** : coupure pendant chaque commande, corps déchiré, commit partiel, dernière image `Valid` préservée, métadonnées inconnues (autre magic, version passée ou future) sans écriture ni réensemencement. Les vecteurs de référence sont recoupés par `common/firmware/boot/tools/otadata_vectors.py`.
3. Builds ciblés C3/S3 de l'adaptateur et du bootloader.
4. Campagne matérielle : activation, interruption de flash, timeout watchdog, absence de confirmation, rollback et reprise après réinitialisation.

### Références de migration

- `iobewi/iobewi_old/firmware/boot/src/lib.rs`
- `iobewi/iobewi_old/firmware/boot/src/tests.rs`
- `iobewi/iobewi_old/firmware/esp32`
- `iobewi/iobewi_old/bootloader/esp`

**Évolution future :** tout changement de disposition ou de magic est une nouvelle version, jamais une modification silencieuse de la version 2. Un bootloader n'interprète que sa propre version.
