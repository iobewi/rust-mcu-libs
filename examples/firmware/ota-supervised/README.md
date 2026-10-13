# OTA système A et composant supervisé B

**Scénario retenu, exécutable encore à implémenter.** Le dossier contient la
spécification et sa table ; il n'ajoute pas de crate vide ou de fausse mise à
jour B. La réception/publication/activation A sera d'abord terminée dans
`../ota-system` et ses mécanismes seront partagés via les briques existantes.

La table conserve exactement NVS, `otadata`, `ota_0` et `ota_1` du premier
scénario. Elle ajoute `wl_meta`, `workload_a` et `workload_b`, conformément au
layout B historique : slots égaux de 256 Kio et métadonnées sur deux secteurs
d'effacement distincts. Cette extension n'est pas un second bootloader.

Le domaine B réutilise le moteur et le writer communs. L'exemple fournira un
composant concret et sa politique start/stop/health, le backend OTM2 et la
vérification du digest relu. A doit rester opérationnel si B manque ou échoue.

Critères de test :

- Tous les scénarios A du premier exemple restent valides.
- Première installation B, activation, confirmation et récupération après échec.
- Refus d'écriture sur un composant en exécution ; arrêt réel avant autorisation.
- Coupures lors des écritures et des phases OTM2 ; reprise déterministe.
- Une mise à jour B ne change ni les images A ni `otadata` ni la configuration.
- Même propriétaire physique `SharedFlash`, sans verrou flash pendant ConfigSpace.

L'absence d'isolation mémoire entre composants doit être traitée par leur
composition. La séparation des régions flash ne constitue pas cette isolation.
