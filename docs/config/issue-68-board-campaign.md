# Campagne ESP32 — ConfigSpace EKV et OTA A (#68)

## Périmètre

S3 principal, C3 en confirmation. Tester le nouveau backend nominal, pas les
anciennes images NVS. Code/compilations et carte sont deux validations distinctes.
Le défaut de reprise EKV après interruption de commit reste suivi dans #70 ;
cette campagne ne le déclare pas corrigé. Les essais ci-dessous coupent/reset
uniquement après la fin des opérations. Pas de migration ni de rescue implicite.

Noter pour chaque essai : commit, chip, capacité flash, variantes, toolchains,
checksum SHA256 de l'image, commande de flash et logs série. Les credentials
Wi-Fi/Bearer/clé TLS ne doivent pas être joints aux rapports publics.

## 1. ConfigSpace isolé — config-manager

Charger export-esp.sh, installer les outils indiqués dans le README et compiler :

```sh
cargo +esp build -Z build-std=core,alloc --target xtensa-esp32s3-none-elf \
  -p example-config-manager --features esp32s3 --release --bins
```

La table standalone contient config à 0x9000/0x40000 et factory à 0x50000.
Pour une installation neuve volontaire sur la carte de développement, effacer
la carte, puis flasher avec un bootloader ESP-IDF compatible. Par exemple,
[espflash](https://github.com/esp-rs/espflash/tree/main/espflash) peut fournir son
bootloader par défaut et convertir la table CSV :

```sh
espflash erase-flash --port /dev/ttyUSB0
espflash flash --port /dev/ttyUSB0 --flash-mode dio \
  --partition-table examples/config-manager/partitions.csv --monitor \
  target/xtensa-esp32s3-none-elf/release/config-manager-esp32
```

Adapter le port réel ; les commandes de cette procédure ne sont pas exécutées
par les validations logicielles. Ne pas utiliser le bootloader OTA A avec cette
table factory. Si CARGO_TARGET_DIR est défini, adapter le chemin de l'ELF.

| Essai | Attendu |
| --- | --- |
| Premier boot sur région vierge | initialized generation 1 ; verified generation 1 |
| 10 reboots logiciels après verified | retained/verified generation 1, valeur config-ready |
| 10 débranchements/rebranchements après verified | même valeur et même génération |
| Variante exercise, sans effacer config | commit/clear/recommit PASS ; génération précédente +3 |
| Reboot exercise, sans effacer config | génération précédente retenue, puis +3 à nouveau |
| Retour à la variante normale | dernière génération conservée ; aucune réinitialisation |

Compiler exercise avec `--features esp32s3,exercise`. Ce mode réalise trois
commits par boot volontairement, puis laisse config-ready comme valeur finale.
Le mode normal n'écrit qu'à la première initialisation. Les assertions contrôlent
remplacement, absence après clear, readback et générations ; les logs donnent
la preuve observable. Aucun test de clear sur carte n'est déclaré réalisé avant
ces essais.

## 2. Refus d'une région non reconnue (optionnel, carte d'essai)

Après sauvegarde d'un dump utile, programmer délibérément des octets non vierges
sans format valide dans la région config. Démarrer le mode normal : attendu
mount failed, aucune réinitialisation automatique, région identique au dump.
Restaurer explicitement la région avant de poursuivre. Ce scénario est un test
fonctionnel de refus ; il ne corrige pas les corruptions dues à #70.

## 3. OTA A avec ConfigSpace EKV

Réinstaller le profil OTA complet : sa table et les offsets diffèrent du profil
standalone. Lire `examples/firmware/ota-system/README.md`, fournir SSID/password,
Bearer et certificat/clé TLS avec le SAN approprié, puis :

```sh
python examples/firmware/ota-system/prepare.py --chip esp32s3 \
  --partition-tool /absolute/path/esp-idf/components/partition_table/gen_esp32part.py
```

Le build valide les images aux deux offsets ; il produit bootloader, application,
otadata, table et config.bin vierge. Pour cette première installation/reset
volontaire, suivre la commande de flash complète du README : bootloader0,
table0x8000, otadata0xd000, app0x20000, config0x3c0000. **Config.bin et otadata
usine ne sont jamais reflashés entre les essais de conservation ou les OTA.**
Les images locales de validation sans credentials ne sont pas des images de
campagne réseau opérationnelles ; reconstruire avec les paramètres de la carte.

| Essai | Attendu |
| --- | --- |
| Premier boot / 10 reboots après stable | config montée, pas de canary écrit, boot stable et HTTPS disponible |
| Upload nominal sans activation | staged uniquement après digests, validation d'image et persistance OTM1 |
| Reboot après staged | état staged récupérable ; aucune activation automatique |
| Activation du même identifiant | reboot, PendingVerify puis confirmed, métadonnées terminées |
| Deuxième mise à jour dans l'autre sens | même résultat avec deux images issues du nouveau backend |
| Mauvais digest / identifiant d'activation | refus, slot confirmé conservé |
| Requête interrompue avant publication, alimentation maintenue | pas de publication incomplète ; nouveau transfert possible |
| Variante reject-self-check | rejet et rollback vers une image nominale EKV |
| Variante hang-self-check | timeout/watchdog et rollback nominal EKV |
| Reboot après confirmation | stabilité, metadata/config intactes |

Utiliser client.py et les variantes prepare.py décrits dans le README. Ne pas
utiliser d'ancienne image NVS comme fallback. Ne pas qualifier les coupures
pendant les commits ConfigSpace dans ce bilan nominal : les résultats connus
et leur réactivation restent dans #70. B/supervised n'a toujours pas d'exécutable.

## Décision de qualification

Joindre logs/dumps/checksums et marquer chaque ligne PASS/FAIL/non exécutée.
Ne pas confondre CI verte (test de coupures ignoré) et recovery validé. Relever
sur la carte les timings de mount/self-check/commit, le heap/pile et les éventuels
resets. Les mesures d'usure hôte accompagnent la conception mais ne donnent pas
une durée de vie physique sans l'endurance de la puce et la fréquence métier.
#68 reste ouverte jusqu'au bilan matériel prévu ; #70 reste ouverte séparément.
