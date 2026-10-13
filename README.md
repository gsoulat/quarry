# Quarry

Client de bases de données léger et multiplateforme (macOS, Linux, Windows), écrit en Rust avec [iced](https://iced.rs).

## Fonctionnalités

- Connexion par URL à **PostgreSQL**, **MySQL** et **SQLite**
- Liste des tables et **schéma** de chaque table (types, NULL, valeurs par défaut, clé primaire)
- Éditeur SQL et grille de résultats défilable (limitée à 5 000 lignes)
- **Plan d'exécution avant exécution** (`EXPLAIN`, sans `ANALYZE`) : arbre du plan, ordre d'exécution des étapes et coût estimé quand le moteur le fournit

| Moteur | Plan | Coût estimé |
|---|---|---|
| PostgreSQL | `EXPLAIN (FORMAT JSON)` | oui |
| MySQL ≥ 8.0.18 | `EXPLAIN FORMAT=TREE` | oui |
| SQLite | `EXPLAIN QUERY PLAN` | non |

Chaque moteur a sa propre échelle de coût : les valeurs ne sont pas comparables d'un moteur à l'autre.

## Lancer

```bash
cargo run --release
```

Exemples d'URL :

```text
postgres://user:motdepasse@localhost:5432/ma_base
mysql://user:motdepasse@localhost:3306/ma_base
sqlite://chemin/vers/fichier.db?mode=rwc
```

## Architecture

```text
src/
├── main.rs              point d'entrée iced
├── app.rs               état, messages et vues
├── theme.rs             palette et styles
└── connector/
    ├── mod.rs           trait Connector commun à tous les moteurs
    ├── sql.rs           PostgreSQL / MySQL / SQLite via sqlx
    └── plan.rs          lecture des différents formats d'EXPLAIN
```

Pour ajouter un moteur (BigQuery, MongoDB/DocumentDB…), il suffit d'implémenter le trait `Connector` dans un nouveau module et de l'ajouter à `connector::connect`.

## Feuille de route

- [ ] Ordre d'exécution du plan (`connector::plan::execution_order`)
- [ ] Connecteur BigQuery
- [ ] Connecteur MongoDB / DocumentDB
- [ ] Grille virtualisée pour les gros résultats
- [ ] Coloration syntaxique SQL et raccourci Ctrl/Cmd+Entrée
- [ ] Connexions enregistrées (mots de passe dans le trousseau du système)
