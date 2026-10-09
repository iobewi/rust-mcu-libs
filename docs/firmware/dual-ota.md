# Dual OTA — système résident A et workload B

## Périmètre et terminologie

**Dual OTA** décrit deux domaines de mise à jour **indépendants** sur un même microcontrôleur :

- **OTA A — système résident (Agent/runtime)** : la partie transversale et persistante du produit : Wi-Fi, réseau, configuration, supervision, logs, transport TLS, orchestration et chargement du workload. Elle possède deux slots logiques **A0/A1** (partition ESP `ota_0` / `ota_1` dans le layout actuel). Le **bootloader** sélectionne A.
- **OTA B — workload métier (application/userspace)** : l'image de logique métier chargée ou démarrée **par A**. Elle possède éventuellement deux slots logiques **B0/B1**. Le **superviseur résident A**, jamais le bootloader ESP, sélectionne B.

« Userspace » désigne ici une **frontière logicielle de composition et de cycle de vie**, pas une garantie d'isolation mémoire ou de privilèges : celles-ci exigent un runtime et, le cas échéant, un MPU/MMU adaptés. L'existence de B n'implique pas à elle seule l'exécution indépendante ni le hot-swap ; ces propriétés nécessitent un chargeur et un superviseur.

```text
ROM → bootloader ────────→ A0 / A1 (résident)
                                │
                                └── superviseur/loader ─────→ B0 / B1 (workload)
```

**Deux modes obligatoires :**
1. **Classique, OTA A seule** : A0/A1 contiennent l'application complète ; B est absent. Aucun loader, stockage B, API workload ou surcharge obligatoire.
2. **Dual OTA, A+B** : A0/A1 contiennent les services transverses et le runtime B ; B0/B1 contiennent la logique métier. A peut évoluer sans B et B peut évoluer sans A **si la compatibilité est respectée**.

La mise à jour du *workload B uniquement*, sans redémarrer le MCU, est un objectif architectural conditionné au runtime de chargement ; elle n'est pas garantie par les seules primitives OTA.

## Matrice fonctionnelle

| Dimension | OTA A | OTA B |
| --- | --- | --- |
| Contenu | Firmware résident / infrastructure | Image applicative métier |
| Slots | A0 et A1 | B0 et B1 (si B activé) |
| Sélection au boot | Bootloader du MCU | Runtime/superviseur de A |
| Validation | Self-check de A ; compatibilité avec B actif si applicable | Health/liveness de B par A |
| Confirmation | A confirme après reboot | A confirme après activation de B |
| Retour arrière | Bootloader restaure l'ancien A | A réactive l'ancien B |
| Redémarrage matériel | Généralement requis | Pas requis **si le runtime le permet** |
| Métadonnées durables héritées | OTM1 et format de boot v1 `EWBT` | OTM2 |
| Périmètre sécurité | Bootability / confiance du firmware A | Chargement sûr de B et contraintes runtime |

Les *positions* A0/A1 et B0/B1 sont des noms logiques. Elles ne supposent ni quatre partitions `data/ota` ESP-IDF ni un format d'image commun : l'image A est amorçable par le bootloader ESP, tandis que le format de B dépend du chargeur choisi.

## Chaînes de mise à jour

**OTA A**

1. Recevoir une demande ciblant `System` (hérité : `Agent`), choisir le slot A inactif et télécharger/écrire l'image.
2. Vérifier taille, digest, format et, lorsque requis, authenticité de l'image.
3. Publier transactionnellement la sélection de l'image A : format **Transactional A/B Boot v1** dans `otadata` (signature binaire historique `EWBT`).
4. Redémarrer ; le bootloader passe la nouvelle A à `PendingVerify` avant le transfert.
5. A exécute ses self-checks et vérifie qu'elle peut encore servir B actif si B existe, puis confirme ou rejette ; à défaut de confirmation après reset, le bootloader revient à l'ancienne A validée.

**OTA B**

1. Recevoir une demande ciblant `Workload`, vérifier la version d'API runtime demandée par B.
2. Écrire et vérifier l'image dans le slot B inactif ; publication durable de la transaction B, **indépendamment** des métadonnées A.
3. A ordonne au superviseur de charger/activer B ; le bootloader ne participe pas à cette décision.
4. A surveille la santé de B et confirme l'activation ou rétablit le slot B précédemment validé. Une panne de B ne doit pas effacer l'unique B sain.

Une livraison combinée A+B peut être coordonnée, mais **n'est pas une transaction atomique inter-domaines**. Chaque domaine conserve son rollback. Le plan de livraison doit anticiper les combinaisons intermédiaires de versions.

## API runtime entre A et B

La couche de contrôle exprime un **artefact souhaité** et une **cible logique**, pas un numéro de slot. Le device choisit le slot inactif.

- A **fournit** une `RuntimeApi { major, minor }`.
- B **requiert** une `RuntimeApi { major, minor }`.
- Compatibilité de base : `major` égal et `provided.minor >= required.minor`.
- Un B incompatible est refusé **avant** son activation.
- Un nouvel A incapable de prendre en charge le B actif **ne doit pas être confirmé** ; le rollback de A reste possible.
- En mode A seule, **aucune exigence de B** ne conditionne la confirmation de A.

Ce contrat modélise une API d'exécution. Il ne présume pas de syscalls, de processus OS, de protection MPU ou de scheduling préemptif.

## Séparation des briques du catalogue

```text
common/
  firmware/
    slots/         ← vocabulaire A0/A1 pour le layout firmware ESP actuel (PR #38)
    image/         ← description, digest, validation d'image A
    boot/          ← Transactional A/B Boot, format v1 EWBT : **A uniquement**
    update/        ← primitives transactionnelles communes (stream, digest, reprise, reconcile)
    model/         ← ciblage System/Workload, compatibilité API, politique A/B (à définir)
  workload/
    image/         ← validation de l'image B, selon le format retenu
    update/        ← transactions/états durables B (OTM2) et activation par superviseur
arch/esp32/
  firmware/        ← adaptateur flash/partition/boot A ; support B selon matériel
```

Ce schéma indique des **responsabilités proposées**, pas des crates toutes déjà présentes. Les implémentations sont à créer progressivement, sans générer de wrappers sans valeur ajoutée.

La fonction `firmware/boot` couvre **exclusivement A**. Étendre le format `EWBT` à B serait une erreur : B ne relève pas du bootloader. En revanche les primitives transport, écriture et contrôle d'intégrité peuvent être communes si leurs interfaces restent indépendantes de la politique d'activation.

## Compatibilité persistante

- **`EWBT`** : les quatre octets du *magic* du format Transactional A/B Boot v1 pour les **slots firmware A uniquement** ; ils ne deviennent pas une signature B. La documentation de ce format est dans [transactional-ab-boot.md](transactional-ab-boot.md). La v1 ne doit pas changer silencieusement.
- **OTM1** : format durable de transaction OTA A hérité ; son encodage et ses clés sont à préserver durant la migration.
- **OTM2** : format durable indépendant pour les transactions OTA B. Sa compatibilité et sa reprise après reset sont à qualifier séparément (source historique : `iobewi_old/docs/otm2.md`).
- **Slots B** : ne pas recycler le `PARTITION_LAYOUT = "embewi-ab-v1"` de A pour affirmer qu'il décrit aussi B ; le schéma de stockage/chargement B doit être défini explicitement.

## Critères de validation

**Mode A seule :** OTA A0→A1→A0, arrêt brutal pendant flash, `PendingVerify`, watchdog, confirmation et rollback ; zéro dépendance requise vers des briques B.

**Mode A+B :** B0→B1 indépendamment de A, échec santé B → retour B0, mise à jour A avec B inchangé, refus d'incompatibilité A/B, coupures lors des publications OTM1 et OTM2, persistance des deux domaines séparément.

**Sur matériel :** vérifier le comportement du loader B, l'isolement des buffers et la coexistence flash/Wi-Fi, puis qualifier explicitement les transitions sans reboot (si implémentées). Les tests hôte d'état ne prouvent pas ces propriétés.

## Sources de référence dans `iobewi_old`

- `docs/dual-ota.md` : distinction Agent/Workload, compatibilité, deux politiques.
- `docs/otm2.md` : contrat durable du workload.
- `firmware/model` : `UpdateTarget`, `AbSlots`, `AgentOta`, `WorkloadOta`, `RuntimeApi`.
- `firmware/boot` : sélection transactionnelle de **A** seulement.
- `firmware/update` et `workload/update` : mécanique commune et chemin dédié à B.

**Principe directeur :** deux domaines OTA autonomes, deux autorités d'activation, des briques partagées au bon niveau, et un mode classique A seule qui reste un citoyen de première classe.
