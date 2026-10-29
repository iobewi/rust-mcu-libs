# Authentification des images OTA (#90)

Le SHA-256 d'une image prouve que les octets sont arrivés intacts ; il ne dit
rien de leur émetteur. Ce lot ajoute une **preuve d'authenticité** au moteur OTA
existant, sans second moteur : `upload → écriture → SHA-256 → validation de
l'image → autorisation → publication OTM1 → autorisation → activation`.

Il authentifie ce que l'OTA **publie et active**. Ce n'est ni Secure Boot ni une
vérification au démarrage (lot 2, voir `verified-boot.md`), ni un compteur anti-rollback persistant (#92),
ni une rotation de clés (lot 3).

## Deux profils, un seul moteur

`OtaService<P: AuthProfile>` (`iobewi-firmware-update`) est le seul chemin vers la
publication et l'activation. Les anciennes fonctions `service::begin`, `publish`
et `activate_staged` ne sont plus publiques. Chaque profil impose son type de
preuve (`P::Proof`) :

| | `UnsignedAllowed` | `SignatureRequired` |
|---|---|---|
| Preuve | `Unsigned` | `Admitted` |
| Intégrité (SHA-256 annoncé = reçu = relu), validation d'image, compatibilité | oui | oui |
| Manifeste IAM1 signé, clé publique épinglée | non | **obligatoire** |
| Publication | après intégrité | seulement avec une preuve liée à l'image publiée |
| Activation | comme avant | l'enveloppe est **représentée** et re-vérifiée, puis le slot est relu et re-haché |
| Coût asymétrique | aucun (P-256 non compilé) | vérification ECDSA P-256 |

Garanties de typage (testées par des `compile_fail`) :

* un service signé n'accepte pas `Unsigned` ; les preuves ont des champs privés et
  aucun constructeur public ;
* une preuve valable pour une autre image (taille ou digest) est refusée à `begin`
  comme à `publish` ;
* une preuve est **liée à la politique effective** qui l'a émise (empreinte SHA-256
  des contraintes de cible et, pour chaque clé épinglée, de son identifiant, son
  algorithme, son état et sa clé publique) : un service dont les clés, les états
  de clé, le plancher de version ou les variantes diffèrent la refuse
  (`PolicyMismatch`), y compris pour le même identifiant associé à une autre clé
  publique, ou après révocation de la clé émettrice ;
* `SignatureRequired::new` refuse une politique sans clé, sans clé active, sans
  variante matérielle ou avec des clés invalides : aucun repli vers le non signé ;
  le profil est choisi à la compilation (`UnsignedAllowed` par défaut,
  `--features signed` pour l'exemple) ; la sûreté repose sur les types, les
  features Cargo ne servent qu'à maîtriser les dépendances et la taille.

Au démarrage du serveur OTA, le firmware journalise le profil de son build :
`OTA authentication: UNSIGNED` ou `OTA authentication: SIGNATURE_REQUIRED` (aucune
clé ni secret dans ce message).

## Manifeste IAM1 v1

Binaire canonique de 76 octets, sans champ optionnel :
`IAM1 | version | algorithme | format d'image | MCU | key_id(8) | produit(16) |
variante(2) | réservé(2) | version de sécurité(4) | taille(4) | SHA-256(32)`.
La signature couvre `"iobewi.ota.image.v1\0" ‖ manifeste` (séparation de
domaine). L'enveloppe est le manifeste suivi de la signature ECDSA P-256 brute
`r ‖ s` (64 octets, `s` bas), soit 140 octets, transportée dans l'en-tête
`X-Image-Manifest` en base64url non rembourré strict. Le corps reste l'image
brute : son format et son SHA-256 ne changent pas. Toute représentation non
canonique est refusée (champ réservé non nul, produit non ASCII ou mal rempli,
longueur inattendue, signature tronquée, `s` haut, scalaire nul, base64 non strict).

## Politique de confiance locale

`TrustPolicy` : produit, famille MCU, format d'image, variantes matérielles,
version de sécurité minimale (plancher statique), et clés publiques épinglées
(`authorized_keys`) avec leur identifiant, leur algorithme et leur état
(actif ou révoqué à la compilation). Le manifeste ne décide jamais de ce qui est
autorisé : l'algorithme déclaré doit être celui épinglé pour la clé, et l'ordre
est clé connue → clé non révoquée → signature → cible → taille et digest.

## Autorisation entre publication et activation

Aucune preuve n'est stockée : ni OTM1 ni OTSF ne changent. L'enveloppe est
vérifiée à la publication (avant le premier effacement), puis **présentée de
nouveau** par le client à l'activation et vérifiée contre l'enregistrement
persistant sous la **politique en vigueur** ; le firmware re-hache le slot avant
d'activer. Conséquences :

* une image publiée sans signature (profil non signé) ne peut pas être activée en
  mode signé sans signature valide de cette image ;
* une clé révoquée ou un plancher relevé après la publication bloquent l'activation ;
* un redémarrage entre publication et activation ne perd rien et n'ajoute rien ;
* le client doit conserver l'enveloppe jusqu'à l'activation.

## Utilisation

```sh
# Clé de signature : hors dépôt, hors firmware.
cargo run -p iobewi-firmware-auth --features host-sign --example ota_sign -- keygen ~/keys/ota.key
export IOBEWI_OTA_SIGNING_PUBKEY=<public_key_hex affichée>   # épinglée dans le firmware
# Firmware signé (le build échoue si la clé est absente) :
cargo build ... -p example-firmware-ota-system --features esp32s3,signed --release
# Signer une image et l'envoyer :
ota_sign sign ~/keys/ota.key app.bin iobewi-ota-sys esp32s3 0 1 > app.manifest
python3 examples/firmware/ota-system/client.py https://device app.bin deploy-1 \
  --ca server-cert.pem --manifest app.manifest --activate
```

## Limites

* L'authenticité n'est vérifiée que lors de l'OTA ; un accès direct à la flash
  contourne tout (secure boot : lot 2, #91). Le CRC d'`ota_meta` n'authentifie rien.
* Plancher de version statique : un rejeu d'une ancienne image signée de version
  suffisante reste possible (#92). Révocation et rotation sont fixées à la
  compilation (lot 3).
* Le domaine B/C (OTM2, `ota-supervised`) **n'est pas couvert** : il garde son
  moteur et reste non authentifié. En conséquence `ota-supervised --features signed`
  **ne compile pas** (erreur explicite) tant que `/workload/*` n'est pas
  authentifié ; `ota-supervised` reste utilisable en profil non signé.
* Le Bearer HTTPS reste l'authentification de l'API d'administration ; les clés
  d'administration et de signature doivent rester distinctes. Un challenge-response
  asymétrique à clés épinglées pourrait la remplacer plus tard ; rien n'est
  implémenté ici.
* Temps de vérification et pile du P-256 : voir la section mesures ; non mesurés
  sur carte à ce stade.
