# Démarrage vérifié : exigences et preuves (#91, #104)

État au 2 novembre 2026. **Profil logiciel, ESP32-S3.** Cette page distingue ce qui est implémenté, ce que les
tests hôte établissent, ce que les campagnes matérielles ont démontré et ce qui n'est pas couvert. Elle ne
remplace pas [`verified-boot.md`](verified-boot.md) (conception) : elle en donne les preuves.

## Code qualifié

* Tête de la PR #105 : `9ba953362e52103c488bb298b218843464a6b5b9` (étiquette `verified-boot-s3-qualified`).
  Parent : `d659865`, qui ne diffère de la tête de #101 (`cc3a8a1`) que par un commit de documentation.
* Second étage qualifié : 34 976 octets, SHA-256 `a7e884a2d0057865565e51e7665ebaa497882f3a7f4f4cb3129878ef318667d1`,
  reconstruit deux fois à l'identique depuis ce commit.
* Applications exécutées sur la carte avec le code de cette tête : l'application de la campagne « contrôle
  canonique » (SHA-256 `bb9cc15fe2bdfdbd1a3647c35640ce0d0bb1a41ada22875260c6ace5756e734a`) et l'image de test du
  rollback (`c602e6d5a05d50229b207713202141dee81597516db195f3b63ced7dc43d46f6`, feature `hang-self-check` et
  une ligne de trace). Les applications A″/B″ des premières campagnes (`a8ca0ed7…`, `1a36e280…`) sont construites
  d'un état antérieur du code : elles ne couvrent pas la console de #102/#103 ni #105. Les images contiennent les
  paramètres du banc et ne sont pas publiées : seules leurs empreintes le sont.
* Carte : ESP32-S3 QFN56 v0.2, flash 16 Mio, DevKitC-1 N16.

## Matrice

« Hôte » : tests exécutés par la CI. « Matériel » : campagne sur la carte, rapport en commentaire de PR (liens
ci-dessous). Toutes les preuves matérielles concernent l'**ESP32-S3** et une exécution unique.

| Propriété | Hôte | Matériel (ESP32-S3) | Limites |
|---|---|---|---|
| Une image non authentique n'est pas exécutée (profil vérifié) | `bootauth`, `auth` | P3-F, M4, P4-A, P4-C | — |
| Repli authentifié sur le slot précédent | `boot_selection`, `ota_sequence` | P4-B, M4-bis, rollback | repli vers `ota_1` démontré ; le sens `ota_1 → ota_0` ne l'est pas |
| Signature RSA-PSS, clé épinglée par l'empreinte de sa structure complète | `auth/tests/rsa_algorithm`, `bootauth` | P4-A (`BadSignature`) | clé de qualification du banc ; pas de rotation |
| Intégrité de l'image (SHA-256 sur la taille signée) | `bootauth` | P4-C (`DigestMismatch`) | — |
| Longueur signée = longueur canonique au démarrage (`BadSize`, raison `0x6e`) | `canonical_size` (7), `esp_tests` (12) | M4-bis | — |
| Longueur canonique à l'upload | différentiel hôte (vrais validateurs) | contrôle canonique : image refusée en HTTP 409 sans publication, relecture de la flash | attribution par élimination : un seul test regroupe digest, structure et longueur canonique, sans journal de la cause |
| Upload signé et refus d'un upload invalide | `auth_service` | P5-B0, P5-B | — |
| La preuve suit l'image (invalidation, écriture, relecture) | `proof_write`, `provision` | P5-B, P5-C | — |
| Refus d'activation sur preuve invalide (`bootable`) | `auth_service` | P5-A | la réponse 409 est générique |
| Activation, `PendingVerify`, confirmation, persistance | `boot/tests` | P5-C | — |
| Rollback `PendingVerify` → `Aborted` → image précédente `Stable` | modèle de scénario | rollback | reset logiciel après le délai de 15 s ; chien de garde matériel non exercé |
| Atomicité des métadonnées (`otadata`, `ota_meta`, preuve) | coupure simulée à chaque commande flash | transitions relues octet pour octet | **aucune coupure d'alimentation qualifiée sur matériel** |
| Non-régression du boot non signé | profil par défaut conservé | référence de toutes les campagnes | — |

Rapports (PR #101 sauf mention) : [P3-F](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6542161790),
[M2-C](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6545206328),
[M4](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6545612392),
[P4-A](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6545924074),
[P4-B](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6546295590),
[P4-C](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6549892382),
[P5-B0](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6555494464),
[M4-bis (#104)](https://github.com/iobewi/rust-mcu-libs/issues/104#issuecomment-6561924341),
[P5-B](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6562124090),
[P5-A](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6572373649),
[P5-C](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6573341394),
[contrôle canonique (#105)](https://github.com/iobewi/rust-mcu-libs/pull/105#issuecomment-6574371681),
[rollback](https://github.com/iobewi/rust-mcu-libs/pull/101#issuecomment-6575199025).

## ESP32-C3 et autres cibles

* **ESP32-C3 : non qualifié sur matériel.** La CI compile et lie le second étage vérifié et l'application
  `signed-rsa`, avec un gabarit de taille ; aucune exécution. Les fonctions de la ROM du C3 (RSA-PSS, SHA-256)
  n'ont jamais été appelées : aucun résultat du S3 ne s'y applique.
* RP2350 et Teensy : hors périmètre.
* Le profil ECDSA P-256 de l'OTA (#90) n'est pas utilisé avec le démarrage vérifié (RSA-PSS seulement) et n'a pas
  été qualifié sur matériel dans ce cadre.

## Ce que ces preuves n'établissent pas

Racine de confiance matérielle (Secure Boot V2, eFuses, JTAG, chiffrement de la flash), anti-rollback persistant,
rotation et conservation des clés (#92), coupures d'alimentation sur matériel, chien de garde matériel,
`reject-self-check` sous démarrage vérifié, absence de toute image authentique (HALT), repli `ota_1 → ota_0`,
disponibilité du repli après un upload refusé, panic après confirmation.

Issues de suivi : #106 (ESP32-C3), #107 (racine matérielle ESP32-S3), #108 (panic après confirmation), #109 (repli
et diagnostic des rejets), #110 (qualification complémentaire), #111 (RP2350 et Teensy), #92 (anti-rollback, clés,
coupures).
