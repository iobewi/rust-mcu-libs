# Reprovisionnement contrôlé du banc ESP32-S3 vers la géométrie large (#91)

**Procédure préparée, non exécutée.** Chaque étape d'écriture exige une autorisation distincte et un arrêt
à la première divergence. Elle suppose le banc décrit dans `verified-boot-qualification.md`.

## Principe

Le projet est en développement : on reprovisionne explicitement plutôt que de migrer en OTA. Rien du
firmware actuel n'est conservé, à une exception près, `config`. Les applications actuelles (Ns, Ns2)
cherchent leur table à `0x8000` : elles ne démarrent pas devant une table à `0x10000`. Il faut les
**recompiler avec le même offset**, ce qui change leurs digests, donc leurs enveloppes IAM1 et leurs preuves IAP1.

| Région | Devient | Sort |
|---|---|---|
| second étage `0x0`–`0x8000` | remplacé (même chemin d'écriture) | sauvegardé |
| table `0x8000` | effacée (le second étage en occupe la place) | sauvegardée |
| `otadata` `0xd000`, `ota_meta` `0xf000` | abandonnés (inclus dans la zone du second étage) | sauvegardés |
| table `0x10000`, `otadata` `0x11000`, `ota_meta` `0x13000` | neufs | — |
| `ota_0`, `ota_1` | applications recompilées pour `0x10000` | sauvegardés (ancien Ns2, prototype A3) |
| `config` `0x380000` | **conservée** | sauvegardée |

Perte explicite : l'état OTM1 (déploiement actif `p90-ns2`), l'état `otadata` (Ns2 confirmé en `ota_1`) et
les deux images actuelles. Aucun n'est recoupé par la nouvelle disposition ; tout est sauvegardé.

## R0 : état de départ et sauvegardes (lecture seule)

Identité (MAC, 16 Mio), `/health` 200, puis lecture et SHA-256, `0400`, de : `0x0`–`0x9000` (second étage et
table), `0xd000`–`0x11000` (`otadata`, `ota_meta`), `0x11000`–`0x20000` (**doit être entièrement `0xff`** :
c'est la future zone de la table, d'`otadata` et d'`ota_meta`, jamais relue depuis #94), les deux slots
entiers, `config`. Une région non vierge là où la nouvelle disposition écrit arrête la procédure.

## R1 : fabriquer, hors carte

```sh
export ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET=0x10000
cargo run -p iobewi-firmware-bootauth --features host-sign --example iap1_pack -- keygen bench-boot.pem
# Identifiants du banc (Wi-Fi, jeton, identité TLS) dans l'environnement, hors Git.
python3 examples/firmware/ota-system/prepare.py --chip esp32s3 --flash-size 16MB \
  --auth signed-rsa --boot unsigned --rsa-key bench-boot.pem \
  --partition-tool /chemin/esp-idf/components/partition_table/gen_esp32part.py     # image A
python3 examples/firmware/ota-system/prepare.py ... --boot verified ...            # second étage vérifié
# image B: même commande avec une autre identité TLS ; garder app.bin et app.env
cat target/ota-system/esp32s3/flash-plan.txt        # offsets, tailles, SHA-256
```

`prepare.py` refuse une incohérence de géométrie (bootloader au-delà de la table, application estampillée pour
un autre offset, fichier de partitions incohérent), dérive la clé épinglée par l'application et par le
bootloader du même fichier, et écrit pour l'image `app.bin` : `app.env` (enveloppe pour `X-Image-Manifest`) et
`app.iap1` (preuve du slot installé à la main). **Les empreintes d'une même construction ne sont pas
reproductibles** (date de compilation dans le descripteur d'application, sel aléatoire de la signature RSA-PSS) :
figer les empreintes des fichiers finaux, ne plus les reconstruire. Vérifier chaque couple image + preuve avec
`iap1_pack verify` avant de toucher la carte.

## R2 : preuve de récupération (avant la première écriture)

Reprendre H2 de `verified-boot-qualification.md` : mode téléchargement ROM par le banc, réécriture à
l'identique du second étage actuel, démarrage de l'ancien Ns2. La restauration d'un second étage
expérimental n'est pas considérée comme garantie tant que H2 n'a pas réussi. Aucun effacement destructif du
second étage dans la procédure de base.

## R3 : écrire, en une session de chargeur, sans démarrage intermédiaire

1. blanchir `0x8000`–`0x10000` (fichier de `0xff`) : l'ancienne table disparaît ;
2. second étage (non signé d'abord) à `0x0` ;
3. table à `0x10000` ;
4. `otadata` d'usine à `0x11000` ; `ota_meta` d'usine à `0x13000` ; **ne pas** écrire `config.bin` (la `config` est conservée) ;
5. blanchir l'en-tête d'`ota_1` (`0x1d0000`, un secteur) : l'ancien Ns2 ne doit pas être pris pour une image ;
6. application recompilée à `0x20000` (`ota_0`) et sa preuve `app.iap1` à `0x1cf000` ;
7. relecture de chaque région écrite et comparaison aux SHA-256 de R1 ;
8. un seul reset.

L'`otadata` d'usine marque `ota_0` comme Valid (premier démarrage : `plan_boot` amorce le slot 0). Attendu :
`boot: table at 0x00010000`, `otadata at 0x00011000`, `ota_0 0x00020000`, `running ota_0 status Stable`,
`/health` 200, Wi-Fi (la `config` conservée), `OTA ready`.

## R4 : démarrage vérifié

Seulement après R3 réussi et sur autorisation : le second étage vérifié à `0x0` (la preuve d'`ota_0` est déjà
en place), puis la mise à jour OTA de l'image B, dont la preuve est écrite par l'appareil lui-même. Attendus et
refus : `verified-boot-qualification.md`, P3 et P4.

## R5 : retour à la géométrie historique

Restaurer depuis les sauvegardes de R0, dans cet ordre : `0x0`–`0x9000` (second étage et table historiques),
`otadata` à `0xd000` et `ota_meta` à `0xf000`, `ota_1` puis `ota_0`, puis blanchir `0x10000`–`0x15000`
(table et métadonnées larges, devenues inutiles). Un seul reset ; attendu : l'ancien Ns2, `ota_1 Stable`. En
cas de bootloader inutilisable, le mode téléchargement ROM reste l'accès, indépendant du contenu de la flash.

## Arrêts

Région « vierge » qui ne l'est pas, empreinte différente, écriture qui ne se relit pas, second étage
supérieur à 61 440 octets, journal qui annonce un autre offset de table que `0x10000`.
