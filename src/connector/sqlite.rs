//! SQLite, intégré à l'application : fonctionne sans aucun téléchargement
//! et sert de base d'essai.

use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{AssertSqlSafe, Column, Row, TypeInfo, ValueRef};

use super::plan;
use super::{Cell, ColumnInfo, Connector, Dialect, MAX_ROWS, QueryPlan, QueryResult, Relation, Result, TableRef};

#[derive(Debug)]
pub struct Sqlite {
    pool: SqlitePool,
    dialect: Dialect,
}

fn err(e: sqlx::Error) -> String {
    e.to_string()
}

fn dialect() -> Dialect {
    Dialect {
        identifier_quote: '"',
        backslash_escapes: false,
        has_schemas: false,
        rename_table_statement: false,
        schema_is_database: false,
        types: ["INTEGER", "REAL", "TEXT", "BLOB", "NUMERIC"].map(String::from).to_vec(),
        id_type: "INTEGER".into(),
        import_types: ["INTEGER".into(), "REAL".into(), "INTEGER".into(), "TEXT".into()],
    }
}

fn decode(row: &SqliteRow) -> Vec<Cell> {
    (0..row.len())
        .map(|i| {
            let type_name = match row.try_get_raw(i) {
                Ok(raw) if raw.is_null() => return Cell::Null,
                Ok(raw) => raw.type_info().name().to_string(),
                Err(e) => return Cell::Unsupported(e.to_string()),
            };
            if let Ok(v) = row.try_get::<i64, _>(i) {
                Cell::Int(v)
            } else if let Ok(v) = row.try_get::<f64, _>(i) {
                Cell::Float(v)
            } else if let Ok(v) = row.try_get::<bool, _>(i) {
                Cell::Bool(v)
            } else if let Ok(v) = row.try_get::<String, _>(i) {
                Cell::Text(v)
            } else {
                Cell::Unsupported(type_name)
            }
        })
        .collect()
}

impl Sqlite {
    pub async fn connect(path: &str) -> Result<Self> {
        if path.trim().is_empty() {
            return Err("Chemin du fichier SQLite manquant".into());
        }
        let options = SqliteConnectOptions::new().filename(path).create_if_missing(true);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(options).await.map_err(err)?;
        Ok(Self { pool, dialect: dialect() })
    }

    #[cfg(test)]
    pub async fn in_memory() -> Self {
        // Une seule connexion : chaque connexion `:memory:` aurait sa propre base.
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        Self { pool, dialect: dialect() }
    }
}

#[async_trait]
impl Connector for Sqlite {
    fn engine(&self) -> &str {
        "SQLite"
    }

    fn dialect(&self) -> &Dialect {
        &self.dialect
    }

    async fn list_schemas(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn list_tables(&self) -> Result<Vec<TableRef>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(err)?;
        Ok(rows.into_iter().map(|(name,)| TableRef { schema: None, name }).collect())
    }

    async fn describe_table(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        let rows: Vec<(String, String, i64, Option<String>, i64)> = sqlx::query_as(
            "SELECT name, type, \"notnull\", CAST(dflt_value AS TEXT), pk FROM pragma_table_info(?)",
        )
        .bind(&table.name)
        .fetch_all(&self.pool)
        .await
        .map_err(err)?;
        Ok(rows
            .into_iter()
            .map(|(name, data_type, notnull, default, pk)| ColumnInfo {
                name,
                data_type,
                nullable: notnull == 0,
                default,
                primary_key: pk != 0,
            })
            .collect())
    }

    async fn list_relations(&self) -> Result<Vec<Relation>> {
        let mut relations = Vec::new();
        for table in self.list_tables().await? {
            let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
                "SELECT id, \"from\", \"table\", \"to\" FROM pragma_foreign_key_list(?) ORDER BY id, seq",
            )
            .bind(&table.name)
            .fetch_all(&self.pool)
            .await
            .map_err(err)?;
            // Une clé composite donne plusieurs lignes avec le même id.
            let mut current: Option<(i64, Relation)> = None;
            for (id, from_col, to_name, to_col) in rows {
                match &mut current {
                    Some((cur, rel)) if *cur == id => {
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

    /// `EXPLAIN QUERY PLAN` : pas de coût chez SQLite.
    async fn explain(&self, sql: &str) -> Result<QueryPlan> {
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
                .persistent(false)
                .fetch_all(&self.pool)
                .await
                .map_err(err)?;
        let rows: Vec<_> = rows.into_iter().map(|(id, parent, _, detail)| (id, parent, detail)).collect();
        Ok(plan::from_sqlite(&rows))
    }

    async fn query(&self, sql: &str) -> Result<QueryResult> {
        let mut stream = sqlx::query(AssertSqlSafe(sql.to_owned())).persistent(false).fetch(&self.pool);
        let mut result = QueryResult::default();
        while let Some(row) = stream.try_next().await.map_err(err)? {
            if result.columns.is_empty() {
                result.columns = row.columns().iter().map(|c| c.name().to_string()).collect();
            }
            result.rows.push(decode(&row));
            if result.rows.len() >= MAX_ROWS {
                break;
            }
        }
        Ok(result)
    }

    async fn execute(&self, sql: &str) -> Result<u64> {
        Ok(sqlx::query(AssertSqlSafe(sql.to_owned()))
            .persistent(false)
            .execute(&self.pool)
            .await
            .map_err(err)?
            .rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Alter, ColumnDef, Input};
    use super::*;

    async fn demo() -> Sqlite {
        let c = Sqlite::in_memory().await;
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
    async fn lists_describes_queries_and_explains() {
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

        let plan = c.explain("SELECT * FROM users u JOIN orders o ON o.user_id = u.id").await.unwrap();
        assert!(!plan.nodes.is_empty());
        assert_eq!(plan.total_cost, None);

        let relations = c.list_relations().await.unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].from.name, "orders");
        assert_eq!(relations[0].to_columns, ["id"]);
        assert!(c.list_schemas().await.unwrap().is_empty());
        assert!(c.create_schema_sql("x").is_none());
    }

    #[tokio::test]
    async fn generated_sql_round_trips() {
        let c = demo().await;
        let users = TableRef { schema: None, name: "users".into() };

        let insert = c.insert_sql(&users, &[("name".into(), Input::Text("O'Brien".into())), ("score".into(), Input::Null)]);
        assert_eq!(insert, "INSERT INTO \"users\" (\"name\", \"score\")\nVALUES ('O''Brien', NULL)");
        assert_eq!(c.execute(&insert).await.unwrap(), 1);

        let key = [("id".into(), Cell::Int(3))];
        let update = c.update_sql(&users, &[("score".into(), Input::Text("7.5".into()))], &key);
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
        c.execute(&create).await.unwrap();
        let add = ColumnDef { name: "color".into(), data_type: "TEXT".into(), nullable: true, primary_key: false };
        c.execute(&c.alter_table_sql(&tags, &Alter::AddColumn(add))).await.unwrap();
        c.execute(&c.alter_table_sql(&tags, &Alter::RenameColumn { from: "color".into(), to: "colour".into() })).await.unwrap();
        c.execute(&c.alter_table_sql(&tags, &Alter::DropColumn("colour".into()))).await.unwrap();
        c.execute(&c.alter_table_sql(&tags, &Alter::RenameTable("labels".into()))).await.unwrap();
        c.execute(&c.drop_table_sql(&TableRef { schema: None, name: "labels".into() })).await.unwrap();

        let names: Vec<_> = c.list_tables().await.unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["orders", "users"]);
    }
}
