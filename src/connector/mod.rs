//! Couche d'accès aux bases.
//!
//! Les types de données (tables, colonnes, cellules, plans…) viennent de
//! la crate `quarry-protocol`, partagée avec les connecteurs. Deux
//! implémentations du trait [`Connector`] :
//! - [`sqlite::Sqlite`], intégrée à l'application (aucun téléchargement) ;
//! - [`process::ProcessConnector`], qui pilote un connecteur téléchargé
//!   (PostgreSQL, MySQL, BigQuery…) lancé comme processus enfant.
//!
//! La génération du SQL d'édition (INSERT, CREATE TABLE…) est faite ici,
//! à partir du [`Dialect`] annoncé par le connecteur.

pub mod plan;
pub mod process;
pub mod sqlite;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

pub use plan::execution_order;
pub use quarry_protocol::{Cell, ColumnInfo, Dialect, PlanNode, QueryPlan, QueryResult, Relation, TableRef};

use crate::store::ConnectionConfig;

pub type Result<T> = std::result::Result<T, String>;

/// Au-delà, on arrête de lire : la grille reste fluide et la mémoire basse.
pub const MAX_ROWS: usize = 5_000;

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
    fn engine(&self) -> &str;

    /// Règles du dialecte, pour générer le SQL d'édition.
    fn dialect(&self) -> &Dialect;

    async fn list_schemas(&self) -> Result<Vec<String>>;
    async fn list_tables(&self) -> Result<Vec<TableRef>>;
    async fn describe_table(&self, table: &TableRef) -> Result<Vec<ColumnInfo>>;
    /// Toutes les clés étrangères de la base, pour le diagramme.
    async fn list_relations(&self) -> Result<Vec<Relation>>;
    /// Plan d'exécution estimé, **sans** exécuter la requête.
    async fn explain(&self, sql: &str) -> Result<QueryPlan>;
    async fn query(&self, sql: &str) -> Result<QueryResult>;
    /// Exécute une instruction sans résultat et renvoie les lignes touchées.
    async fn execute(&self, sql: &str) -> Result<u64>;

    // ------------------------------------------------------------------
    // Génération de SQL, d'après le dialecte
    // ------------------------------------------------------------------

    fn quote_identifier(&self, ident: &str) -> String {
        let q = self.dialect().identifier_quote;
        let doubled = format!("{q}{q}");
        format!("{q}{}{q}", ident.replace(q, &doubled))
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

    /// Littéral texte : apostrophes doublées, antislash doublé si le moteur
    /// l'interprète.
    fn quote_literal(&self, value: &str) -> String {
        let escaped = if self.dialect().backslash_escapes { value.replace('\\', "\\\\") } else { value.to_string() };
        format!("'{}'", escaped.replace('\'', "''"))
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
        format!("INSERT INTO {} ({})\nVALUES\n{}", self.quote_table(table), columns.join(", "), values.join(",\n"))
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
        let d = self.dialect();
        if !d.has_schemas && !d.schema_is_database {
            return None;
        }
        let keyword = if d.schema_is_database { "DATABASE" } else { "SCHEMA" };
        Some(format!("CREATE {keyword} {}", self.quote_identifier(name)))
    }

    /// Refuse de supprimer un schéma non vide (pas de CASCADE) : c'est voulu.
    fn drop_schema_sql(&self, name: &str) -> Option<String> {
        let d = self.dialect();
        if !d.has_schemas && !d.schema_is_database {
            return None;
        }
        let keyword = if d.schema_is_database { "DATABASE" } else { "SCHEMA" };
        Some(format!("DROP {keyword} {}", self.quote_identifier(name)))
    }

    fn drop_table_sql(&self, table: &TableRef) -> String {
        format!("DROP TABLE {}", self.quote_table(table))
    }

    fn alter_table_sql(&self, table: &TableRef, alter: &Alter) -> String {
        let quoted = self.quote_table(table);
        match alter {
            Alter::AddColumn(column) => format!("ALTER TABLE {quoted}\nADD COLUMN {}", self.column_ddl(column)),
            Alter::DropColumn(name) => format!("ALTER TABLE {quoted}\nDROP COLUMN {}", self.quote_identifier(name)),
            Alter::RenameColumn { from, to } => format!(
                "ALTER TABLE {quoted}\nRENAME COLUMN {} TO {}",
                self.quote_identifier(from),
                self.quote_identifier(to)
            ),
            Alter::RenameTable(to) if self.dialect().rename_table_statement => {
                format!("RENAME TABLE {quoted} TO {}", self.quote_identifier(to))
            }
            Alter::RenameTable(to) => format!("ALTER TABLE {quoted}\nRENAME TO {}", self.quote_identifier(to)),
        }
    }
}

/// Ouvre une connexion enregistrée, avec son mot de passe éventuel.
pub async fn connect(config: &ConnectionConfig, password: Option<String>) -> Result<Arc<dyn Connector>> {
    let engine = &config.engine;
    match engine.plugin_id() {
        None if engine.id == "sqlite" => Ok(Arc::new(sqlite::Sqlite::connect(&config.path).await?)),
        None => Err(format!("Moteur intégré inconnu : {}", engine.id)),
        Some(id) => {
            let binary = crate::plugins::installed_binary(id).ok_or_else(|| {
                format!("Le connecteur {engine} n'est pas installé : ouvre « Gérer » puis « Connecteurs » pour le télécharger.")
            })?;
            let params = quarry_protocol::ConnectParams {
                host: config.host.clone(),
                port: config.port.or(engine.default_port()),
                user: config.user.clone(),
                password,
                database: config.database.clone(),
                path: config.path.clone(),
                project_id: config.project_id.clone(),
                credentials_path: config.credentials_path.clone(),
                max_rows: MAX_ROWS,
            };
            Ok(Arc::new(process::ProcessConnector::launch(&binary, params).await?))
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Connecteur factice : seules les méthodes de génération SQL servent.
    #[derive(Debug)]
    pub struct Fake(pub Dialect);

    impl Fake {
        pub fn standard() -> Self {
            Self(Dialect {
                identifier_quote: '"',
                backslash_escapes: false,
                has_schemas: true,
                rename_table_statement: false,
                schema_is_database: false,
                types: vec!["INTEGER".into(), "TEXT".into()],
                id_type: "INTEGER".into(),
                import_types: ["BIGINT".into(), "DOUBLE PRECISION".into(), "BOOLEAN".into(), "TEXT".into()],
            })
        }
    }

    #[async_trait]
    impl Connector for Fake {
        fn engine(&self) -> &str {
            "fake"
        }
        fn dialect(&self) -> &Dialect {
            &self.0
        }
        async fn list_schemas(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
        async fn list_tables(&self) -> Result<Vec<TableRef>> {
            Ok(Vec::new())
        }
        async fn describe_table(&self, _: &TableRef) -> Result<Vec<ColumnInfo>> {
            Ok(Vec::new())
        }
        async fn list_relations(&self) -> Result<Vec<Relation>> {
            Ok(Vec::new())
        }
        async fn explain(&self, _: &str) -> Result<QueryPlan> {
            Ok(Default::default())
        }
        async fn query(&self, _: &str) -> Result<QueryResult> {
            Ok(Default::default())
        }
        async fn execute(&self, _: &str) -> Result<u64> {
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Fake;
    use super::*;

    #[test]
    fn mysql_dialect_changes_quoting_and_rename() {
        let mut fake = Fake::standard();
        fake.0.identifier_quote = '`';
        fake.0.backslash_escapes = true;
        fake.0.rename_table_statement = true;
        fake.0.schema_is_database = true;
        fake.0.has_schemas = false;
        let t = TableRef { schema: None, name: "users".into() };
        assert_eq!(fake.quote_identifier("a`b"), "`a``b`");
        assert_eq!(fake.quote_literal("C:\\x'y"), "'C:\\\\x''y'");
        assert_eq!(fake.alter_table_sql(&t, &Alter::RenameTable("people".into())), "RENAME TABLE `users` TO `people`");
        assert_eq!(fake.create_schema_sql("app").unwrap(), "CREATE DATABASE `app`");
    }

    #[test]
    fn standard_dialect_generates_ansi_sql() {
        let fake = Fake::standard();
        let t = TableRef { schema: Some("public".into()), name: "users".into() };
        assert_eq!(
            fake.update_sql(&t, &[("name".into(), Input::Text("O'Brien".into()))], &[("id".into(), Cell::Int(3))]),
            "UPDATE \"public\".\"users\"\nSET \"name\" = 'O''Brien'\nWHERE \"id\" = 3"
        );
        assert_eq!(fake.create_schema_sql("ventes").unwrap(), "CREATE SCHEMA \"ventes\"");
    }
}
