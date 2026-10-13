# Exemples firmware étalons

Ces exemples assemblent les briques de `common/` et `arch/` pour tester sur carte
les chaînes de mise à jour. Le produit garde ses choix de transport, configuration,
self-check et récupération. Les exemples ne créent aucun second moteur OTA,
writer ou propriétaire physique de flash.

| Dossier | Scénario | État |
| --- | --- | --- |
| `bootloader/` | Second étage commun aux deux scénarios ESP | Porté ; lien/packaging/taille vérifiés C3/S3, qualification carte à faire |
| `ota-system/` | Chaîne complète du système A | Code de boot/confirmation existant déplacé ; réception/publication/activation depuis un transfert à raccorder |
| `ota-supervised/` | Même chaîne A et domaine supplémentaire B | Spécification et table de partitions ; exécutable à implémenter après A |

Ordre : terminer `ota-system`, qualifier la chaîne A, puis assembler A+B dans
`ota-supervised`. Le second exemple importe les mécanismes communs ; il ne
recopie pas le premier. B possède sa politique d'activation/récupération et ne
programme jamais `otadata`. Un domaine supplémentaire est logique : le profil
B historique utilise deux slots et deux copies de métadonnées dans des secteurs
distincts, pas simplement une partition exécutée par le bootloader A.

Chaque scénario porte sa table et sa procédure de test. Réussite, refus,
blocage et coupures sont des variantes du scénario, pas des implémentations
OTA différentes. Les tables proposées tiennent dans 4 Mio ; une future table
16 Mio ajoute des régions/domaines sans changer le moteur ou le writer.

Aucun scénario n'est qualifié fonctionnellement par les seuls tests hôte ou
builds. Les étapes incomplètes restent explicitement indiquées dans #61.
