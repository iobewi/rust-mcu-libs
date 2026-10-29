# OTA du domaine système A

Composition de référence C3/S3 : upload HTTPS → écriture du slot inactif →
digest du flux et digest relu en flash → validation ESP Full → publication
OTM1 → activation → reboot → probation → confirmation ou rollback.

Le code est raccordé ; la qualification sur carte reste obligatoire.
Aucune activation ni transaction OTM1 n'est fabriquée au démarrage.
Le bootloader historique conserve son mode Structure : le contrôle Full réalisé
avant publication dans cet exemple ne change pas la politique du second étage.
Le digest assure l'intégrité, pas une signature/authenticité de l'image.

## Composition et limites

Un seul SharedFlash sert EKV, les images et otadata. Un seul SystemBoot est
possédé par la tâche, emprunté par BootRuntime pendant le boot puis utilisé
par les routes. Son mutex logique sert un seul executor, distinct du mutex
physique. Une requête d'upload conserve la réservation exclusive jusqu'à la
publication ; ArtifactWriteSession est le seul propriétaire de WriteSession.
UploadManager n'est pas imbriqué dans cette composition.

L'upload est traité en blocs de 2 Kio ; le writer dispose de son secteur de
4 Kio et efface en avance par lots de 64 Kio. Le serveur traite une connexion
à la fois, sans session reprise entre requêtes. Un transfert interrompu ne
publie rien ; il peut être recommencé. Le flux incomplet, trop grand, un mauvais
digest ou une image non bootable ne sont jamais publiés. L'ancien staged est
résolu par service::begin avant le premier erase. Activating et PendingVerify
refusent un nouveau transfert. Une image entièrement publiée reste staged
jusqu'à une demande explicite d'activation du même identifiant.

Le verrou physique est relâché avant tout accès au store OTM1 ou à ConfigSpace. Après finish,
les octets écrits sont relus et hachés, puis le vrai validateur ESP vérifie
l'image avec la MemoryMap C3/S3 et une limite égale à la taille reçue. La
réservation reste vivante pendant ces contrôles et la publication OTM1.

L'activation réutilise activate_staged : intention OTM1 persistée avant la
transition otadata, restauration staged si celle-ci échoue. Le reboot intervient
après réponse, même si son envoi échoue. Une réponse perdue ne signifie donc
pas que l'activation n'a pas eu lieu : consulter les logs avant un nouvel essai.

Le heap de référence est de 160 Kio, comme l'exemple HTTPS existant ; ressources
réseau : 3 sockets, buffers TLS RX/TX de 4 Kio chacun, HTTP 2 Kio. La consommation
réelle du heap et de la pile, les timings radio/flash et le handover restent à
mesurer sur carte. Les credentials de cet exemple sont intégrés au build.

## Démarrage et probation

HAL → TIMG0 → esp-rtos → watchdog 20 s → SharedFlash → `ota_meta` → ConfigSpace
(Wi-Fi) → boot gate. Aucun second TimerGroup n'est construit. OTM1 vit dans la
partition `ota_meta` (deux secteurs, trames CRC + séquence, voir
`docs/firmware/ota-meta-reprovisioning.md`) : ConfigSpace ne réserve plus d'espace
`ota`. Le self-check de confirmation exige le store `ota_meta` lisible **et** la
configuration montée et saine (CRC/records en lecture seule) ; une configuration
qui ne se monte pas n'empêche pas la décision de boot : l'image en probation est
rejetée et le rollback reste possible, jamais confirmée. Sa deadline de
probation est de 15 s. La confirmation durable précède le désarmement.
Refus, timeout ou échec de confirmation provoquent un reset ; le watchdog reste
armé. Une confirmation durable suivie d'un échec de finition OTM1 est signalée
Degraded, avec nouvelle tentative au prochain boot. Un boot stable est contrôlé
sans politique de reset probatoire. Les erreurs de découverte initiale bloquent
avec watchdog armé ; un blocage avant son armement reste hors fenêtre.

`reject-self-check` et `hang-self-check` sont mutuellement exclusifs. Le premier
refuse le contrôle, le second le bloque de façon coopérative. Une vraie intention
OTM1 Activating concordante et un fallback confirmé sont requis pour tester la
probation. Aucun effacement automatique de configuration ni rescue n'est ajouté.

## Connexion et reconnexion Wi-Fi

Les deux étalons OTA utilisent `iobewi-wifi-manager` (`common/net/wifi/manager`)
avec le transport `iobewi-esp-wifi`. Le manifeste ConfigSpace ne déclare plus que
`wifi` ; OTM1 a sa propre partition `ota_meta`.

Au premier démarrage sans configuration Wi-Fi valide, les identifiants du build
servent au provisioning : connexion et DHCP, puis commit WFC1 par le gestionnaire.
Un échec est retenté avec la temporisation existante (1, 2, 4, 8 puis 10 secondes),
sans panic ni reboot. Les identifiants enregistrés sont prioritaires aux boots
suivants ; changer les variables de build ne remplace pas ces valeurs persistées.
Pour changer de réseau, utiliser le provisioning du gestionnaire dans une
composition adaptée, ou préparer explicitement une nouvelle configuration usine.

`maintain()` reste actif en parallèle du serveur HTTPS : celui-ci attend la
première connexion, puis le gestionnaire reconnecte la station après une perte
de lien. Il n'y a pas de reset automatique du firmware en cas d'échec Wi-Fi.
La persistance EKV garde les limites documentées dans #70.

Validation carte #72 : désactiver les resets automatiques du banc sur échec de
join ; observer une nouvelle tentative dans le même boot, puis `/health` ;
couper/rétablir le point d'accès et vérifier le retour HTTPS sans reboot.
La cause radio des refus `DisassociatedDueToInactivity` reste à déterminer.
Campagne S3 et correction de pile : [ota-wifi-board-campaign.md](../../../docs/firmware/ota-wifi-board-campaign.md).

## Construction

Installer Rust nightly + rust-src/RISC-V, esp 1.98.1.0 + GCC Xtensa, CMake,
Clang ESP étendu pour le build TLS C3, et esptool 5.5.0. Charger export-esp.sh et
placer les exécutables requis sur PATH, comme pour l'exemple https-health.
Définir avant le build :

```sh
export IOBEWI_STA_SSID='your-network'
export IOBEWI_STA_PASSWORD='your-password'
export IOBEWI_OTA_TOKEN='your-long-random-token'
export IOBEWI_TLS_CERT_PEM="$(cat server-cert.pem)"
export IOBEWI_TLS_KEY_PEM="$(cat server-key.pem)"
```

Le certificat doit avoir un SAN correspondant au nom/IP utilisé par le client.
Le serveur ne démarre pas sans token et identité TLS. Il n'ouvre aucun port HTTP
clair. Les routes de mutation exigent Bearer ; `/health` indique seulement que
le serveur répond, pas une mesure détaillée de santé.

Depuis la racine du dépôt, préparer sans flasher, avec le générateur officiel
ESP-IDF de tables de partitions :

```sh
python3 examples/firmware/ota-system/prepare.py --chip esp32s3 \
  --partition-tool /absolute/path/esp-idf/components/partition_table/gen_esp32part.py
```

La commande construit/lie bootloader et application, génère les images ESP avec
header 4MB (ou 16MB avec `--flash-size 16MB`), contrôle que le bootloader finit avant la table
et que bootloader et application portent la même géométrie, valide l'application aux deux
offsets lus dans la table CSV, génère une otadata initiale avec le **codec réel** et la table CSV.
Elle ne contacte aucune carte. Résultats : `target/ota-system/<chip>/`.
Utiliser `--chip esp32c3` pour C3. Les tables sont des profils explicites (4 Mio minimum, plus les
deux tables 16 Mio de la carte S3 : `partitions-s3-16mb-legacy.csv` et `partitions-s3-16mb-wide.csv`) ;
la bibliothèque découvre réellement les partitions. L'offset de la table vient de
`ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET` (défaut 0x8000) et le script écrit dans
`flash-plan.txt` les offsets, tailles et SHA-256 de ce qu'il faut flasher, tirés de la table.

Pour une première installation volontaire sur une carte d'essai :

```sh
python3 -m esptool --chip esp32s3 --port /dev/ttyUSB0 write-flash \
  0x0 target/ota-system/esp32s3/bootloader.bin \
  0x8000 target/ota-system/esp32s3/partitions.bin \
  0xd000 target/ota-system/esp32s3/otadata.bin \
  0xf000 target/ota-system/esp32s3/ota_meta.bin \
  0x20000 target/ota-system/esp32s3/app.bin \
  0x3c0000 target/ota-system/esp32s3/config.bin
# Offsets above are the default 4 MiB profile; with another table offset or flash size, the
# offsets to use are the ones listed in target/ota-system/<chip>/flash-plan.txt.
```

Cette installation initialise explicitement ota_0 comme Valid. Elle remplace
les régions citées et remet explicitement ConfigSpace et le store OTM1 (`ota_meta`) à zéro. Ne pas réutiliser l'otadata usine pour les
mises à jour normales : utiliser le transfert OTA ci-dessous.

## Transfert et activation

`POST /ota/upload`, Content-Length requis, pas de Transfer-Encoding :
Authorization Bearer, X-Deployment-ID (1–64 caractères ASCII alphanumériques,
`-_.`) et X-Image-SHA256 au format `sha256:<64 hex>`. Body : image ESP binaire,
pas l'ELF. Réponse 200 `staged` seulement après vérification et publication.

`POST /ota/activate`, body vide et même X-Deployment-ID : réponse 200
`activating` et reboot. Les refus renvoient 400/401/409 selon l'étape ; aucune
activation implicite après upload. Le client stdlib vérifie TLS avec le CA fourni :

```sh
python3 examples/firmware/ota-system/client.py https://device.local \
  target/ota-system/esp32s3/app.bin nominal-2 --ca server-cert.pem --activate
```

Omettre `--activate` pour inspecter l'état staged avant activation. Pour tester
refus/blocage, garder une image nominale en fallback, reconstruire avec
`prepare.py --fault reject-self-check` ou `--fault hang-self-check`, puis transférer
et activer la nouvelle image. Ne pas flasher son otadata usine. Observer le
retour au fallback, puis tester une mise à jour nominale dans l'autre sens.

## Vérification et gates restantes

Tests hôte du service runtime/config-space et de l'admission des requêtes ; CI
`host-esp32-linked-bins` : tests/Clippy hôte et liens réels C3/S3. Le S3 conserve
l'avertissement upstream RWX. Les scénarios ci-dessous restent à exécuter sur
carte : premier boot, reboot stable, transfert/activation dans les deux sens,
mauvais digest, transfert interrompu, image refusée, identifiant d'activation
incorrect, probation nominale/refus/timeout, watchdog réellement expiré,
coupures durant écriture/publication/activation/confirmation et échec de finition
OTM1. Vérifier le fallback confirmé et la configuration après chaque essai.

Aucun flash/essai matériel n'est réalisé par la livraison logicielle. La chaîne
supervisée B/C reste distincte ; #61 ouverte et #65 Draft jusqu'aux gates finales.

## Authentification des images (profils)

Deux profils partagent le même moteur (voir
[docs/firmware/ota-image-authentication.md](../../../docs/firmware/ota-image-authentication.md)) :

* **par défaut, `UnsignedAllowed`** : intégrité et validation d'image, comportement
  inchangé, aucune cryptographie asymétrique compilée ;
* **`--features signed`, `SignatureRequired`** : chaque upload porte un manifeste
  IAM1 signé (`X-Image-Manifest`), vérifié avant le premier effacement, puis
  représenté et re-vérifié à l'activation (le slot est aussi re-haché). La clé
  publique épinglée est fournie par `IOBEWI_OTA_SIGNING_PUBKEY` (hex SEC1, 130
  chiffres) ; sans elle le build échoue, jamais de repli vers le non signé.

`ota_sign` (exemple de `iobewi-firmware-auth`, hôte seulement) génère la clé et
signe ; `client.py --manifest FICHIER` envoie l'enveloppe à l'upload et à
l'activation. Le domaine B/C (`ota-supervised`) n'est pas concerné : sa combinaison avec
`signed` est refusée à la compilation. Le serveur journalise `OTA authentication: UNSIGNED`
ou `SIGNATURE_REQUIRED` au démarrage.

## ConfigSpace EKV — nouveau profil de développement

Partition `config` data/0x40, 0x3c0000, taille 0x40000 (256 Kio), sans NVS.
Les offsets/taille des deux images A et d'otadata restent ceux du scénario.
La composition construit Store/EspConfigFlash une seule fois, manifest ota2048,
seed d'allocation fourni par le RNG HAL (pseudo-aléatoire avant activation RF).
Une région entièrement vierge est initialisée au premier boot ; une région
nonblank corrompue échoue sans formatage. Pas de migration/compatibilité CSM1.

`config.bin` est un fichier vierge de préparation **à flasher uniquement pour
une installation/reset volontaire en développement**. Ne pas le flasher entre
les mises à jour OTA ou les tests de conservation après reboot. Les anciennes
images NVS ne constituent pas un fallback compatible : installer deux images
EKV puis qualifier les mises à jour dans les deux sens.

Le défaut de reprise après interruption de commit EKV est accepté pour poursuivre
le développement et suivi dans #70. Les tests nominaux ne qualifient pas cette
garantie. Les coupures visant les commits ConfigSpace restent une campagne
séparée ; un mount échoué n'efface jamais automatiquement les données.

Campagne nominale : [procédure ConfigSpace/EKV et OTA A](../../../docs/config/issue-68-board-campaign.md).
