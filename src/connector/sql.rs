//! Connecteur pour les moteurs SQL gérés par sqlx : PostgreSQL, MySQL, SQLite.

use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::types::{JsonValue, Uuid};
use sqlx::{AssertSqlSafe, Column, Row, TypeInfo, ValueRef};

use super::plan::{self, QueryPlan};
use super::{Alter, Cell, ColumnInfo, Connector, MAX_ROWS, QueryResult, Relation, Result, TableRef};
use crate::store::{ConnectionConfig, Engine};

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
    /// Les options sont construites champ par champ plutôt que via une URL :
    /// un mot de passe contenant `@` ou `/` n'a pas besoin d'être échappé.
    pub async fn connect(config: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let port = config.port.or(config.engine.default_port()).unwrap_or_default();
        let database = (!config.database.is_empty()).then_some(config.database.as_str());

        match config.engine {
            Engine::Postgres => {
                let mut options =
                    PgConnectOptions::new().host(&config.host).port(port).username(&config.user);
                if let Some(password) = password {
                    options = options.password(password);
                }
                if let Some(database) = database {
                    options = options.database(database);
                }
                let pool = PgPoolOptions::new().max_connections(MAX_CONNECTIONS);
                Ok(Self::Postgres(pool.connect_with(options).await.map_err(err)?))
            }
            Engine::MySql => {
                let mut options =
                    MySqlConnectOptions::new().host(&config.host).port(port).username(&config.user);
                if let Some(password) = password {
                    options = options.password(password);
                }
                if let Some(database) = database {
                    options = options.database(database);
                }
                let pool = MySqlPoolOptions::new().max_connections(MAX_CONNECTIONS);
                Ok(Self::MySql(pool.connect_with(options).await.map_err(err)?))
            }
            Engine::Sqlite => {
                if config.path.trim().is_empty() {
                    return Err("Chemin du fichier SQLite manquant".into());
                }
                let options =
                    SqliteConnectOptions::new().filename(&config.path).create_if_missing(true);
                let pool = SqlitePoolOptions::new().max_connections(MAX_CONNECTIONS);
                Ok(Self::Sqlite(pool.connect_with(options).await.map_err(err)?))
            }
            Engine::BigQuery => Err("BigQuery n'est pas un moteur sqlx".into()),
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

    fn quote_literal(&self, value: &str) -> String {
        match self {
            // MySQL interprète l'antislash dans les littéraux.
            Self::MySql(_) => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''")),
            _ => format!("'{}'", value.replace('\'', "''")),
        }
    }

    fn alter_table_sql(&self, table: &TableRef, alter: &Alter) -> String {
        match (self, alter) {
            // MySQL < 8 : RENAME COLUMN n'existe pas ; CHANGE exige le type,
            // que l'on ne connaît pas ici. On garde la syntaxe standard (MySQL 8+).
            (Self::MySql(_), Alter::RenameTable(to)) => {
                format!("RENAME TABLE {} TO {}", self.quote_table(table), self.quote_identifier(to))
            }
            _ => default_alter_table_sql(self, table, alter),
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

    async fn list_relations(&self) -> Result<Vec<Relation>> {
        match self {
            Self::Postgres(pool) => {
                let rows: Vec<(String, String, Vec<String>, String, String, Vec<String>)> =
                    sqlx::query_as(
                        "SELECT n.nspname::text, c.relname::text,
                                array_agg(a.attname::text ORDER BY k.ord),
                                fn.nspname::text, fc.relname::text,
                                array_agg(fa.attname::text ORDER BY k.ord)
                         FROM pg_constraint con
                         JOIN pg_class c ON c.oid = con.conrelid
                         JOIN pg_namespace n ON n.oid = c.relnamespace
                         JOIN pg_class fc ON fc.oid = con.confrelid
                         JOIN pg_namespace fn ON fn.oid = fc.relnamespace
                         CROSS JOIN LATERAL unnest(con.conkey, con.confkey)
                             WITH ORDINALITY AS k(attnum, fattnum, ord)
                         JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum
                         JOIN pg_attribute fa ON fa.attrelid = fc.oid AND fa.attnum = k.fattnum
                         WHERE con.contype = 'f'
                         GROUP BY con.oid, n.nspname, c.relname, fn.nspname, fc.relname
                         ORDER BY 1, 2",
                    )
                    .fetch_all(pool)
                    .await
                    .map_err(err)?;
                Ok(rows
                    .into_iter()
                    .map(|(schema, name, cols, to_schema, to_name, to_cols)| Relation {
                        from: TableRef { schema: Some(schema), name },
                        from_columns: cols,
                        to: TableRef { schema: Some(to_schema), name: to_name },
                        to_columns: to_cols,
                    })
                    .collect())
            }
            Self::MySql(pool) => {
                let rows: Vec<(String, String, String, String)> = sqlx::query_as(
                    "SELECT CAST(table_name AS CHAR),
                            CAST(GROUP_CONCAT(column_name ORDER BY ordinal_position SEPARATOR ',') AS CHAR),
                            CAST(referenced_table_name AS CHAR),
                            CAST(GROUP_CONCAT(referenced_column_name ORDER BY ordinal_position SEPARATOR ',') AS CHAR)
                     FROM information_schema.key_column_usage
                     WHERE table_schema = DATABASE() AND referenced_table_name IS NOT NULL
                     GROUP BY constraint_name, table_name, referenced_table_name
                     ORDER BY 1",
                )
                .fetch_all(pool)
                .await
                .map_err(err)?;
                let split = |s: String| s.split(',').map(str::to_owned).collect();
                Ok(rows
                    .into_iter()
                    .map(|(name, cols, to_name, to_cols)| Relation {
                        from: TableRef { schema: None, name },
                        from_columns: split(cols),
                        to: TableRef { schema: None, name: to_name },
                        to_columns: split(to_cols),
                    })
                    .collect())
            }
            Self::Sqlite(pool) => {
                let mut relations = Vec::new();
                for table in self.list_tables().await? {
                    let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
                        "SELECT id, \"from\", \"table\", \"to\"
                         FROM pragma_foreign_key_list(?) ORDER BY id, seq",
                    )
                    .bind(&table.name)
                    .fetch_all(pool)
                    .await
                    .map_err(err)?;
                    // Une clé composite donne plusieurs lignes avec le même id.
                    let mut current: Option<(i64, Relation)> = None;
                    for (id, from_col, to_name, to_col) in rows {
                        match &mut current {
                            Some((cur_id, rel)) if *cur_id == id => {
                                rel.from_columns.push(from_col);
                                rel.to_columns.push(to_col.unwrap_or_default());
                            }
                            _ => {
                                if let Some((_, rel)) = current.take() {
                                    relations.push(rel);
                                }
                                current = Some((
                                    id,
                                    Relation {
                                        from: table.clone(),
                                        from_columns: vec![from_col],
                                        to: TableRef { schema: None, name: to_name },
                                        to_columns: vec![to_col.unwrap_or_default()],
                                    },
                                ));
                            }
                        }
                    }
                    if let Some((_, rel)) = current {
                        relations.push(rel);
                    }
                }
                Ok(relations)
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

    async fn execute(&self, sql: &str) -> Result<u64> {
        let sql = AssertSqlSafe(sql.to_owned());
        let affected = match self {
            Self::Postgres(pool) => sqlx::query(sql).execute(pool).await.map_err(err)?.rows_affected(),
            Self::MySql(pool) => sqlx::query(sql).execute(pool).await.map_err(err)?.rows_affected(),
            Self::Sqlite(pool) => sqlx::query(sql).execute(pool).await.map_err(err)?.rows_affected(),
        };
        Ok(affected)
    }
}

/// Version standard de `alter_table_sql`, réutilisée quand le dialecte n'a
/// pas de cas particulier.
fn default_alter_table_sql(c: &SqlConnector, table: &TableRef, alter: &Alter) -> String {
    let table = c.quote_table(table);
    match alter {
        Alter::AddColumn(column) => format!("ALTER TABLE {table}\nADD COLUMN {}", c.column_ddl(column)),
        Alter::DropColumn(name) => format!("ALTER TABLE {table}\nDROP COLUMN {}", c.quote_identifier(name)),
        Alter::RenameColumn { from, to } => format!(
            "ALTER TABLE {table}\nRENAME COLUMN {} TO {}",
            c.quote_identifier(from),
            c.quote_identifier(to)
        ),
        Alter::RenameTable(to) => format!("ALTER TABLE {table}\nRENAME TO {}", c.quote_identifier(to)),
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

        let relations = c.list_relations().await.unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].from.name, "orders");
        assert_eq!(relations[0].from_columns, ["user_id"]);
        assert_eq!(relations[0].to.name, "users");
        assert_eq!(relations[0].to_columns, ["id"]);
    }

    #[tokio::test]
    async fn generated_sql_round_trips() {
        use super::super::{ColumnDef, Input};

        let c = demo().await;
        let users = TableRef { schema: None, name: "users".into() };

        let insert = c.insert_sql(
            &users,
            &[("name".into(), Input::Text("O'Brien".into())), ("score".into(), Input::Null)],
        );
        assert_eq!(insert, "INSERT INTO \"users\" (\"name\", \"score\")\nVALUES ('O''Brien', NULL)");
        assert_eq!(c.execute(&insert).await.unwrap(), 1);

        let key = [("id".into(), Cell::Int(3))];
        let update = c.update_sql(&users, &[("score".into(), Input::Text("7.5".into()))], &key);
        assert_eq!(update, "UPDATE \"users\"\nSET \"score\" = '7.5'\nWHERE \"id\" = 3");
        assert_eq!(c.execute(&update).await.unwrap(), 1);
        let row = c.query("SELECT score FROM users WHERE id = 3").await.unwrap();
        assert_eq!(row.rows[0][0].display(), "7.5");

        assert_eq!(c.execute(&c.delete_sql(&users, &key)).await.unwrap(), 1);

        let tags = TableRef { schema: None, name: "tags".into() };
        let create = c.create_table_sql(
            &tags,
            &[
                ColumnDef { name: "id".into(), data_type: "INTEGER".into(), nullable: false, primary_key: true },
                ColumnDef { name: "label".into(), data_type: "TEXT".into(), nullable: true, primary_key: false },
            ],
        );
        assert_eq!(
            create,
            "CREATE TABLE \"tags\" (\n    \"id\" INTEGER NOT NULL,\n    \"label\" TEXT,\n    PRIMARY KEY (\"id\")\n)"
        );
        c.execute(&create).await.unwrap();
        let add = c.alter_table_sql(
            &tags,
            &Alter::AddColumn(ColumnDef { name: "color".into(), data_type: "TEXT".into(), nullable: true, primary_key: false }),
        );
        c.execute(&add).await.unwrap();
        c.execute(&c.alter_table_sql(&tags, &Alter::RenameColumn { from: "color".into(), to: "colour".into() })).await.unwrap();
        c.execute(&c.alter_table_sql(&tags, &Alter::DropColumn("colour".into()))).await.unwrap();
        c.execute(&c.alter_table_sql(&tags, &Alter::RenameTable("labels".into()))).await.unwrap();
        c.execute(&c.drop_table_sql(&TableRef { schema: None, name: "labels".into() })).await.unwrap();

        let names: Vec<_> = c.list_tables().await.unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["orders", "users"]);
    }
}
