# Reprovisionnement du bootloader et d'`otadata`

Les métadonnées de boot sont au format **version 2** (magic `BOOT`, voir [transactional-ab-boot.md](transactional-ab-boot.md)). La version précédente n'est pas lue : la rupture est assumée (#93). Un appareil qui détient des métadonnées d'un autre format **s'arrête** au démarrage (raison 3 dans le journal du bootloader) au lieu d'être réensemencé. Cette procédure le remet en service.

## À savoir avant d'agir

- **L'OTA applicative actuelle ne met pas à jour le bootloader** (0x0). Elle ne met pas non plus `otadata` au nouveau format : le bootloader et l'application modifient `otadata` dans le cadre normal des transitions d'activation, de confirmation et de rollback, mais uniquement au format v2 déjà en place. Le passage au format v2 n'est donc **pas une migration OTA automatique** : il exige une intervention physique (USB/UART) sur le bootloader et sur les métadonnées incompatibles, à faire une fois par appareil.
- **Ordre impératif : bootloader d'abord, `otadata` ensuite.** Un ancien bootloader qui lirait un `otadata` v2 le jugerait corrompu et réensemencerait le slot 0 sans prévenir, ce qui peut démarrer un firmware plus ancien que celui confirmé. Un nouveau bootloader sur un ancien `otadata` s'arrête au contraire explicitement.
- Le reprovisionnement **ne touche pas** à la configuration (`config`), à `wl_meta` ni aux images : il écrit uniquement le bootloader et `otadata`.
- Un retour arrière vers un ancien bootloader après écriture de la version 2 n'est pas supporté.

## Journal du bootloader

| Message | Sens |
|---|---|
| `boot: HALT, nothing safe to boot, reason 1` | `otadata` vierge et slot 0 non amorçable. |
| `... reason 2` | Des entrées valides existent, aucune ne mène à une image amorçable. |
| `... reason 3` | `otadata` n'est ni vierge ni une entrée valide : écriture déchirée, corruption, ou métadonnées d'un autre format (par exemple un appareil déjà déployé). |

## 1. Choisir le slot à confirmer

Il faut désigner le slot qui contient l'image à conserver (`ota_0` = 0, `ota_1` = 1).

- **Appareil de la version précédente :** la séquence (octets 0 à 3) et l'état (octets 24 à 27) sont aux mêmes offsets. Lire `otadata` sans rien écrire :
  `esptool --chip esp32s3 read_flash 0xd000 0x2000 otadata-old.bin`
  Le slot est `(seq - 1) % 2` de l'entrée la plus récente dont l'état vaut 2 (`Valid`), à 0 et à 0x1000.
- **Appareil neuf ou non confirmé :** prendre le slot 0.

## 2. Construire les artefacts

```sh
# bootloader (profil de la CI), puis image ESP et contrôle de taille (< 0x8000 octets)
cargo +nightly build -p iobewi-esp-bootloader --features esp32s3 --profile bootloader \
    -Z build-std=core,alloc --target xtensa-esp32s3-none-elf   # toolchain +esp pour l'S3
python3 -m esptool --chip esp32s3 elf2image --flash-size 16MB --output bootloader.bin <elf>

# otadata : entrée Valid pour le slot choisi, 0x2000 octets, générée par le vrai codec
cargo run -p iobewi-esp-boot --example factory_otadata -- otadata.bin <0|1>
```

Le script `common/firmware/boot/tools/otadata_vectors.py` encode indépendamment une entrée pour recouper `otadata.bin` (`python3 otadata_vectors.py 1:valid`).

## 3. Flasher (hors CI, accord explicite requis)

```sh
python3 -m esptool --chip esp32s3 write_flash 0x0 bootloader.bin     # 1. bootloader
python3 -m esptool --chip esp32s3 write_flash 0xd000 otadata.bin     # 2. otadata
```

Ne pas inverser les deux étapes (voir plus haut). Pour un appareil neuf, la procédure d'installation usine des campagnes (effacement complet, bootloader, table, `otadata`, application) reste valable, avec `otadata.bin` généré comme ci-dessus.

## 4. Vérifier

Au premier démarrage, le journal doit montrer `boot: otadata at 0xd000`, un plan `slot=<n> seq=<n+1>` sans écriture, puis le démarrage de l'application sans `HALT`. Après un cycle d'activation, de confirmation et de redémarrage, relire `otadata` : les deux entrées portent `424f4f54` (« BOOT ») à l'offset 4 et `02000000` à l'offset 8.

## Limites

- Une coupure d'alimentation pendant le tout premier ensemencement (`otadata` vierge) laisse un secteur déchiré et mène à l'arrêt de raison 3. C'est pourquoi `otadata` doit être écrit à l'usine ; sinon, effacer `otadata` puis suivre l'étape 3.
- Cette procédure a été validée sur l'hôte uniquement. Elle n'a **pas** été exécutée sur une carte : la qualification matérielle (S3, puis C3) reste à faire.
