//! Couche d'accès aux bases : un trait commun que chaque moteur implémente.
//!
//! L'interface ne connaît que [`Connector`] ; ajouter un moteur (MongoDB…)
//! revient à écrire un nouveau module qui implémente ce trait.

#[cfg(feature = "bigquery")]
mod bigquery;
mod plan;
mod sql;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

pub use plan::{QueryPlan, execution_order};

use crate::store::{ConnectionConfig, Engine};

/// Au-delà, on arrête de lire : la grille reste fluide et la mémoire basse.
pub const MAX_ROWS: usize = 5_000;

pub type Result<T> = std::result::Result<T, String>;

/// Une table (ou vue), éventuellement qualifiée par son schéma.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

impl fmt::Display for TableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.schema {
            Some(schema) => write!(f, "{schema}.{}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// Description d'une colonne, pour la vue « Schéma ».
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
}

/// Valeur d'une cellule, indépendante du moteur.
#[derive(Debug, Clone)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    /// Type que l'on ne sait pas encore décoder : on garde son nom.
    Unsupported(String),
}

impl Cell {
    /// Texte affiché dans la grille de résultats.
    pub fn display(&self) -> String {
        const MAX_CHARS: usize = 200;
        match self {
            Cell::Null => "NULL".into(),
            Cell::Bool(b) => b.to_string(),
            Cell::Int(i) => i.to_string(),
            Cell::Float(f) => f.to_string(),
            Cell::Text(s) if s.chars().count() > MAX_CHARS => {
                let truncated: String = s.chars().take(MAX_CHARS).collect();
                format!("{truncated}…")
            }
            Cell::Text(s) => s.clone(),
            Cell::Unsupported(type_name) => format!("<{type_name}>"),
        }
    }
}

/// Une clé étrangère : `from.from_columns` référence `to.to_columns`.
#[derive(Debug, Clone)]
pub struct Relation {
    pub from: TableRef,
    pub from_columns: Vec<String>,
    pub to: TableRef,
    pub to_columns: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

/// Valeur saisie dans un formulaire de ligne.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Null,
    Default,
    Text(String),
}

/// Colonne d'une table à créer.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

/// Modification de structure d'une table existante.
#[derive(Debug, Clone)]
pub enum Alter {
    AddColumn(ColumnDef),
    DropColumn(String),
    RenameColumn { from: String, to: String },
    RenameTable(String),
}

#[async_trait]
pub trait Connector: Send + Sync + fmt::Debug {
    /// Nom lisible du moteur (« PostgreSQL », « SQLite »…).
    fn engine(&self) -> &'static str;

    /// Protège un identifiant (nom de table, de colonne) pour ce moteur.
    fn quote_identifier(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    /// Nom de table qualifié et protégé.
    fn quote_table(&self, table: &TableRef) -> String {
        match &table.schema {
            Some(schema) => {
                format!("{}.{}", self.quote_identifier(schema), self.quote_identifier(&table.name))
            }
            None => self.quote_identifier(&table.name),
        }
    }

    /// Littéral texte : les apostrophes sont doublées (SQL standard).
    fn quote_literal(&self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    /// Littéral d'une valeur lue dans la grille (pour cibler une ligne).
    fn cell_literal(&self, cell: &Cell) -> String {
        match cell {
            Cell::Null => "NULL".into(),
            Cell::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Cell::Int(i) => i.to_string(),
            Cell::Float(f) => f.to_string(),
            Cell::Text(s) => self.quote_literal(s),
            Cell::Unsupported(_) => "NULL".into(),
        }
    }

    fn input_literal(&self, input: &Input) -> String {
        match input {
            Input::Null => "NULL".into(),
            Input::Default => "DEFAULT".into(),
            Input::Text(s) => self.quote_literal(s),
        }
    }

    /// Clause `WHERE` qui cible une ligne par sa clé.
    fn where_key(&self, key: &[(String, Cell)]) -> String {
        key.iter()
            .map(|(column, value)| {
                let column = self.quote_identifier(column);
                match value {
                    Cell::Null => format!("{column} IS NULL"),
                    other => format!("{column} = {}", self.cell_literal(other)),
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// Requête d'aperçu proposée quand on clique sur une table.
    fn preview_sql(&self, table: &TableRef) -> String {
        format!("SELECT * FROM {} LIMIT 100", self.quote_table(table))
    }

    /// Toutes les lignes de la table (bornées par `MAX_ROWS` à la lecture).
    fn data_sql(&self, table: &TableRef) -> String {
        format!("SELECT * FROM {} LIMIT {MAX_ROWS}", self.quote_table(table))
    }

    fn insert_sql(&self, table: &TableRef, values: &[(String, Input)]) -> String {
        let columns: Vec<_> = values.iter().map(|(c, _)| self.quote_identifier(c)).collect();
        let literals: Vec<_> = values.iter().map(|(_, v)| self.input_literal(v)).collect();
        format!(
            "INSERT INTO {} ({})\nVALUES ({})",
            self.quote_table(table),
            columns.join(", "),
            literals.join(", ")
        )
    }

    /// INSERT multi-lignes (import par lots).
    fn insert_many_sql(&self, table: &TableRef, columns: &[String], rows: &[Vec<Input>]) -> String {
        let columns: Vec<_> = columns.iter().map(|c| self.quote_identifier(c)).collect();
        let values: Vec<String> = rows
            .iter()
            .map(|row| format!("({})", row.iter().map(|v| self.input_literal(v)).collect::<Vec<_>>().join(", ")))
            .collect();
        format!(
            "INSERT INTO {} ({})\nVALUES\n{}",
            self.quote_table(table),
            columns.join(", "),
            values.join(",\n")
        )
    }

    fn update_sql(&self, table: &TableRef, set: &[(String, Input)], key: &[(String, Cell)]) -> String {
        let assignments: Vec<_> = set
            .iter()
            .map(|(c, v)| format!("{} = {}", self.quote_identifier(c), self.input_literal(v)))
            .collect();
        format!(
            "UPDATE {}\nSET {}\nWHERE {}",
            self.quote_table(table),
            assignments.join(",\n    "),
            self.where_key(key)
        )
    }

    fn delete_sql(&self, table: &TableRef, key: &[(String, Cell)]) -> String {
        format!("DELETE FROM {}\nWHERE {}", self.quote_table(table), self.where_key(key))
    }

    fn column_ddl(&self, column: &ColumnDef) -> String {
        let mut ddl = format!("{} {}", self.quote_identifier(&column.name), column.data_type.trim());
        if !column.nullable || column.primary_key {
            ddl.push_str(" NOT NULL");
        }
        ddl
    }

    fn create_table_sql(&self, table: &TableRef, columns: &[ColumnDef]) -> String {
        let mut lines: Vec<_> = columns.iter().map(|c| format!("    {}", self.column_ddl(c))).collect();
        let pk: Vec<_> = columns.iter().filter(|c| c.primary_key).map(|c| self.quote_identifier(&c.name)).collect();
        if !pk.is_empty() {
            lines.push(format!("    PRIMARY KEY ({})", pk.join(", ")));
        }
        format!("CREATE TABLE {} (\n{}\n)", self.quote_table(table), lines.join(",\n"))
    }

    /// `None` si le moteur n'a pas de schémas (SQLite).
    fn create_schema_sql(&self, name: &str) -> Option<String> {
        Some(format!("CREATE SCHEMA {}", self.quote_identifier(name)))
    }

    /// Refuse de supprimer un schéma non vide (pas de CASCADE) : c'est volontaire.
    fn drop_schema_sql(&self, name: &str) -> Option<String> {
        Some(format!("DROP SCHEMA {}", self.quote_identifier(name)))
    }

    fn drop_table_sql(&self, table: &TableRef) -> String {
        format!("DROP TABLE {}", self.quote_table(table))
    }

    fn alter_table_sql(&self, table: &TableRef, alter: &Alter) -> String {
        let table = self.quote_table(table);
        match alter {
            Alter::AddColumn(column) => format!("ALTER TABLE {table}\nADD COLUMN {}", self.column_ddl(column)),
            Alter::DropColumn(name) => format!("ALTER TABLE {table}\nDROP COLUMN {}", self.quote_identifier(name)),
            Alter::RenameColumn { from, to } => format!(
                "ALTER TABLE {table}\nRENAME COLUMN {} TO {}",
                self.quote_identifier(from),
                self.quote_identifier(to)
            ),
            Alter::RenameTable(to) => format!("ALTER TABLE {table}\nRENAME TO {}", self.quote_identifier(to)),
        }
    }

    async fn list_tables(&self) -> Result<Vec<TableRef>>;

    /// Schémas (ou datasets) existants, même vides. Vide si le moteur n'en a pas.
    async fn list_schemas(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn describe_table(&self, table: &TableRef) -> Result<Vec<ColumnInfo>>;

    /// Toutes les clés étrangères de la base, pour le diagramme.
    async fn list_relations(&self) -> Result<Vec<Relation>>;

    /// Plan d'exécution estimé, **sans** exécuter la requête.
    async fn explain(&self, sql: &str) -> Result<QueryPlan>;

    async fn query(&self, sql: &str) -> Result<QueryResult>;

    /// Exécute une instruction sans résultat (INSERT, UPDATE, DDL…) et
    /// renvoie le nombre de lignes touchées quand le moteur le connaît.
    async fn execute(&self, sql: &str) -> Result<u64>;
}

/// Ouvre une connexion enregistrée, avec son mot de passe éventuel.
pub async fn connect(
    config: &ConnectionConfig,
    password: Option<String>,
) -> Result<Arc<dyn Connector>> {
    match config.engine {
        Engine::Postgres | Engine::MySql | Engine::Sqlite => {
            Ok(Arc::new(sql::SqlConnector::connect(config, password.as_deref()).await?))
        }
        #[cfg(feature = "bigquery")]
        Engine::BigQuery => Ok(Arc::new(bigquery::BigQueryConnector::connect(config).await?)),
        #[cfg(not(feature = "bigquery"))]
        Engine::BigQuery => Err("Quarry a été compilé sans la fonctionnalité « bigquery »".into()),
    }
}
