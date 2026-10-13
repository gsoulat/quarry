# Quarry

Client de bases de données léger et multiplateforme (macOS, Linux, Windows), écrit en Rust avec [iced](https://iced.rs).

## Fonctionnalités

- **PostgreSQL**, **MySQL**, **SQLite** et **Google BigQuery**
- **Gestionnaire de connexions** (bouton « Gérer ») : ajouter, tester, modifier, supprimer, ranger dans des **dossiers**
- **Menu « Édition »** : créer / renommer / supprimer une table, ajouter / renommer / supprimer une colonne (types dans un menu déroulant par moteur), insérer / modifier / supprimer une ligne (onglet Données). Le SQL généré est **affiché et confirmé avant exécution**
- Barre latérale groupée en **Local / Distant / Cloud** : un clic pour se (re)connecter
- Mots de passe stockés dans le **trousseau du système** (Keychain, Gestionnaire d'identification Windows, Secret Service), jamais en clair sur le disque
- Tables groupées par schéma, filtre, barre latérale **redimensionnable**, bouton `+` pour **insérer le nom d'une table au curseur**
- **Schéma** de chaque table (types, NULL, valeurs par défaut, clé primaire)
- Éditeur SQL avec coloration, `⌘↩` exécute (la sélection seule si elle existe), `⌘E` explique
- Grille de résultats paginée (200 lignes par page, 5 000 max), colonnes ajustées au contenu, **clic sur une cellule pour copier**
- **Diagramme des tables** (onglet Diagramme) : clés étrangères dessinées sur un canvas, zoom et déplacement, boîtes déplaçables, placement automatique, trois niveaux **MPD / MLD / MCD**
- **Plan d'exécution avant exécution** (`EXPLAIN`, sans `ANALYZE`) : arbre du plan, ordre d'exécution des étapes et coût estimé quand le moteur le fournit

| Moteur | Plan | Coût estimé |
|---|---|---|
| PostgreSQL | `EXPLAIN (FORMAT JSON)` | oui |
| MySQL ≥ 8.0.18 | `EXPLAIN FORMAT=TREE` | oui |
| SQLite | `EXPLAIN QUERY PLAN` | non |
| BigQuery | dry run (gratuit) | oui, en $ (volume analysé × 6,25 $/Tio) |

Chaque moteur a sa propre échelle de coût : les valeurs ne sont pas comparables d'un moteur à l'autre.

## Lancer

```bash
cargo run --release
```

Les connexions sont enregistrées dans `connections.json`, dans le dossier de configuration du système (`~/Library/Application Support/fr.gsoulat.Quarry/` sur macOS).

Pour BigQuery, renseigne l'identifiant du projet et, au choix, le chemin d'une clé JSON de compte de service ou rien pour utiliser `gcloud auth application-default login`.

Pour compiler sans BigQuery (binaire plus léger) :

```bash
cargo build --release --no-default-features
```

Pour produire une vraie application (`Quarry.app`, `.msi`, `.deb`) avec son icône :

```bash
cargo install cargo-bundle && cargo bundle --release
```

L'icône est générée par `cargo run --release --example gen_icon -- assets/icon.png 1024`.

## Architecture

```text
src/
├── main.rs              point d'entrée iced
├── app.rs               état, messages et vues
├── theme.rs             palette et styles
├── manager.rs           fenêtre de gestion des connexions
├── store.rs             connexions enregistrées + trousseau
├── icon.rs              icône de fenêtre et du Dock
├── diagram.rs           diagramme des tables (canvas, placement, MPD/MLD/MCD)
├── editing.rs           formulaires ligne / table / colonne et confirmation du SQL
└── connector/
    ├── mod.rs           trait Connector commun à tous les moteurs
    ├── sql.rs           PostgreSQL / MySQL / SQLite via sqlx
    ├── bigquery.rs      Google BigQuery (API REST)
    └── plan.rs          lecture des différents formats d'EXPLAIN
```

Pour ajouter un moteur (MongoDB/DocumentDB…), il suffit d'implémenter le trait `Connector` dans un nouveau module et de l'ajouter à `connector::connect`.

## Feuille de route

- [ ] Ordre d'exécution du plan (`connector::plan::execution_order`)
- [x] Connecteur BigQuery
- [x] Connexions enregistrées (mots de passe dans le trousseau du système)
- [ ] Classement local / distant (`store::ConnectionConfig::location`)
- [ ] Connecteur MongoDB / DocumentDB
- [x] Diagramme des relations (MCD / MLD / MPD)
- [ ] Positions du diagramme mémorisées par connexion
- [ ] Onglets de requêtes enregistrables
- [ ] Connecteurs téléchargeables (processus séparés)
- [ ] Tri et filtre des résultats
- [x] Édition des lignes et des tables par l'interface
- [ ] Import de fichiers (CSV, Parquet, Excel) dans une table
- [ ] Thème clair
