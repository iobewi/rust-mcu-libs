# Transactional A/B Boot — format de démarrage A

Ce document décrit le **format historique du boot ESP32 de A**, et non la politique générique de l'ensemble des firmwares. Voir [Firmware A/B](dual-ota.md) pour les modes A seul et A+B.

## Terminologie

**Transactional A/B Boot** est le nom fonctionnel du mécanisme de sélection, d'activation, de confirmation et de retour arrière des firmwares A/B. Le composant portable visé est `common/firmware/boot` et son API doit parler de **boot metadata**, **boot plan** et **boot state**.

**EWBT** n'est **pas** conservé comme nom d'architecture. Les quatre octets ASCII `EWBT` restent toutefois le *magic* historique **du format binaire v1 sur flash**. Les renommer ou les réécrire sans migration explicite rendrait illisibles les métadonnées existantes. On parlera donc de *Transactional A/B Boot metadata v1 (legacy magic `EWBT`)*.

Ce document formalise le comportement de `iobewi_old/firmware/boot` ; il **ne crée pas une nouvelle version du format**. La migration vers `rust-mcu-libs` doit conserver le comportement et reprendre les tests adversariaux avant toute fusion.

## Objectif et périmètre

Une mise à jour ne doit jamais effacer l'unique image déjà confirmée pour activer une candidate. Le bootloader distingue **image écrite**, **image tentée**, et **image confirmée** ; un démarrage réussi n'est pas une confirmation.

La logique pure de boot ne possède ni flash ni driver : elle produit un plan d'opérations. Un adaptateur ESP32 exécute ce plan, relit et vérifie les écritures, puis démarre le slot choisi. L'application peut demander une activation, confirmer un démarrage après self-check, ou rejeter une candidate.

Ne sont **pas** couverts ici : téléchargement HTTP, digest/signature d'image, partition discovery, politique de timeout/watchdog, réseau, modèles d'agent/workload ou Secure Boot.

## Disposition flash v1

La partition ESP `otadata` comprend **deux secteurs de 4096 octets**. Une entrée de **32 octets** se trouve au début de chaque secteur. La disposition reprend la taille et les offsets de `esp_ota_select_entry_t`, mais **pas ses règles de validation**.

| Offset | Taille | Champ | Valeur / règle |
| --- | ---: | --- | --- |
| 0 | 4 | `ota_seq` | Séquence u32 little-endian, non nulle et différente de `0xffffffff` |
| 4 | 4 | `magic` | Octets ASCII `EWBT` (héritage v1, ne pas renommer en flash) |
| 8 | 4 | `version` | u32 = 1 |
| 12 | 4 | `ext_crc` | CRC-32 sur séquence, état, magic et version |
| 16 | 4 | `reserved` | `0xffffffff` |
| 20 | 4 | `commit` | `0x5AC3A53C`, programmé **en dernier** |
| 24 | 4 | `ota_state` | État u32 |
| 28 | 4 | `idf_crc` | CRC-32 sur `ota_seq` |

Les champs u32 sont little-endian. Le calcul CRC suit la fonction existante `crc32_le(u32::MAX, data)` du code v1 : il ne faut pas lui substituer une autre convention CRC sans vecteurs de tests.

Une entrée totalement effacée (`0xff` partout) est **Blank**. Une entrée dont un seul invariant échoue est **Corrupt**. Une entrée au format ESP-IDF standard, sans les extensions v1, est donc **rejetée** par la logique actuelle : il n'existe pas de mode de compatibilité implicite.

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

**Premier boot :** si les deux secteurs n'ont aucune entrée utilisable ni candidate antérieure rejetée, le slot 0 peut être initialisé en `Valid` **uniquement s'il est amorçable** ; sinon arrêt explicite.

## Atomicité de publication

Une modification d'entrée se fait par **trois commandes flash séparées** :

1. Effacer le secteur cible, jamais celui qui détient le dernier `Valid` à protéger.
2. Programmer les 32 octets du corps, `commit` restant à `0xffffffff`, puis **relire et comparer**.
3. Programmer les quatre octets du mot `commit` dans une commande distincte.

Le décodeur n'accepte que les entrées totalement cohérentes. Une coupure pendant les étapes 1 ou 2 ne doit pas faire reconnaître une nouvelle entrée. Une coupure lors du commit final ne doit aboutir qu'à une entrée absente/incomplète ou à une entrée totalement valide.

Cette propriété suppose que le pilote flash respecte les commandes séparées, la vérification de lecture et l'ordre des opérations. La bibliothèque de décision **ne garantit pas à elle seule** la durabilité matérielle.

## Invariants de migration

- **Compatibilité flash :** conserver les deux secteurs, les 32 octets, `EWBT` comme magic v1, tous les offsets, le CRC, le mot commit et les états.
- **Fallback :** ne jamais effacer l'unique entrée `Valid` pendant l'activation.
- **Un seul essai :** une candidate `New` devient `PendingVerify` avant le premier démarrage ; un `PendingVerify` non confirmé est abandonné après reset.
- **Aucune validation implicite :** seul l'application peut confirmer après ses propres contrôles.
- **Séparation des responsabilités :** logique pure d'un côté, application du plan flash et bootloader de l'autre.
- **Sécurité :** un CRC n'authentifie pas le firmware ; signature / Secure Boot relèvent d'un autre mécanisme.

## Vérifications requises

1. Tests hôte de décodage, états, sélection, séquences, activation, confirmation et rejet.
2. **Reprise des tests adversariaux de `iobewi_old/firmware/boot`** : coupure pendant chaque commande, corps déchiré, commit partiel, dernière image `Valid` préservée.
3. Builds ciblés C3/S3 de l'adaptateur et du bootloader.
4. Campagne matérielle : activation, interruption de flash, timeout watchdog, absence de confirmation, rollback et reprise après réinitialisation.

### Références de migration

- `iobewi/iobewi_old/firmware/boot/src/lib.rs`
- `iobewi/iobewi_old/firmware/boot/src/tests.rs`
- `iobewi/iobewi_old/firmware/esp32`
- `iobewi/iobewi_old/bootloader/esp`

**Évolution future :** si nous souhaitons remplacer effectivement les octets `EWBT` sur flash, cela devra être un **format v2** spécifié séparément, avec migration, compatibilité descendante et tests de coupure supplémentaires ; il ne faut pas modifier v1 silencieusement.
