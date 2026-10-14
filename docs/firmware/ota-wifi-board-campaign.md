# Campagne carte ESP32-S3 — Wi-Fi des étalons OTA (#72 / PR #74)

Ce document qualifie sur carte l'intégration de `common/net/wifi/manager` dans l'étalon `ota-supervised`. Il vérifie deux comportements :
- une connexion échouée est retentée sans panic ni reset ;
- une perte de lien est récupérée sans reboot.

La cause radio de `DisassociatedDueToInactivity` n'a pas été recherchée, et aucun défaut du gestionnaire n'est imputé.

**Verdict : `998a1be` échoue, `e94568a` passe sur les essais exécutés.**
- `998a1be`, révision demandée : **FAIL**. La pile principale déborde et la carte se fige après 1 à 7 connexions HTTPS.
- `e94568a`, même révision plus la correction : **PASS** sur les essais exécutés. Les limites sont détaillées plus bas.

## Identité

| Élément | Valeur |
| --- | --- |
| Révision qualifiée | `998a1be60953dd44487343c63b19bbc3b9a7793c`, puis la correction `e94568a` (même branche) |
| Carte | ESP32-S3-DevKitC-1 N16R8, flash ID 0x184068, 16 MiB, pont UART CH343 |
| Table | celle de #73 : `ota_0` 0x20000, `ota_1` 0x1a0000, `wl_meta` 0x320000, `workload_a/b` 0x330000/0x370000, `config` 0x3c0000 |
| Toolchain | `rustc 1.98.1-nightly (183f762d6 2026-09-08) (1.98.1.0)`, esptool 5.5.0 |
| Construction | `prepare.py --chip esp32s3 --scenario ota-supervised`, puis `build-component.py --variant a` et `--variant b`, avec les paramètres du banc (identifiants Wi-Fi, jeton et identité TLS hors dépôt) |
| `app.bin` de `998a1be` | `47810818284a1a9fd82f693906d56fedd7baa057b83ae0d920cf7ed6ea2df52e` |
| `app.bin` de `e94568a` | `16e3736fe216b110370f0fb29402909899f9687299c92961f1e16e2098b2577e` |
| B : `native-a.iwni` / `native-b.iwni` | `41150ac5…7b2e` / `20800fcb…ad8a`, identiques à #73 |
| Réseau | 2,4 GHz WPA2, un seul SSID servi par 3 à 7 bornes visibles selon le scan, canal 11, -40 à -54 dBm, DHCP en /24. SSID masqué `<ssid>` dans les traces |
| Banc | banc Web Serial de #73, **reset EN automatique sur échec Wi-Fi désactivé** pendant toute la campagne (trace `auto reset off`) |

**Installation.** L'image a été installée par OTA A depuis le firmware de #73 (`client.py … --activate`), sans effacement ni installation usine.
- La configuration a été sauvegardée avant installation. `config` (`ec282193…`) et `otadata` (`2512b620…`) ont été relues en lecture seule ; l'espace `wifi` y était absent.
- Les images de diagnostic ont dû être écrites en `0x20000` par le port série, car le défaut empêchait l'OTA.
- L'image corrigée a ensuite été installée par OTA A.

**Remarque.** Sur le S3, un reset EN s'affiche aussi `rst:0x1 (POWERON)`. Les boots EN sont donc identifiés par la ligne du banc `EN reset injected (<repère>)`. Ces resets ont deux origines :
- les repères de campagne ;
- l'opérateur, tracés `(manuel)`, 5 au total, tous identifiés ci-dessous.

## Défaut constaté sur `998a1be` et correction

| Étape | Observation |
| --- | --- |
| 1ᵉʳ boot après OTA | provisioning conforme. La 1ʳᵉ tentative échoue (`DisassociatedDueToInactivity`), puis `Wi-Fi bootstrap failed; retry in 1000 ms without reboot`, puis connexion, DHCP, `configuration committed generation=1` et `OTA ready` |
| Juste après | trois `/health` à 200, puis **carte figée** : plus aucun log, y compris les `alive` de B, et ni panic, ni reset, ni watchdog |
| Reproductions | blocage après 2, 6, 1 et 0 requête(s) supplémentaires. Au repos, sans requête, la carte reste vivante 90 s. L'envoi OTA de l'image de diagnostic fige la carte deux fois |
| Sonde de pile (patch hors dépôt, `33d062bd…`) | pile réservée 56 760 o. **49 824 o utilisés dès `OTA ready`**, contre 27 808 o en #73. 50 744 o après la 1ʳᵉ requête, puis blocage à la 2ᵉ |
| Même code avec 16 Ko de tas transférés à la pile (`a1669841…`) | **30/30 `/health`**. Pic de **59 248 o**, supérieur aux 56 760 o de l'image nominale. La sonde attribue +11,5 Ko à la construction de `join(maintain, serveur)` et +4,8 Ko jusqu'au démarrage du serveur |

**Cause établie.** Composer `maintain()` et le serveur HTTPS par `join()` dans la tâche de boot fait déborder la pile principale du S3 pendant les handshakes TLS. Le débordement corrompt les données statiques situées sous la pile.

**Correction `e94568a`, sans nouvelle abstraction.** `maintain()` devient une tâche Embassy, dont l'état est statique. Elle transmet la pile réseau au serveur par un signal (`StaticCell`), et le corps du serveur redevient celui d'avant #74. `embassy-futures`, devenu inutile, est retiré.

Mesure sur l'image corrigée instrumentée (`9e44473b…`) :
- 28 096 o à `OTA ready` ;
- 34 640 o après 30 `/health` ;
- **47 344 o après un envoi et une activation B**, soit 9 352 o de marge (16 %), comme en #73 (47 280 o).

La faible marge déjà signalée dans #73 demeure.

**Contrôles locaux sur `e94568a`, tous réussis :**
- `cargo fmt --all -- --check` ;
- tests et Clippy `-D warnings` hôte d'`example-firmware-ota-system` et d'`example-firmware-ota-supervised` ;
- liens release C3 (`ota-system`) et S3 (`ota-system`, `ota-supervised`) par `.github/scripts/compile_mcu.py`, sans identifiants.

## Résultats sur l'image corrigée (`16e3736f…`)

| # | Essai | Verdict | Faits |
| --- | --- | --- | --- |
| 0 | Espace `wifi` dans le ConfigSpace existant | **PASS** | Manifeste `ota` + `wifi`. Après provisioning, la config relue (`e687c2b9…`, conservée hors dépôt car elle contient le mot de passe) contient la clé `wifi` et un enregistrement `WFC1`, SSID de 11 caractères. `ota`/OTM est conservé, sans effacement ni formatage |
| 1 | Premier démarrage : provisioning, persistance, HTTPS | **PASS**, observé sur `998a1be` | Voir la 1ʳᵉ ligne du tableau précédent. Le commit n'a lieu qu'après connexion et DHCP. Le chemin de provisioning n'est pas modifié par la correction. **Non rejoué** sur `e94568a`, car il aurait fallu effacer l'espace `wifi` |
| 1b | Boots suivants avec les identifiants persistés | **PASS** | Plus de `bootstrap` : la 1ʳᵉ ligne est `wifi: connect attempt 1`. Les variables de build ne sont plus utilisées |
| 2 | 20 démarrages, resets EN du banc `C3-1`…`C3-20` | **PASS 20/20** | 17 boots : 1 échec `DisassociatedDueToInactivity` puis succès, DHCP en 9–10 s. 2 boots : succès immédiat, DHCP en 5 s. 1 boot : `DisassociatedDueToInactivity` puis `NoAccessPointFound` puis succès, DHCP en 15,4 s. `/health` à 200 en 12–19 s après le reset. Aucun reset de rattrapage, aucun panic, B relancé à chaque boot |
| 2b | Démarrages à froid (USB) | **non exécuté** | Non demandé à l'opérateur. Seuls des resets EN ont été pratiqués |
| 3 | AP indisponible au démarrage | **PASS** (variante blocage MAC) | Le Wi-Fi ne pouvait pas être coupé ; l'ESP a été bloqué sur le routeur. Après un reset EN manuel pendant le blocage (17:57:00) : 5 refus `AuthenticationFailed`, espacés de 1, 2, 4, 8 puis 10 s, puis connexion à la 6ᵉ tentative. Après déblocage : connexion, DHCP et `OTA ready` à 17:57:48, `/health` à 200 à 17:57:52, **sans reset** |
| 4 | Pertes de lien après connexion | **PASS sur 1 cycle complet** ; 2 autres cycles avec reconnexion vue mais HTTPS non vérifié | 17:54:29 : déconnexion opérateur, `link down`, reconnexion à la 1ʳᵉ tentative en 2,4 s. 17:54:49 : blocage de 35 s, 5 refus, reconnexion. 17:58:58 : blocage de 68 s, 7 refus, `ready after 7 failed attempt(s)` à 18:00:06, **`/health` revenu à 200 à 18:00:07**. Aucun reboot : B tourne en continu. Les sondes HTTPS des deux premiers cycles n'étaient pas actives. Arrêté à la demande de l'opérateur ; le gestionnaire avait déjà été validé à son implémentation |
| 5 | Non-régression B après reconnexion | **PASS** | Après la reconnexion de 18:00:06 : statut B `Valid`, `Healthy`, B en marche. Envoi nominal `c74-b2` (`native-b`) : `staged`, `pending`, `confirmed` 5 s plus tard, sans reboot |
| — | Aucun panic `STA join failed` ni reset automatique | **PASS** | 0 occurrence dans la capture. Sur l'image corrigée, les seuls resets sont les 20 EN du banc, le reboot d'activation OTA et les 2 resets manuels de l'essai 3 (17:56:18, 17:57:00). Les autres resets portent sur le firmware de #73 ou sur `998a1be` figée : resets manuels de 17:03:44, 17:10:12 et 17:19:45, resets de diagnostic du banc D1, D3, D4 et D5 |

## Limites

- **`DisassociatedDueToInactivity` à la 1ʳᵉ tentative** sur 18 boots sur 20. La cause radio reste inconnue et n'a pas été recherchée. Le gestionnaire la rattrape dans le même boot, en environ 4 à 5 s de plus.
- **Blocage MAC au lieu d'une coupure de l'AP.** Le routeur répond `AuthenticationFailed`, et le transport l'affiche `refused the credentials (check the passphrase)`. Ce message est trompeur dans ce cas, mais il n'a aucun effet de bord : les identifiants enregistrés sont conservés et les tentatives continuent.
- **Démarrages à froid non exécutés.**
- **Une seule perte de lien** a son retour HTTPS mesuré.
- **Marge de pile à 16 %** sur S3 après un envoi B : déjà signalée dans #73.
- **Retour en arrière impossible vers un firmware antérieur à #74 après provisioning, déduit du code et non essayé.** Le contrôle `Store::validate` refuse les clés hors manifeste, donc un firmware sans espace `wifi` refuserait de monter la config.
- **#70 (EKV sous coupure) n'a pas été retesté.**

## Traces (hors dépôt, `/workspace/qual74/`, SSID masqué)

- `logs/serial-C74-campaign.log` : capture complète du banc ;
- `logs/C1-first-boot-and-hang.log`, `D2-…`, `D3-…`, `D4-…`, `D5-…` : blocages sur `998a1be` ;
- `logs/D6-mem-probe-hang.log` et `D7-stack16k-30-health.log` : mesures de pile ;
- `logs/F1-fix-mem-probe.log` : image corrigée instrumentée ;
- `logs/C3-boots20.log` et `C3-boots-*-client.log` : les 20 boots ;
- `logs/C4-user-ap-block.raw.log`, `C5-link-cycles.log` et `C5-healthpoll.log` : essais 3 et 4 ;
- `artifacts/build*/IDENTITY.txt` : empreintes ;
- `artifacts/qual-mem-74*.patch` et `fix-plus-qual-mem.patch` : sondes, jamais fusionnées ;
- `dumps/P0-*` : sauvegarde avant installation ;
- `dumps/C2-config-*` : relecture après provisioning, **sensible**.
