# Issue #68 — audit de persistance ConfigSpace

État : premier incrément préparatoire. Référence auditée : rust-mcu-libs
b346a209f4d3903d9eddbbc914de714ac48db683, après les corrections a85db0c.
Cette PR dépend de #65 ; elle ne fusionne ni ne clôture ce chantier OTA.
Aucun nouveau moteur sélectionné définitivement, aucune migration flash exécutée.

## Responsabilités et consommateurs réels

| Élément | Situation observée | Action |
| --- | --- | --- |
| common/config/space | Manager, propriété des handles, quotas ; contrat ConfigBackend générique | Conserver API et comportement |
| common/fs/nvs | Codec CSM1 portable mêlé aux règles de noms/comptabilité NVS ; dépend de esp-nvs | Extraire le codec, conserver le reste provisoirement |
| arch/esp32/config/space | Générations, remplacement, tombstone, santé, namespace, découverte et accès NVS | Déplacer la persistance générique ; garder l'adaptation physique |
| arch/esp32/fs/nvs | Vue NvsFlash empruntée au SharedFlash existant | Garder pour lecture/migration et consommateurs identifiés |
| arch/esp32/flash | Unique propriétaire et verrou physique ; correction RDID S3 de #65 | Réutiliser ; aucune nouvelle instance FlashStorage |

Consommateurs de ConfigBackend dans le dépôt : Wi-Fi (128 octets), TLS
(6 154 octets), log policy (1 024 octets), métadonnées et bootstrap OTA (budget
2 048 dans ota-system), exemple config-manager (32 octets). Ces consommateurs
sont génériques ; seuls config-manager et ota-system composent concrètement le
backend NVS. Aucun autre consommateur direct de fs/nvs n'a été trouvé par la
recherche Rust/Cargo du dépôt. Cela n'inventorie pas les dépôts externes.

Les tables ota-system/ota-supervised réservent NVS 0x4000 octets (16 Kio).
L'admission actuelle réserve deux versions du blob avec la formule NVS :
2 * (ceil((13 + payload)/32) + ceil((13 + payload)/4000) + 1).
Elle donne 14 unités pour Wi-Fi, 392 pour TLS, 70 pour logs, 134 pour OTA,
8 pour demo. Ces unités sont des entrées NVS, pas des octets. La capacité
réelle dépend des statistiques/pages réservées ; ne pas promettre que tous
les budgets sont admis dans la table de 16 Kio.

## Premier incrément : codec unique et compatible

Déplacer exactement MAGIC CSM1, HEADER_LEN=13, FLAG_PRESENT, CorruptRecord et
encode_record/decode_record vers common/config/space::record. Garder les chemins
existants via réexport depuis common/fs/nvs ; déplacer les deux tests golden et
corruption avec le code de production. Aucun schéma ni format persistant modifié.
Les limites de noms à 15 octets restent spécifiques à la compatibilité NVS.

CSM1 ne possède ni longueur interne ni CRC propre. Son framing ne constitue pas
un moteur transactionnel : il doit être enveloppé dans un stockage assurant
intégrité, publication atomique, recovery et recyclage. Préserver la génération
après clear par un tombstone durable ; ne pas remplacer ce contrat par un delete
qui perdrait la dernière génération. Ne pas sauter les commits identiques pour
économiser l'usure sans décision explicite sur la sémantique des générations.

## Réutilisation amont : candidats à mesurer

Sources primaires consultées :
- [sequential-storage](https://github.com/tweedegolf/sequential-storage),
  [documentation](https://docs.rs/sequential-storage/latest/sequential_storage/).
- [ekv](https://github.com/embassy-rs/ekv), source main
  a0e2c7e16cf790e2fa7e34fac9f9dac1557824aa (2026-05-28),
  [version publiée 1.0.0](https://docs.rs/ekv/1.0.0/ekv/),
  [compatibilité de configuration](https://docs.rs/ekv/1.0.0/ekv/config/).

| Candidat | Intérêt | Limite à qualifier |
| --- | --- | --- |
| sequential-storage Map | Journal compact, CRC, rotation des pages ; interface embedded-storage | Dans l'API documentée, un item tient dans une page. Un TLS de 6 154 octets dépasse une page de 4 Kio. Une adaptation multi-pages demanderait une publication atomique supplémentaire ; ne pas la masquer |
| ekv | Transactions atomiques, valeurs multi-pages, CRC optionnel, rotation cyclique des pages | Réserve de compaction/metadata, amplification des petits commits, état/RAM et flash ; profil compilé fait partie du format |
| Journal maison | Ajustable aux quelques espaces | Ajouterait un moteur complexe alors que des briques existent ; non retenu avant preuve d'un écart bloquant |

ekv est un candidat prioritaire à expérimenter, pas encore la décision finale.
Tester la version publiée avant de dépendre de main : main et 1.0.0 ne sont pas
interchangeables par hypothèse. Le profil proposé pour expérimentation doit
admettre CSM1 + 6 154 octets, CRC activé, et une marge de compaction mesurée.
Les paramètres de format doivent être identifiés dans un superbloc/version de
composition et vérifiés avant montage ; la documentation ekv prévient que leurs
changements ne sont pas détectés automatiquement au montage.

La configuration main étudiée définit MIN_FREE_PAGE_COUNT = 1 + scratch + 1
+ branching_factor + ceil(max_record_size/PAGE_MAX_PAYLOAD_SIZE). Une simple
réduction des réserves peut augmenter l'amplification. Les 4 secteurs actuels
ne doivent pas être supposés suffisants pour ce candidat. Toute hausse de
partition doit être chiffrée et explicitement décidée.

Réutiliser les traits embedded-storage/embedded-storage-async quand adéquats.
Si un candidat impose un trait spécifique (ekv Flash), l'intégration doit rester
concrète et bornée. Ne pas prétendre qu'une NOR avec programmation 256 octets,
une EEPROM et une FRAM ont les mêmes garanties. Les géométries non supportées
doivent être refusées avant toute écriture ; pas de read-erase-modify-write caché.

## Usure : critères obligatoires

Le banc de stockage persistant doit compter, par secteur, les effacements,
les opérations de programmation, octets programmés et octets utiles demandés.
Conserver les compteurs du banc à travers les réouvertures ; ils ne sont pas
des métadonnées persistées par le produit.

Comparer NVS actuel et candidat sur les mêmes séquences :
- petits commits OTA répétés, valeurs variables et identiques ;
- Wi-Fi/logs et valeurs TLS maximales ;
- espaces chauds modifiés avec espaces froids inchangés ;
- clear/recommit, saturation et compactions multiples ;
- boots répétés et coupures pendant programmation/effacement/compaction.

Publier nombres totaux, histogramme par secteur, min/max/moyenne, coût maximal
d'un commit, RAM et occupation flash. Amplification = octets programmés / octets
utiles (rapporter les suppressions séparément). Coût d'usure = effacements par
commit et par secteur. Examiner le wear leveling dynamique et l'impact des
pages froides ; la seule rotation annoncée par un moteur ne prouve pas une
répartition uniforme sur tous les secteurs physiques.

La durée de vie dépend de l'endurance E_i de la puce réelle et du taux mesuré
r_i d'effacements par jour : limite estimée au secteur le plus sollicité,
min_i((E_i - usure_initiale_i) / r_i), pour r_i > 0. Aucune valeur d'endurance,
fréquence métier ou durée en années ne doit être inventée. Les coupures peuvent
augmenter le coût de recovery : inclure cet effet dans les mesures.

## Compatibilité et migration : choix à résoudre

Le montage doit distinguer vierge, format reconnu, ancien NVS, format inconnu
et corruption. Aucun de ces échecs ne déclenche un reformatage automatique.

Option à étudier : région ConfigSpace distincte, bornée et validée, ancienne
NVS conservée pendant l'import. Lister les clés cfg_space, décoder les records
CSM1, préserver générations et tombstones ; publier un marqueur de migration
seulement après copie vérifiée, rendre chaque interruption reprenable et tester
source corrompue/destination saturée. Lire une NVS corrompue peut échouer ou
paniquer avec esp-nvs 0.5 : définir la conduite à tenir sans perte silencieuse.
Les dumps/reproducteurs de #68 ne sont pas présents dans cette PR ; les obtenir
avant de prétendre rejouer ces scénarios précis.

Retour à un ancien firmware : celui-ci continuerait à voir l'ancienne NVS.
Une copie ponctuelle ne synchronise pas les commits ultérieurs. OTM1 partagé
avec le fallback OTA rend ce point concret : décider compatibilité/transition
avant bascule. Pas de double écriture permanente, pas de migration en place
supposée sûre sans espace temporaire et preuve de reprise.

## Incréments suivants et gates

1. Extraction CSM1 + audit, sans changement du backend de production.
2. Expérimentation du candidat avec simulateur persistant, tailles réelles,
   saturation/compaction, faults byte/bit/opération et compteurs d'usure.
3. Décision du profil/format, admission garantie et stratégie de migration.
4. Backend ConfigSpace commun, adaptateur ESP au SharedFlash, puis migration
   et qualification carte des scénarios OTM1 qui ont échoué dans #61.
5. Retrait des seules couches devenues sans consommateur ; autres ports
   matériels hors périmètre.

## État des validations de ce premier incrément

Les corps du codec et les deux tests sont déplacés sans modification ; la
comparaison de sources et git diff --check sont exécutables localement.
Le compilateur Rust est absent de l'environnement de reprise ; le téléchargement
rustup de static.rust-lang.org a été bloqué par la politique réseau. Aucune
exécution Rust locale, mesure d'usure, injection de fautes ou qualification
matérielle n'est annoncée PASS à ce stade. Les gates Rust/CI restent obligatoires.
