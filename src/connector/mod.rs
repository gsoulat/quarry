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

#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

#[async_trait]
pub trait Connector: Send + Sync + fmt::Debug {
    /// Nom lisible du moteur (« PostgreSQL », « SQLite »…).
    fn engine(&self) -> &'static str;

    /// Protège un identifiant (nom de table, de colonne) pour ce moteur.
    fn quote_identifier(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    /// Requête d'aperçu proposée quand on clique sur une table.
    fn preview_sql(&self, table: &TableRef) -> String {
        let name = match &table.schema {
            Some(schema) => {
                format!("{}.{}", self.quote_identifier(schema), self.quote_identifier(&table.name))
            }
            None => self.quote_identifier(&table.name),
        };
        format!("SELECT * FROM {name} LIMIT 100")
    }

    async fn list_tables(&self) -> Result<Vec<TableRef>>;

    async fn describe_table(&self, table: &TableRef) -> Result<Vec<ColumnInfo>>;

    /// Plan d'exécution estimé, **sans** exécuter la requête.
    async fn explain(&self, sql: &str) -> Result<QueryPlan>;

    async fn query(&self, sql: &str) -> Result<QueryResult>;
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
