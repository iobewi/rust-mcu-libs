# Firmware A/B — architecture de mise à jour

## Décision d'architecture

Le catalogue `rust-mcu-libs` fournit des **briques firmware indépendantes**. La configuration la plus simple ne possède qu'un firmware **A** en deux slots A0/A1. Une application peut choisir **A+B**, avec deux autres slots B0/B1. **A et B ne sont pas des catégories de logiciels** : ce sont deux domaines de firmware, aux cycles de vie séparés.

Le scénario typique place les services transverses (réseau, Wi-Fi, TLS, logs, supervision) dans A et la logique métier dans B. **Ce n'est pas une obligation de la bibliothèque.** Le nom « workload » décrit seulement une manière possible d'utiliser B.

| Mode | Description |
| --- | --- |
| A seul | Firmware complet A0/A1, OTA et rollback classiques ; aucun composant B requis |
| A+B | Deux domaines indépendants, chacun avec deux slots, sa validation et son rollback |
| Mise à jour B seule | Le domaine B peut être relivré sans réécrire A ; l'activation sans reboot dépend du runtime utilisé |

## Arborescence retenue

```text
common/
└── firmware/
    ├── image/         # Description et validation des images
    ├── slots/         # Slots logiques, sans format ESP imposé
    ├── boot/          # Politique de boot transactionnel
    └── update/
        ├── core/      # Transactions communes (stream, digest, reprise)
        ├── a/         # Activation/confirmation/rollback du domaine A
        └── b/         # Activation/confirmation/rollback du domaine B
```

Les répertoires `update/a` et `update/b` exposent des **politiques optionnelles**. `update/core` n'impose pas deux domaines. Cette arborescence est **la cible**, non un inventaire de crates déjà livrées.

## Abstraction et activation

Une transaction de mise à jour peut partager les opérations suivantes : préparer, écrire dans le slot inactif, vérifier l'intégrité, publier durablement, activer, réconcilier après interruption, confirmer ou restaurer.

La **politique d'activation** dépend du domaine et de la composition choisie :

- En mode classique, A est sélectionné par le bootloader et confirmé après redémarrage. Sur ESP32, l'adaptateur de boot actuel emploie les métadonnées **Transactional A/B Boot v1** (magic binaire historique `EWBT`).
- En mode A+B de type résident/workload, A peut servir de superviseur et charger B indépendamment. Le bootloader ESP ne sélectionne alors que A ; B possède son propre contrat de persistance et de rollback.
- La bibliothèque ne doit **pas imposer** ce modèle à toutes les applications : d'autres mécanismes d'activation peuvent être fournis via composition explicite.

Le fait de nommer B « firmware » ne garantit ni isolation mémoire, ni chargeur dynamique, ni absence de redémarrage ; ces propriétés dépendent de l'implémentation réelle.

## Slots et compatibilité flash

`A0/A1` et `B0/B1` sont des identifiants **logiques**. Le crate générique `common/firmware/slots` ne doit pas présupposer `ota_0`/`ota_1`, ni un nombre fixe de partitions physiques. Les noms `ota_0`/`ota_1` et `PARTITION_LAYOUT = "embewi-ab-v1"` appartiennent au **contrat ESP32 historique du firmware A** et doivent rester compatibles dans l'adaptateur ou un module de compatibilité explicite.

**Attention à la PR #38 :** son `AppSlot::{Ota0,Ota1}` représente actuellement ce contrat hérité, **pas encore la future abstraction générique**. Ne pas la fusionner comme API générique définitive sans réviser cette portée.

## État durable

- **OTM1** : contrat historique de transaction pour la mise à jour A.
- **OTM2** : contrat historique de transaction distinct pour le domaine B dans `iobewi_old`.
- Le format **Transactional A/B Boot v1** concerne la sélection au boot de A sur ESP32 ; les octets `EWBT` restent inchangés pour la compatibilité flash, voir [la spécification de boot](transactional-ab-boot.md).

Une mise à jour A+B n'est **pas automatiquement atomique entre les deux domaines**. Les versions successives peuvent coexister ; les éventuelles règles de compatibilité entre A et B sont décidées par le produit/runtime, pas par les primitives de stockage.

## Validation attendue

1. **A seul :** mise à jour A0↔A1, coupures lors des écritures, confirmation et rollback, sans dépendance sur B.
2. **A+B :** mise à jour de chaque domaine isolément, conservation de l'autre, pannes/coupures séparées, reprise de transaction et rollback.
3. **Composition résident/workload (si retenue) :** tests de compatibilité de l'API d'exécution, comportement du chargeur, supervision et éventuel basculement sans reboot.
4. **ESP32 :** builds C3/S3 et qualification matérielle du bootloader, du flash et des états de redémarrage.

Références : `iobewi_old/docs/dual-ota.md`, `iobewi_old/docs/otm2.md`, `iobewi_old/firmware/model`. Ces sources sont utiles pour porter les contrats existants, mais leurs choix Agent/Workload ne sont plus des restrictions universelles de la nouvelle bibliothèque.
