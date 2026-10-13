//! Export d'un jeu de lignes (résultats ou données d'une table) vers un
//! fichier : CSV, JSON, instructions INSERT ou classeur Excel.

use std::path::PathBuf;

use crate::connector::{Cell, Connector, Input, TableRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Sql,
    Xlsx,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Csv, Format::Json, Format::Sql, Format::Xlsx];

    pub fn label(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Json => "JSON",
            Format::Sql => "SQL (INSERT)",
            Format::Xlsx => "Excel (.xlsx)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Sql => "sql",
            Format::Xlsx => "xlsx",
        }
    }
}

fn json_value(cell: &Cell) -> serde_json::Value {
    match cell {
        Cell::Null | Cell::Unsupported(_) => serde_json::Value::Null,
        Cell::Bool(b) => (*b).into(),
        Cell::Int(i) => (*i).into(),
        Cell::Float(f) => serde_json::Number::from_f64(*f).map_or(serde_json::Value::Null, Into::into),
        Cell::Text(s) => s.clone().into(),
    }
}

/// Produit le contenu du fichier. `table` sert aux INSERT (nom de table) ;
/// sans table, les INSERT visent `export`.
pub fn render(
    format: Format,
    columns: &[String],
    rows: &[&Vec<Cell>],
    table: Option<&TableRef>,
    connector: &dyn Connector,
) -> Result<Vec<u8>, String> {
    match format {
        Format::Csv => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            writer.write_record(columns).map_err(|e| e.to_string())?;
            for row in rows {
                let record: Vec<String> = row
                    .iter()
                    .map(|c| match c {
                        Cell::Null => String::new(),
                        other => other.display(),
                    })
                    .collect();
                writer.write_record(&record).map_err(|e| e.to_string())?;
            }
            writer.into_inner().map_err(|e| e.to_string())
        }
        Format::Json => {
            let objects: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .zip(row.iter())
                        .map(|(name, cell)| (name.clone(), json_value(cell)))
                        .collect::<serde_json::Map<_, _>>()
                        .into()
                })
                .collect();
            serde_json::to_vec_pretty(&objects).map_err(|e| e.to_string())
        }
        Format::Sql => {
            let fallback = TableRef { schema: None, name: "export".into() };
            let table = table.unwrap_or(&fallback);
            let mut out = String::new();
            for row in rows {
                let values: Vec<(String, Input)> = columns
                    .iter()
                    .zip(row.iter())
                    .map(|(name, cell)| {
                        let input = match cell {
                            Cell::Null | Cell::Unsupported(_) => Input::Null,
                            other => Input::Text(other.display()),
                        };
                        (name.clone(), input)
                    })
                    .collect();
                out.push_str(&connector.insert_sql(table, &values).replace('\n', " "));
                out.push_str(";\n");
            }
            Ok(out.into_bytes())
        }
        Format::Xlsx => {
            let mut workbook = rust_xlsxwriter::Workbook::new();
            let sheet = workbook.add_worksheet();
            let bold = rust_xlsxwriter::Format::new().set_bold();
            for (c, name) in columns.iter().enumerate() {
                sheet.write_string_with_format(0, c as u16, name, &bold).map_err(|e| e.to_string())?;
            }
            for (r, row) in rows.iter().enumerate() {
                let r = r as u32 + 1;
                for (c, cell) in row.iter().enumerate() {
                    let c = c as u16;
                    let result = match cell {
                        Cell::Null | Cell::Unsupported(_) => Ok(&mut *sheet),
                        Cell::Bool(b) => sheet.write_boolean(r, c, *b),
                        Cell::Int(i) => sheet.write_number(r, c, *i as f64),
                        Cell::Float(f) => sheet.write_number(r, c, *f),
                        Cell::Text(s) => sheet.write_string(r, c, s),
                    };
                    result.map_err(|e| e.to_string())?;
                }
            }
            workbook.save_to_buffer().map_err(|e| e.to_string())
        }
    }
}

/// Demande où enregistrer, puis écrit le fichier. Renvoie le chemin choisi,
/// ou `None` si l'utilisateur annule.
pub async fn save_with_dialog(suggested: String, format: Format, bytes: Vec<u8>) -> Result<Option<PathBuf>, String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_file_name(format!("{suggested}.{}", format.extension()))
        .add_filter(format.label(), &[format.extension()])
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = handle.path().to_path_buf();
    tokio::fs::write(&path, bytes).await.map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connecteur factice : seules les méthodes de génération SQL servent ici.
    #[derive(Debug)]
    struct Fake;

    #[async_trait::async_trait]
    impl Connector for Fake {
        fn engine(&self) -> &'static str {
            "fake"
        }
        async fn list_tables(&self) -> crate::connector::Result<Vec<TableRef>> {
            Ok(Vec::new())
        }
        async fn describe_table(&self, _: &TableRef) -> crate::connector::Result<Vec<crate::connector::ColumnInfo>> {
            Ok(Vec::new())
        }
        async fn list_relations(&self) -> crate::connector::Result<Vec<crate::connector::Relation>> {
            Ok(Vec::new())
        }
        async fn explain(&self, _: &str) -> crate::connector::Result<crate::connector::QueryPlan> {
            Ok(Default::default())
        }
        async fn query(&self, _: &str) -> crate::connector::Result<crate::connector::QueryResult> {
            Ok(Default::default())
        }
        async fn execute(&self, _: &str) -> crate::connector::Result<u64> {
            Ok(0)
        }
    }

    fn sample() -> (Vec<String>, Vec<Vec<Cell>>) {
        (
            vec!["id".into(), "name".into(), "score".into()],
            vec![
                vec![Cell::Int(1), Cell::Text("Ada, \"la\" première".into()), Cell::Float(9.5)],
                vec![Cell::Int(2), Cell::Text("Linus".into()), Cell::Null],
            ],
        )
    }

    #[test]
    fn csv_quotes_and_leaves_null_empty() {
        let (columns, rows) = sample();
        let refs: Vec<&Vec<Cell>> = rows.iter().collect();
        let out = String::from_utf8(render(Format::Csv, &columns, &refs, None, &Fake).unwrap()).unwrap();
        assert_eq!(out, "id,name,score\n1,\"Ada, \"\"la\"\" première\",9.5\n2,Linus,\n");
    }

    #[test]
    fn json_keeps_types() {
        let (columns, rows) = sample();
        let refs: Vec<&Vec<Cell>> = rows.iter().collect();
        let out = render(Format::Json, &columns, &refs, None, &Fake).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value[0]["id"], 1);
        assert_eq!(value[0]["score"], 9.5);
        assert!(value[1]["score"].is_null());
    }

    #[test]
    fn sql_inserts_one_statement_per_row() {
        let (columns, rows) = sample();
        let refs: Vec<&Vec<Cell>> = rows.iter().collect();
        let table = TableRef { schema: None, name: "users".into() };
        let out = String::from_utf8(render(Format::Sql, &columns, &refs, Some(&table), &Fake).unwrap()).unwrap();
        assert_eq!(out.lines().count(), 2);
        assert!(out.starts_with("INSERT INTO \"users\" (\"id\", \"name\", \"score\") VALUES ('1', 'Ada, \"la\" première', '9.5');"));
        assert!(out.contains("('2', 'Linus', NULL)"));
    }

    #[test]
    fn xlsx_is_a_zip() {
        let (columns, rows) = sample();
        let refs: Vec<&Vec<Cell>> = rows.iter().collect();
        let out = render(Format::Xlsx, &columns, &refs, None, &Fake).unwrap();
        assert_eq!(&out[..2], b"PK");
    }
}
