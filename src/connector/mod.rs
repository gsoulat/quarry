//! Couche d'accès aux bases : un trait commun que chaque moteur implémente.
//!
//! L'interface ne connaît que [`Connector`] ; ajouter un moteur (BigQuery,
//! MongoDB…) revient à écrire un nouveau module qui implémente ce trait.

mod plan;
mod sql;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

pub use plan::{QueryPlan, execution_order};

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

/// Ouvre une connexion en choisissant le moteur d'après le schéma de l'URL.
pub async fn connect(url: &str) -> Result<Arc<dyn Connector>> {
    let scheme = url.split("://").next().unwrap_or_default();
    match scheme {
        "postgres" | "postgresql" | "mysql" | "sqlite" => {
            let connector = sql::SqlConnector::connect(url).await?;
            Ok(Arc::new(connector))
        }
        "" => Err("URL de connexion vide".into()),
        other => Err(format!("Moteur non supporté : « {other} »")),
    }
}
