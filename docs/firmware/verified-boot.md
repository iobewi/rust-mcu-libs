# Démarrage vérifié (#91)

Une signature vérifiée à l'upload n'est pas un Secure Boot : les octets d'un slot peuvent changer
ensuite. Ce lot décide, **avant l'exécution** d'un slot, s'il porte encore une image authentique.
Il s'appuie sur #90 (IAM1, clés épinglées) sans toucher à OTM1, OTSF ni BOOT v2.

**État : implémenté et testé sur hôte. Le second étage vérifié ne tient pas dans les 32 Kio de la
table historique sur ESP32-S3 ; il tient avec la table à `0x10000` (`flash-geometry.md`) et sur ESP32-C3
avec la géométrie historique.** Rien n'a été installé sur une carte.

## Deux profils de boot

| | `UnsignedBoot` | `VerifiedBoot` |
|---|---|---|
| Contrôles | validation structurelle de l'image (inchangée) | structure **et** preuve IAP1 valide |
| Cryptographie asymétrique | aucune, rien n'est lié | RSA-PSS 3072 par la ROM |
| Slot sans preuve valide | démarre | **refusé**, jamais exécuté |
| Repli vers le non vérifié | — | **aucun** : il n'existe pas de chemin de code |

Le profil est choisi à la construction du bootloader (`--features verified-boot`) ; le journal
affiche `boot authentication: UNSIGNED` ou `VERIFIED`. `VerifiedBoot` ne se construit pas sans clé
épinglée, produit ni variante : `IOBEWI_BOOT_PINNED_KEYS` (1 à 3 condensés), `IOBEWI_BOOT_PRODUCT`,
`IOBEWI_BOOT_VARIANTS`, éventuellement `IOBEWI_BOOT_MIN_SECURITY_VERSION`. Aucune valeur par défaut.

## La preuve IAP1

Le dernier secteur de 4 Kio de chaque slot applicatif (`PROOF_SECTOR_SIZE`) est réservé. Aucune
image n'y écrit : la capacité utile d'un slot est sa taille moins ce secteur, **pour tous les chemins**
(jeton d'écriture OTA du domaine A, validation de l'image au boot et à l'activation, outil
`validate_image`). La table de partitions ne change pas.

```text
  0  magic "IAP1"     4  version u16 = 1     6  algorithme u16 (identifiant IAM1)
  8  longueur u32     12 crc32 u32 (octets 0..12 + charge)
 16  mot de validation 0x5AC3A53C, programmé en dernier par sa propre commande
 20  charge : enveloppe IAM1 (manifeste 76 + signature 384) | clé publique (776)
```

* **Lien image ↔ preuve** : par le manifeste signé (taille et SHA-256 de l'image), pas par la position.
  Une preuve périmée ou recopiée d'un autre slot échoue à la comparaison du digest.
* **Écriture** : effacer le secteur, programmer l'en-tête puis la charge, relire, et seulement alors
  programmer le mot de validation. Une coupure laisse un secteur vierge (`ProofAbsent`), un en-tête sans
  validation (`ProofTruncated`) ou l'enregistrement complet. Le mot de validation a des bits équilibrés :
  un mot partiellement programmé n'en est jamais un.
* **Clés** : épinglées par le SHA-256 de leur encodage **complet** (776 octets : module, exposant et
  les deux valeurs de Montgomery précalculées). Modifier `rinv` ou `mdash` change l'empreinte et la ROM
  les utilise (mesuré) : elles ne peuvent pas être laissées hors de l'épingle.

## Décisions

Le bootloader garde `plan_boot` : seule la question « cette image est-elle démarrable ? » change.
`select::plan` en tire trois réponses (démarrable, refusée, **erreur de lecture**) et jette le plan si
une lecture a échoué, pour qu'une erreur d'E/S ne soit jamais écrite dans `otadata` comme `Invalid`.

| Situation | Décision |
|---|---|
| Candidat authentique | `New` → `Pending`, probation inchangée |
| Candidat altéré, sans preuve, clé inconnue, autre cible, digest faux | refusé, `Invalid` ; le confirmé authentique démarre |
| Confirmé altéré, repli authentique | le repli démarre, le confirmé est marqué `Invalid` |
| Aucun slot authentique | HALT ; aucune écriture `otadata` (comportement existant sur HALT) |
| Erreur de lecture flash | plan jeté, flash réinitialisée, 3 essais, puis arrêt sans rien écrire |
| Refus cryptographique | confirmé par une seconde vérification avant d'être cru |
| Échec fonctionnel en probation | inchangé : `Aborted`, distinct d'un refus d'authenticité (`Invalid`) |

Un `otadata` vierge amorce `ota_0` seulement si ce slot porte une preuve valide : un appareil de
fabrication doit recevoir ses preuves avant son premier démarrage.

## Cryptographie : la ROM

Le second étage appelle `ets_sha_*` et `ets_rsa_pss_verify` de la ROM (ESP32-S3 et C3). Mesuré sur
un ESP32-S3 (phase A3) : sel de 32 octets exigé, clé et signature en petit-boutiste, aucune initialisation
du moteur RSA, environ 2,5 ms par vérification et 204 ms de hachage par Mio **en contexte applicatif**.
La vérification en contexte bootloader et la durée de lecture flash restent à mesurer.

IAM1 reçoit l'identifiant d'algorithme 2 (`RsaPss3072Sha256`). P-256 reste inchangé. Le contrôle des
champs de cible (`Target`) est partagé avec le service OTA, et une lecture « en place » du manifeste
(`RawManifest`, `Target::check_raw`) évite de décoder les énumérations dans le second étage ; un test
vérifie qu'elle ne peut jamais accepter ce que le décodage refuse.

## Taille

| Second étage (octets) | ESP32-S3 | ESP32-C3 |
|---|---|---|
| Non signé | **30 016** | **23 008** |
| Vérifié | **34 352** | **26 912** |
| Région avant la table, géométrie historique (0x8000) | 32 768 : le vérifié dépasse de 1 584 | 32 768 : marge 5 856 |
| Région avant la table, géométrie large (0x10000) | 65 536 (limite dure CI 61 440) : marge 27 088 | non utilisée |

Le choix retenu est de déplacer la table du S3 (`flash-geometry.md`) plutôt que de réduire le
bootloader. Le gain mesuré de `opt-level = "z"`, de `optimize_for_size` et de la lecture du manifeste « en
place » (−944 octets, retenue) reste dans l'historique de la PR ; `esp_hal::init` (environ 13 Kio) est
conservé. Cartes mémoire (IRAM, DRAM, pile) et budgets CI : `flash-geometry.md`.

La CI bloque le non signé et le vérifié selon la géométrie, rapporte le vérifié S3 historique (qui ne
tient pas) et bloque le vérifié S3 large.

## Ce que cela ne protège pas

* Un attaquant qui peut réécrire le bootloader (le profil logiciel ne protège pas son propre second
  étage). Une racine matérielle (Secure Boot V2 : la ROM authentifie le second étage avec un condensé en
  eFuse) est un profil distinct, non implémenté ici. Sur S3 elle exigerait un second étage ≤ 28 672 octets
  (secteur de signature de 4 Kio).
* Le rejeu d'une ancienne image valablement signée (anti-rollback persistant : #92).
* La rotation ou la révocation dynamique des clés.

## Outils et migration

`cargo run -p iobewi-firmware-bootauth --features host-sign --example iap1_pack -- sign KEY IMAGE PRODUCT MCU VARIANT SECVER OUT`
produit `OUT.env` (enveloppe IAM1 en base64url) et `OUT.iap1` (secteur de preuve de 4096 octets) ;
`keygen`, `pin` et `inspect` complètent l'outil. La clé privée ne quitte pas la machine qui signe.

Un appareil déjà qualifié n'a aucune preuve : installer un second étage vérifié le mettrait en HALT.
L'écriture de la preuve à l'activation OTA n'est **pas encore câblée** dans l'exemple ; en l'état, les
preuves se provisionnent depuis l'hôte. Voir `verified-boot-qualification.md` et `geometry-reprovisioning.md`.
