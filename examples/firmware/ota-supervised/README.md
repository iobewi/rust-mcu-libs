# OTA système A et composant natif supervisé B

Composition exécutable **ESP32-S3**, encore à qualifier sur carte. A utilise le
même source de boot et de routes que `../ota-system`. B utilise le moteur
`ArtifactWriteSession`, le même `SharedFlash`, le codec/politique OTM2 existant
et le runtime natif historique de `iobewi_old@345378ed` (IWNI v1, ABI v1).
A reste disponible si B manque, est corrompu, refuse de démarrer ou échoue.

## Composition

- Table : A/config/otadata inchangés ; B ajoute `wl_meta`, `workload_a`,
  `workload_b` (deux slots de 256 Kio). Découverte réelle et table complète validée.
- OTM2 : `NorMetadata` emprunte le propriétaire flash verrouillé pendant chaque
  transaction synchrone. Deux copies dans des secteurs distincts, effacement,
  vérification vierge, programmation des 180 octets et relecture. Le store
  historique choisit la copie opposée à la dernière valide. Aucun formatage
  automatique sur erreur. Aucun accès ConfigSpace dans ce verrou.
- Un owner B serialise upload, activation, stop et observation de santé. La
  réservation protège le slot réellement exécuté jusqu'à la publication.
  Supersession durable après réservation et avant erase ; digest du flux puis
  relu en flash et gate IWNI avant `Staged`.
- Activation : digest relu et preflight sans effets → intention Activating →
  arrêt réel du précédent → chargement et démarrage du candidat →
  PendingConfirmation. Confirmation après 5 s de santé (la fenêtre sans progrès
  du runtime est 3 s). Échec → RollingBack → arrêt réel → relecture/chargement du
  précédent → Valid ou Empty. Échec de restauration laisse RollingBack.
- Au reboot, Valid ou le précédent d'un Staged redémarre après relecture du
  digest. Une activation/confirmation/rollback inachevée entraîne un rollback.
  Métadonnées non reconnues : B arrêté, A disponible, sans état inventé.
- IWNI charge du code réellement compilé sur APP_CPU avec pile 8 Kio. Région
  historique `0x3FCE3700..0x3FCEB700` réservée par le linker, assertions contre
  IRAM et RAM reclaimed A. Logs bornés, arrêt coopératif puis park forcé,
  progression et quarantaine repris du runtime historique. Flash multicore
  auto-park existant conservé.
- Garde anti-boucle de démarrage : record RTC checksummé historique, suppression
  d'auto-start après démarrages non propres répétés ; preuve de santé continue
  30 s pour remettre le compteur à zéro. Les reboots volontaires A marquent
  clean. La suppression n'efface pas OTM2 et permet une nouvelle activation.

**Code natif de confiance, sans isolation mémoire.** Le digest assure
l'intégrité, pas l'authenticité. L'opérateur authentifié choisit les images de
confiance ; une séparation de partitions ne sandboxe pas le composant.
C3 n'a pas de chargeur natif ici : son étalon système A reste séparé.

## Préparer et installer

Utiliser les outils/variables TLS et Wi-Fi du README `ota-system`. Depuis la
racine, charger `export-esp.sh` et préparer sans contacter la carte :

```sh
python3 examples/firmware/ota-system/prepare.py --chip esp32s3 \
  --scenario ota-supervised \
  --partition-tool /absolute/path/esp-idf/components/partition_table/gen_esp32part.py
python3 examples/firmware/ota-supervised/build-component.py --variant a
python3 examples/firmware/ota-supervised/build-component.py --variant b
```

`prepare.py` produit dans `target/ota-supervised/esp32s3` les images système et
les fichiers vierges config/wl_meta pour **installation usine volontaire
uniquement**. `CARGO_TARGET_DIR` est respecté. Le composant est un ELF séparé,
packagé par `iobewi-workload-pack` avec le même gate IWNI que le chargeur.
Les variantes `--fault panic`, `ignore-stop`, `log-flood` servent aux essais.

Première installation sur carte de développement, après sauvegarde utile :

```sh
python3 -m esptool --chip esp32s3 --port /dev/ttyUSB0 erase-flash
python3 -m esptool --chip esp32s3 --port /dev/ttyUSB0 write-flash \
  0x0 target/ota-supervised/esp32s3/bootloader.bin \
  0x8000 target/ota-supervised/esp32s3/partitions.bin \
  0xd000 target/ota-supervised/esp32s3/otadata.bin \
  0x20000 target/ota-supervised/esp32s3/app.bin \
  0x320000 target/ota-supervised/esp32s3/wl_meta.bin \
  0x3c0000 target/ota-supervised/esp32s3/config.bin
```

**Ne jamais reflasher ces métadonnées/config usine entre les OTA ou essais de
conservation.** Les premiers logs doivent indiquer A stable et serveur HTTPS
prêt, sans composant B en exécution.

## Routes B

Toutes exigent le même Bearer que l'étalon A. Le client conserve sa validation
TLS ; upload sans activation implicite :

```sh
python3 examples/firmware/ota-system/client.py https://device.local \
  target/ota-supervised/esp32s3/native-a.iwni hello-a --ca server-cert.pem \
  --domain component --version 1 --runtime-api 1.0 --activate
```

| Route | Contrat |
| --- | --- |
| POST `/workload/upload` | Content-Length, X-Deployment-ID (1–24 ASCII alphanumérique/-_.), X-Component-Version (1–16 octets sans NUL), X-Runtime-Api (MAJOR.MINOR), X-Image-SHA256 ; réponse staged après vérifications |
| POST `/workload/activate` | Body vide, X-Deployment-ID identique au candidat ; réponse pending ; **pas de reboot système** |
| GET `/workload/status` | Texte diagnostic : record OTM2, identité réellement exécutée, santé |
| POST `/workload/stop` | Body vide ; arrêt réel, sans effacer la sélection persistante (un reboot peut la redémarrer) ; pendant probation le monitor résout l'échec par rollback |

Les routes A `/ota/upload`, `/ota/activate`, `/health` sont celles de l'étalon
système. Pour une OTA A de cette composition, utiliser son `app.bin` avec
`client.py` sans `--domain component`. Une image A dépourvue de runtime B peut
préserver ses partitions mais ne l'exécutera pas.

## Campagne matérielle restante (#61)

Aucun essai carte B n'est déclaré PASS par le code ou la CI. Consigner commit,
SoC/capacité, toolchain, SHA256, commandes, logs et dumps avant/après.

1. Premier boot sans B : A/HTTPS disponibles, aucune sélection inventée.
2. Upload B seul → staged, reboot → staged conservé et aucun candidat lancé.
3. Activation explicite → logs réels hello-a, progression, Pending puis Valid ;
   A ne reboote pas. Installer hello-b dans l'autre slot, confirmer, revenir.
4. Pendant exécution, upload dans l'autre slot ; vérifier l'identité courante,
   les refus d'ID/API/header/digest, et l'absence de publication après transfert
   incomplet. Une supersession interrompue ne doit pas laisser l'ancien staged.
5. Candidat panic : rollback vers le précédent, A/HTTPS disponibles. Ignore-stop :
   arrêt réellement forcé lors du remplacement. Log-flood : drain borné et A
   disponible. Observer la garde anti-boucle sur la variante de crash pertinente.
6. Reboots entre intentions et effets OTM2, puis coupures ciblées sur `wl_meta`
   pendant erase/program : dernière copie récupérable ou état explicite, jamais
   reset/formatage silencieux. Ces essais B NOR ne valident pas les commits EKV.
7. Dumps avant/après OTA B : images A, otadata et config strictement inchangées ;
   seule la région B autorisée change. Rejouer A dans les deux sens avec B présent.
8. Mesurer heap/piles/temps de chargement et impact radio/flash. Les anciennes
   marges de l'exemple A seul ne sont pas transposables à ce nouveau binaire.

Le défaut EKV sous interruption (#70) reste différé ; ne pas relancer sa
campagne dans ce mandat. #72 Wi-Fi reste différée. Qualification C3 dans #71,
qualification réelle B et compléments A/B dans #61.
