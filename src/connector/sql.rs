//! Connecteur pour les moteurs SQL gérés par sqlx : PostgreSQL, MySQL, SQLite.

use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::types::{JsonValue, Uuid};
use sqlx::{AssertSqlSafe, Column, Row, TypeInfo, ValueRef};

use super::plan::{self, QueryPlan};
use super::{Cell, ColumnInfo, Connector, QueryResult, Result, TableRef};

/// Au-delà, on arrête de lire : la grille reste fluide et la mémoire basse.
const MAX_ROWS: usize = 5_000;
/// Peu de connexions suffisent pour un client interactif.
const MAX_CONNECTIONS: u32 = 4;

#[derive(Debug)]
pub enum SqlConnector {
    Postgres(PgPool),
    MySql(MySqlPool),
    Sqlite(SqlitePool),
}

fn err(e: sqlx::Error) -> String {
    e.to_string()
}

impl SqlConnector {
    pub async fn connect(url: &str) -> Result<Self> {
        if url.starts_with("postgres") {
            let pool = PgPoolOptions::new().max_connections(MAX_CONNECTIONS);
            Ok(Self::Postgres(pool.connect(url).await.map_err(err)?))
        } else if url.starts_with("mysql") {
            let pool = MySqlPoolOptions::new().max_connections(MAX_CONNECTIONS);
            Ok(Self::MySql(pool.connect(url).await.map_err(err)?))
        } else {
            let pool = SqlitePoolOptions::new().max_connections(MAX_CONNECTIONS);
            Ok(Self::Sqlite(pool.connect(url).await.map_err(err)?))
        }
    }
}

/// Décode une ligne en essayant les types courants un par un.
/// sqlx vérifie la compatibilité des types : un essai qui échoue est sans effet.
macro_rules! row_decoder {
    ($name:ident, $row:ty, unchecked_text: $unchecked:literal) => {
        fn $name(row: &$row) -> Vec<Cell> {
            (0..row.len())
                .map(|i| {
                    let type_name = match row.try_get_raw(i) {
                        Ok(raw) if raw.is_null() => return Cell::Null,
                        Ok(raw) => raw.type_info().name().to_string(),
                        Err(e) => return Cell::Unsupported(e.to_string()),
                    };
                    if let Ok(v) = row.try_get::<i64, _>(i) {
                        Cell::Int(v)
                    } else if let Ok(v) = row.try_get::<i32, _>(i) {
                        Cell::Int(v.into())
                    } else if let Ok(v) = row.try_get::<i16, _>(i) {
                        Cell::Int(v.into())
                    } else if let Ok(v) = row.try_get::<f64, _>(i) {
                        Cell::Float(v)
                    } else if let Ok(v) = row.try_get::<f32, _>(i) {
                        Cell::Float(v.into())
                    } else if let Ok(v) = row.try_get::<bool, _>(i) {
                        Cell::Bool(v)
                    } else if let Ok(v) = row.try_get::<String, _>(i) {
                        Cell::Text(v)
                    } else if let Ok(v) = row.try_get::<DateTime<Utc>, _>(i) {
                        Cell::Text(v.to_rfc3339())
                    } else if let Ok(v) = row.try_get::<NaiveDateTime, _>(i) {
                        Cell::Text(v.to_string())
                    } else if let Ok(v) = row.try_get::<NaiveDate, _>(i) {
                        Cell::Text(v.to_string())
                    } else if let Ok(v) = row.try_get::<NaiveTime, _>(i) {
                        Cell::Text(v.to_string())
                    } else if let Ok(v) = row.try_get::<Uuid, _>(i) {
                        Cell::Text(v.to_string())
                    } else if let Ok(v) = row.try_get::<JsonValue, _>(i) {
                        Cell::Text(v.to_string())
                    } else if $unchecked
                        && let Ok(v) = row.try_get_unchecked::<String, _>(i)
                    {
                        // MySQL transmet DECIMAL & co. sous forme de texte.
                        Cell::Text(v)
                    } else {
                        Cell::Unsupported(type_name)
                    }
                })
                .collect()
        }
    };
}

row_decoder!(decode_pg, PgRow, unchecked_text: false);
row_decoder!(decode_mysql, MySqlRow, unchecked_text: true);
row_decoder!(decode_sqlite, SqliteRow, unchecked_text: false);

/// Lit au plus `MAX_ROWS` lignes et les convertit en [`QueryResult`].
macro_rules! run_query {
    ($pool:expr, $sql:expr, $decode:ident) => {{
        let mut stream = sqlx::query(AssertSqlSafe($sql.to_owned())).fetch($pool);
        let mut result = QueryResult::default();
        while let Some(row) = stream.try_next().await.map_err(err)? {
            if result.columns.is_empty() {
                result.columns = row.columns().iter().map(|c| c.name().to_string()).collect();
            }
            result.rows.push($decode(&row));
            if result.rows.len() >= MAX_ROWS {
                break;
            }
        }
        Ok(result)
    }};
}

#[async_trait]
impl Connector for SqlConnector {
    fn engine(&self) -> &'static str {
        match self {
            Self::Postgres(_) => "PostgreSQL",
            Self::MySql(_) => "MySQL",
            Self::Sqlite(_) => "SQLite",
        }
    }

    fn quote_identifier(&self, ident: &str) -> String {
        match self {
            Self::MySql(_) => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    async fn list_tables(&self) -> Result<Vec<TableRef>> {
        match self {
            Self::Postgres(pool) => {
                let rows: Vec<(String, String)> = sqlx::query_as(
                    "SELECT table_schema::text, table_name::text
                     FROM information_schema.tables
                     WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
                     ORDER BY 1, 2",
                )
                .fetch_all(pool)
                .await
                .map_err(err)?;
                Ok(rows
                    .into_iter()
                    .map(|(schema, name)| TableRef { schema: Some(schema), name })
                    .collect())
            }
            Self::MySql(pool) => {
                let rows: Vec<(String,)> = sqlx::query_as(
                    "SELECT CAST(table_name AS CHAR)
                     FROM information_schema.tables
                     WHERE table_schema = DATABASE()
                     ORDER BY 1",
                )
                .fetch_all(pool)
                .await
                .map_err(err)?;
                Ok(rows.into_iter().map(|(name,)| TableRef { schema: None, name }).collect())
            }
            Self::Sqlite(pool) => {
                let rows: Vec<(String,)> = sqlx::query_as(
                    "SELECT name FROM sqlite_master
                     WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
                     ORDER BY name",
                )
                .fetch_all(pool)
                .await
                .map_err(err)?;
                Ok(rows.into_iter().map(|(name,)| TableRef { schema: None, name }).collect())
            }
        }
    }

    async fn describe_table(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        let to_info = |(name, data_type, nullable, default, primary_key)| ColumnInfo {
            name,
            data_type,
            nullable,
            default,
            primary_key,
        };

        match self {
            Self::Postgres(pool) => {
                let rows: Vec<(String, String, bool, Option<String>, bool)> = sqlx::query_as(
                    "SELECT c.column_name::text,
                            c.data_type::text,
                            c.is_nullable = 'YES',
                            c.column_default::text,
                            EXISTS (
                                SELECT 1
                                FROM information_schema.table_constraints tc
                                JOIN information_schema.key_column_usage k
                                  ON k.constraint_name = tc.constraint_name
                                 AND k.table_schema = tc.table_schema
                                 AND k.table_name = tc.table_name
                                WHERE tc.constraint_type = 'PRIMARY KEY'
                                  AND tc.table_schema = c.table_schema
                                  AND tc.table_name = c.table_name
                                  AND k.column_name = c.column_name
                            )
                     FROM information_schema.columns c
                     WHERE c.table_schema = $1 AND c.table_name = $2
                     ORDER BY c.ordinal_position",
                )
                .bind(table.schema.as_deref().unwrap_or("public"))
                .bind(&table.name)
                .fetch_all(pool)
                .await
                .map_err(err)?;
                Ok(rows.into_iter().map(to_info).collect())
            }
            Self::MySql(pool) => {
                let rows: Vec<(String, String, i64, Option<String>, i64)> = sqlx::query_as(
                    "SELECT CAST(column_name AS CHAR),
                            CAST(column_type AS CHAR),
                            is_nullable = 'YES',
                            CAST(column_default AS CHAR),
                            column_key = 'PRI'
                     FROM information_schema.columns
                     WHERE table_schema = DATABASE() AND table_name = ?
                     ORDER BY ordinal_position",
                )
                .bind(&table.name)
                .fetch_all(pool)
                .await
                .map_err(err)?;
                Ok(rows
                    .into_iter()
                    .map(|(n, t, null, d, pk)| to_info((n, t, null != 0, d, pk != 0)))
                    .collect())
            }
            Self::Sqlite(pool) => {
                let rows: Vec<(String, String, i64, Option<String>, i64)> = sqlx::query_as(
                    "SELECT name, type, \"notnull\", CAST(dflt_value AS TEXT), pk
                     FROM pragma_table_info(?)",
                )
                .bind(&table.name)
                .fetch_all(pool)
                .await
                .map_err(err)?;
                Ok(rows
                    .into_iter()
                    .map(|(n, t, notnull, d, pk)| to_info((n, t, notnull == 0, d, pk != 0)))
                    .collect())
            }
        }
    }

    async fn explain(&self, sql: &str) -> Result<QueryPlan> {
        // Aucun de ces EXPLAIN n'exécute la requête (pas d'ANALYZE).
        match self {
            Self::Postgres(pool) => {
                let (json,): (JsonValue,) =
                    sqlx::query_as(AssertSqlSafe(format!("EXPLAIN (FORMAT JSON) {sql}")))
                        .fetch_one(pool)
                        .await
                        .map_err(err)?;
                plan::from_postgres_json(&json)
            }
            Self::MySql(pool) => {
                let (tree,): (String,) =
                    sqlx::query_as(AssertSqlSafe(format!("EXPLAIN FORMAT=TREE {sql}")))
                        .fetch_one(pool)
                        .await
                        .map_err(|e| {
                            format!("{e}\n(EXPLAIN FORMAT=TREE nécessite MySQL 8.0.18 ou plus)")
                        })?;
                Ok(plan::from_mysql_tree(&tree))
            }
            Self::Sqlite(pool) => {
                let rows: Vec<(i64, i64, i64, String)> =
                    sqlx::query_as(AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
                        .fetch_all(pool)
                        .await
                        .map_err(err)?;
                let rows: Vec<_> = rows.into_iter().map(|(id, parent, _, detail)| (id, parent, detail)).collect();
                Ok(plan::from_sqlite(&rows))
            }
        }
    }

    async fn query(&self, sql: &str) -> Result<QueryResult> {
        match self {
            Self::Postgres(pool) => run_query!(pool, sql, decode_pg),
            Self::MySql(pool) => run_query!(pool, sql, decode_mysql),
            Self::Sqlite(pool) => run_query!(pool, sql, decode_sqlite),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn demo() -> SqlConnector {
        // Une seule connexion : chaque connexion `:memory:` aurait sa propre base.
        let pool = SqlitePoolOptions::new().max_connections(1);
        let c = SqlConnector::Sqlite(pool.connect("sqlite::memory:").await.unwrap());
        for sql in [
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL)",
            "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id))",
            "INSERT INTO users (name, score) VALUES ('Ada', 9.5), ('Linus', NULL)",
        ] {
            c.query(sql).await.unwrap();
        }
        c
    }

    #[tokio::test]
    async fn sqlite_lists_describes_queries_and_explains() {
        let c = demo().await;

        let tables = c.list_tables().await.unwrap();
        let names: Vec<_> = tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["orders", "users"]);

        let schema = c.describe_table(&tables[1]).await.unwrap();
        assert_eq!(schema.len(), 3);
        assert!(schema[0].primary_key);
        assert!(!schema[1].nullable);

        let result = c.query("SELECT name, score FROM users ORDER BY id").await.unwrap();
        assert_eq!(result.columns, ["name", "score"]);
        assert_eq!(result.rows[1][1].display(), "NULL");

        let plan = c
            .explain("SELECT * FROM users u JOIN orders o ON o.user_id = u.id")
            .await
            .unwrap();
        assert!(!plan.nodes.is_empty());
        assert_eq!(plan.total_cost, None);
    }
}
