# Qualification matérielle du démarrage vérifié (protocole proposé)

**Rien de ce document n'a été exécuté.** Il se lit avec `geometry-reprovisioning.md` : le second
étage vérifié du S3 s'installe avec la table à `0x10000`, donc après un reprovisionnement contrôlé du banc
(table, `otadata`, `ota_meta` et applications recompilées pour ce décalage). Les adresses des secteurs de
preuve (`0x1cf000`, `0x37f000`) ne changent pas. Chaque étape exige une autorisation distincte, arrêt à la
première divergence. Le second étage S3 vérifié (34 352 octets) ne tient pas devant une table à `0x8000` : il n'a de sens
qu'avec la géométrie large (`flash-geometry.md`).

La restauration d'un bootloader expérimental n'est pas considérée comme garantie tant que H2 n'a pas été
démontrée sur le banc.

## H0 : état de départ (lecture seule)

Identité (MAC, 16 Mio), Ns2 confirmé et Stable dans `ota_1`, `/health` 200 TLS vérifié, régions égales à
`p90-post-s1` (table et bootloader, `otadata`, `ota_meta`, `config`, les deux slots).

## H1 : sauvegardes

Bootloader (`0x0`–`0x8000`) et table (`0x8000`), `otadata`, `ota_meta`, `config`, les deux slots entiers, chacun
avec son SHA-256, en lecture seule (`0400`). Le firmware de repli (Ns2 confirmé) n'est jamais écrasé.

## H2 : démontrer la restauration avant de risquer quoi que ce soit

1. Entrer en mode téléchargement ROM par le banc (EN et IO0 par le pont UART) : cet accès est matériel et ne
   dépend d'aucun contenu de la flash.
2. Réécrire à l'identique le bootloader actuel à `0x0` ; relire ; vérifier le démarrage de Ns2 (`ota_1 Stable`).
3. Démontrer le cas dégradé : effacer uniquement `0x0`–`0x8000`, constater que la carte ne démarre plus,
   revenir en mode téléchargement, réécrire la sauvegarde, vérifier le démarrage.

Si 3 échoue, tout s'arrête : aucun bootloader expérimental ne sera installé.

## H3 : provisionner les preuves avant le bootloader vérifié

Avec `iap1_pack` : une clé de banc RSA-3072 dédiée, les enveloppes des deux images (Ns et Ns2 de #90), et
les secteurs `.iap1`. Écrire chaque secteur au `proof_offset` de son slot (dernier secteur : `0x1cf000` pour
`ota_0`, `0x37f000` pour `ota_1` sur le banc) par le banc, accès complet requis. Vérifier par relecture et
par `iap1_pack inspect`. Ces écritures ne touchent ni image, ni `otadata`, ni `ota_meta`. Les deux slots
doivent avoir leur preuve **avant** H4.

## H4 : premier démarrage du bootloader vérifié

Installer le bootloader vérifié (clé épinglée = celle de H3) à `0x0` ; un seul reset. Attendus : journal
`boot authentication: VERIFIED`, Ns2 démarre en `ota_1 Stable`, `/health` 200, `otadata` et `ota_meta`
inchangés. Relever la durée de démarrage et la pile (remplissage par motif).

## H5 : refus (un à la fois, restauration entre deux)

| Cas | Attendu |
|---|---|
| un octet du slot confirmé modifié | refusé, repli sur l'autre slot s'il est authentique, sinon HALT sans écriture |
| secteur de preuve effacé | idem, motif `ProofAbsent` |
| mot de validation effacé | motif `ProofTruncated` |
| preuve d'une clé non épinglée | motif `UnknownKey` |
| lecture en erreur (non reproductible sur carte : couvert sur hôte) | — |

Aucun refus ne doit être écrit dans `otadata` quand le résultat est un HALT.

## H6 : retour

Réécrire le bootloader de H1 (et lui seul), vérifier le démarrage de Ns2. Les secteurs de preuve restent
sans effet avec le bootloader non signé.

## Hors périmètre

ESP32-C3 matériel, Secure Boot V2 et eFuses (protocole propre, carte sacrifiée : le fusible est
irréversible), anti-rollback (#92), écriture de la preuve à l'activation OTA (à câbler avant tout usage
de terrain).
