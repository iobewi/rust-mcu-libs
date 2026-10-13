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

Un seul SharedFlash sert NVS, les images et otadata. Un seul SystemBoot est
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

Le verrou physique est relâché avant tout accès ConfigSpace. Après finish,
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

HAL → TIMG0 → esp-rtos → watchdog 20 s → SharedFlash → NVS/OTM1 → boot gate.
Aucun second TimerGroup n'est construit. ConfigSpace réserve `ota`, budget
2048 octets. Le self-check utilise le canary NVS existant ; sa deadline de
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
header 4MB, contrôle la taille bootloader avant 0x8000, valide l'application aux
deux offsets, génère une otadata initiale avec le **codec réel** et la table CSV.
Elle ne contacte aucune carte. Résultats : `target/ota-system/<chip>/`.
Utiliser `--chip esp32c3` pour C3. Les deux tables sont des profils explicites
4 Mio minimum ; la bibliothèque découvre réellement les partitions.

Pour une première installation volontaire sur une carte d'essai :

```sh
python3 -m esptool --chip esp32s3 --port /dev/ttyUSB0 write-flash \
  0x0 target/ota-system/esp32s3/bootloader.bin \
  0x8000 target/ota-system/esp32s3/partitions.bin \
  0xd000 target/ota-system/esp32s3/otadata.bin \
  0x20000 target/ota-system/esp32s3/app.bin
```

Cette installation initialise explicitement ota_0 comme Valid. Elle remplace
les régions citées, sans effacer NVS. Ne pas réutiliser l'otadata usine pour les
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
