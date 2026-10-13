//! Import d'un fichier (CSV, Excel, Parquet) dans une table existante ou
//! nouvelle. Le fichier est lu en mémoire, ses colonnes mises en
//! correspondance avec celles de la table, puis insérées par lots.

use std::path::{Path, PathBuf};

use iced::widget::{Column, button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Fill, Font};

use crate::app::Message;
use crate::connector::{Cell, ColumnDef, ColumnInfo, Connector, Input, TableRef};
use crate::editing::{Pending, Refresh};
use crate::store::Engine;
use crate::theme;

const MONO: Font = Font::MONOSPACE;
/// Lignes par instruction INSERT.
pub const BATCH: usize = 200;
const PREVIEW_ROWS: usize = 5;
const IGNORE: &str = "(ignorer)";

#[derive(Debug, Clone)]
pub struct ImportedFile {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

/// Texte d'une cellule de fichier → valeur typée quand c'est évident.
fn parse_cell(raw: &str) -> Cell {
    let t = raw.trim();
    if t.is_empty() {
        return Cell::Null;
    }
    if let Ok(i) = t.parse::<i64>() {
        return Cell::Int(i);
    }
    if let Ok(f) = t.replace(',', ".").parse::<f64>()
        && t.chars().any(|c| c.is_ascii_digit())
        && !t.contains(' ')
    {
        return Cell::Float(f);
    }
    match t.to_lowercase().as_str() {
        "true" | "vrai" => Cell::Bool(true),
        "false" | "faux" => Cell::Bool(false),
        _ => Cell::Text(raw.to_string()),
    }
}

fn json_to_cell(v: serde_json::Value) -> Cell {
    match v {
        serde_json::Value::Null => Cell::Null,
        serde_json::Value::Bool(b) => Cell::Bool(b),
        serde_json::Value::Number(n) => n.as_i64().map(Cell::Int).or_else(|| n.as_f64().map(Cell::Float)).unwrap_or(Cell::Null),
        serde_json::Value::String(s) => Cell::Text(s),
        other => Cell::Text(other.to_string()),
    }
}

pub fn read_csv(path: &Path) -> Result<ImportedFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    // Séparateur : celui qui apparaît le plus dans l'en-tête.
    let header = text.lines().next().unwrap_or_default();
    let delimiter = [b',', b';', b'\t', b'|']
        .into_iter()
        .max_by_key(|&d| header.bytes().filter(|&b| b == d).count())
        .unwrap_or(b',');
    let mut reader = csv::ReaderBuilder::new().delimiter(delimiter).flexible(true).from_reader(text.as_bytes());
    let columns: Vec<String> = reader.headers().map_err(|e| e.to_string())?.iter().map(str::to_string).collect();
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        let mut row: Vec<Cell> = record.iter().map(parse_cell).collect();
        row.resize(columns.len(), Cell::Null);
        rows.push(row);
    }
    Ok(ImportedFile { columns, rows })
}

pub fn read_excel(path: &Path) -> Result<ImportedFile, String> {
    use calamine::{Data, Reader};
    let mut workbook = calamine::open_workbook_auto(path).map_err(|e| e.to_string())?;
    let sheet = workbook.sheet_names().first().cloned().ok_or("Classeur sans feuille")?;
    let range = workbook.worksheet_range(&sheet).map_err(|e| e.to_string())?;
    let mut lines = range.rows();
    let columns: Vec<String> = lines
        .next()
        .ok_or("Feuille vide")?
        .iter()
        .enumerate()
        .map(|(i, c)| match c {
            Data::Empty => format!("colonne_{}", i + 1),
            other => other.to_string(),
        })
        .collect();
    let rows = lines
        .map(|line| {
            let mut row: Vec<Cell> = line
                .iter()
                .map(|c| match c {
                    Data::Empty | Data::Error(_) => Cell::Null,
                    Data::Int(i) => Cell::Int(*i),
                    Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => Cell::Int(*f as i64),
                    Data::Float(f) => Cell::Float(*f),
                    Data::Bool(b) => Cell::Bool(*b),
                    Data::DateTime(d) => Cell::Text(d.as_datetime().map_or_else(|| d.to_string(), |dt| dt.to_string())),
                    other => Cell::Text(other.to_string()),
                })
                .collect();
            row.resize(columns.len(), Cell::Null);
            row
        })
        .collect();
    Ok(ImportedFile { columns, rows })
}

pub fn read_parquet(path: &Path) -> Result<ImportedFile, String> {
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let reader = SerializedFileReader::new(file).map_err(|e| e.to_string())?;
    let mut columns: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    for record in reader.get_row_iter(None).map_err(|e| e.to_string())? {
        let record = record.map_err(|e| e.to_string())?;
        if columns.is_empty() {
            columns = record.get_column_iter().map(|(name, _)| name.clone()).collect();
        }
        rows.push(record.get_column_iter().map(|(_, field)| json_to_cell(field.to_json_value())).collect());
    }
    if columns.is_empty() {
        return Err("Fichier Parquet vide".into());
    }
    Ok(ImportedFile { columns, rows })
}

pub fn read_file(path: &Path) -> Result<ImportedFile, String> {
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("csv" | "tsv" | "txt") => read_csv(path),
        Some("xlsx" | "xlsm" | "xls" | "ods") => read_excel(path),
        Some("parquet") => read_parquet(path),
        other => Err(format!("Format non pris en charge : {}", other.unwrap_or("sans extension"))),
    }
}

/// Type de colonne déduit du contenu, dans le dialecte du moteur.
pub fn infer_type(column: usize, rows: &[Vec<Cell>], engine: Engine) -> &'static str {
    let (int, float, boolean, text) = match engine {
        Engine::Postgres => ("BIGINT", "DOUBLE PRECISION", "BOOLEAN", "TEXT"),
        Engine::MySql => ("BIGINT", "DOUBLE", "BOOLEAN", "TEXT"),
        Engine::Sqlite => ("INTEGER", "REAL", "INTEGER", "TEXT"),
        Engine::BigQuery => ("INT64", "FLOAT64", "BOOL", "STRING"),
    };
    let mut seen_int = false;
    let mut seen_float = false;
    let mut seen_bool = false;
    for row in rows {
        match row.get(column) {
            Some(Cell::Null) | None => {}
            Some(Cell::Int(_)) => seen_int = true,
            Some(Cell::Float(_)) => seen_float = true,
            Some(Cell::Bool(_)) => seen_bool = true,
            Some(_) => return text,
        }
    }
    if seen_float {
        float
    } else if seen_int {
        int
    } else if seen_bool {
        boolean
    } else {
        text
    }
}

/// Nom de colonne SQL à partir d'un en-tête de fichier.
pub fn normalize(name: &str) -> String {
    let mut out: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "colonne".into() } else { out }
}

/// Correspondance automatique fichier → table, par nom normalisé.
pub fn auto_mapping(file_columns: &[String], table_columns: &[ColumnInfo]) -> Vec<Option<usize>> {
    file_columns
        .iter()
        .map(|f| {
            let key = normalize(f);
            table_columns.iter().position(|c| normalize(&c.name) == key)
        })
        .collect()
}

/// Instructions INSERT par lots pour les colonnes mappées.
pub fn insert_statements(
    c: &dyn Connector,
    table: &TableRef,
    file: &ImportedFile,
    mapping: &[Option<usize>],
    target_columns: &[String],
) -> Vec<String> {
    let pairs: Vec<(usize, &String)> = mapping
        .iter()
        .enumerate()
        .filter_map(|(file_col, target)| target.and_then(|t| target_columns.get(t)).map(|name| (file_col, name)))
        .collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = pairs.iter().map(|(_, n)| (*n).clone()).collect();
    file.rows
        .chunks(BATCH)
        .map(|chunk| {
            let rows: Vec<Vec<Input>> = chunk
                .iter()
                .map(|row| {
                    pairs
                        .iter()
                        .map(|(file_col, _)| match row.get(*file_col) {
                            Some(Cell::Null) | Some(Cell::Unsupported(_)) | None => Input::Null,
                            Some(other) => Input::Text(other.display()),
                        })
                        .collect()
                })
                .collect();
            c.insert_many_sql(table, &names, &rows)
        })
        .collect()
}

// ----------------------------------------------------------------------
// Formulaire
// ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Existing,
    New,
}

#[derive(Debug, Clone)]
pub struct ImportForm {
    pub path: PathBuf,
    pub file: ImportedFile,
    pub target: Target,
    /// Table existante sélectionnée (si Target::Existing).
    pub existing: Option<(TableRef, Vec<ColumnInfo>)>,
    pub new_schema: Option<String>,
    pub new_name: String,
    /// Pour Target::Existing : colonne cible par colonne du fichier.
    pub mapping: Vec<Option<usize>>,
    pub engine: Engine,
}

#[derive(Debug, Clone)]
pub enum ImportMessage {
    Target(Target),
    NewName(String),
    /// Colonne du fichier `i` → colonne de la table (nom) ou « (ignorer) ».
    Map(usize, String),
    Submit,
    Cancel,
}

impl ImportForm {
    pub fn new(path: PathBuf, file: ImportedFile, existing: Option<(TableRef, Vec<ColumnInfo>)>, engine: Engine) -> Self {
        let mapping = existing.as_ref().map(|(_, cols)| auto_mapping(&file.columns, cols)).unwrap_or_default();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("import");
        let new_schema = existing.as_ref().and_then(|(t, _)| t.schema.clone());
        Self {
            target: if existing.is_some() { Target::Existing } else { Target::New },
            new_name: normalize(stem),
            new_schema,
            mapping,
            path,
            file,
            existing,
            engine,
        }
    }

    pub fn apply(&mut self, message: ImportMessage) {
        match message {
            ImportMessage::Target(t) => self.target = t,
            ImportMessage::NewName(n) => self.new_name = n,
            ImportMessage::Map(i, choice) => {
                if let Some(slot) = self.mapping.get_mut(i) {
                    *slot = self.existing.as_ref().and_then(|(_, cols)| cols.iter().position(|c| c.name == choice));
                }
            }
            ImportMessage::Submit | ImportMessage::Cancel => {}
        }
    }

    pub fn to_sql(&self, c: &dyn Connector) -> Result<Pending, String> {
        if self.file.rows.is_empty() {
            return Err("Le fichier ne contient aucune ligne".into());
        }
        let (table, statements, title) = match self.target {
            Target::Existing => {
                let (table, cols) = self.existing.as_ref().ok_or("Aucune table sélectionnée")?;
                if self.mapping.iter().all(Option::is_none) {
                    return Err("Aucune colonne du fichier ne correspond à la table".into());
                }
                let names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
                let inserts = insert_statements(c, table, &self.file, &self.mapping, &names);
                (table.clone(), inserts, format!("Importer {} ligne(s) dans {table}", self.file.rows.len()))
            }
            Target::New => {
                let name = self.new_name.trim();
                if name.is_empty() {
                    return Err("Nom de la nouvelle table manquant".into());
                }
                let table = TableRef { schema: self.new_schema.clone(), name: name.into() };
                let defs: Vec<ColumnDef> = self
                    .file
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, col)| ColumnDef {
                        name: normalize(col),
                        data_type: infer_type(i, &self.file.rows, self.engine).into(),
                        nullable: true,
                        primary_key: false,
                    })
                    .collect();
                let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
                let mapping: Vec<Option<usize>> = (0..names.len()).map(Some).collect();
                let mut statements = vec![c.create_table_sql(&table, &defs)];
                statements.extend(insert_statements(c, &table, &self.file, &mapping, &names));
                (table.clone(), statements, format!("Créer {table} et importer {} ligne(s)", self.file.rows.len()))
            }
        };
        let mut iter = statements.into_iter();
        let sql = iter.next().ok_or("Rien à importer")?;
        Ok(Pending { title, sql, refresh: Refresh::Structure, select: Some(table), extra: iter.collect() })
    }
}

pub fn view<'a>(form: &'a ImportForm, error: Option<&'a str>) -> Element<'a, Message> {
    let msg = |m: ImportMessage| Message::Import(m);
    let label = |s: &'static str| text(s).size(12).color(theme::MUTED);

    let summary = text(format!(
        "{} — {} ligne(s), {} colonne(s)",
        form.path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
        form.file.rows.len(),
        form.file.columns.len()
    ))
    .size(13)
    .color(theme::TEXT);

    let choice = |label_text: String, target: Target, enabled: bool| {
        button(text(label_text).size(13))
            .on_press_maybe(enabled.then_some(msg(ImportMessage::Target(target.clone()))))
            .style(theme::list_item(form.target == target))
            .padding([4, 10])
    };
    let targets = row![
        choice(
            match &form.existing {
                Some((t, _)) => format!("Dans la table {t}"),
                None => "Dans la table sélectionnée (aucune)".into(),
            },
            Target::Existing,
            form.existing.is_some(),
        ),
        choice("Créer une nouvelle table".into(), Target::New, true),
    ]
    .spacing(6);

    let body: Element<'_, Message> = match form.target {
        Target::Existing => {
            let cols: Vec<String> = form.existing.as_ref().map(|(_, c)| c.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
            let lines = form.file.columns.iter().enumerate().map(|(i, fc)| {
                let mut options = vec![IGNORE.to_string()];
                options.extend(cols.iter().cloned());
                let selected = form.mapping.get(i).copied().flatten().and_then(|t| cols.get(t).cloned()).unwrap_or_else(|| IGNORE.into());
                row![
                    text(fc).size(12).font(MONO).width(Fill),
                    text("→").size(12).color(theme::MUTED),
                    pick_list(options, Some(selected), move |v| Message::Import(ImportMessage::Map(i, v)))
                        .text_size(12)
                        .padding(4)
                        .width(220),
                ]
                .spacing(8)
                .align_y(iced::Center)
                .into()
            });
            column![label("Correspondance des colonnes (fichier → table)"), Column::with_children(lines).spacing(4)]
                .spacing(6)
                .into()
        }
        Target::New => {
            let types = form.file.columns.iter().enumerate().map(|(i, c)| {
                row![
                    text(normalize(c)).size(12).font(MONO).width(Fill),
                    text(infer_type(i, &form.file.rows, form.engine)).size(12).color(theme::MUTED),
                ]
                .spacing(8)
                .into()
            });
            column![
                row![
                    label("Nom de la nouvelle table"),
                    text_input("", &form.new_name)
                        .on_input(|v| Message::Import(ImportMessage::NewName(v)))
                        .font(MONO)
                        .size(13)
                        .padding(6)
                        .width(260),
                ]
                .spacing(10)
                .align_y(iced::Center),
                label("Colonnes et types déduits du contenu"),
                Column::with_children(types).spacing(2),
            ]
            .spacing(8)
            .into()
        }
    };

    let preview_rows = form.file.rows.iter().take(PREVIEW_ROWS).map(|r| {
        text(r.iter().map(|c| c.display()).collect::<Vec<_>>().join(" | ")).size(11).font(MONO).wrapping(text::Wrapping::None).into()
    });
    let preview = column![
        label("Aperçu"),
        text(form.file.columns.join(" | ")).size(11).font(MONO).color(theme::PRIMARY).wrapping(text::Wrapping::None),
        Column::with_children(preview_rows).spacing(1),
    ]
    .spacing(4);

    let actions = row![
        button(text("Voir le SQL").size(13)).on_press(msg(ImportMessage::Submit)).padding([6, 14]),
        button(text("Annuler").size(13)).on_press(msg(ImportMessage::Cancel)).style(button::secondary).padding([6, 14]),
        text(error.unwrap_or_default()).size(13).color(theme::DANGER),
    ]
    .spacing(8)
    .align_y(iced::Center);

    let content = column![
        text("Importer un fichier").size(18).color(theme::TEXT),
        summary,
        targets,
        scrollable(column![body, preview].spacing(14)).height(Fill),
        actions,
    ]
    .spacing(12);

    let panel = container(content).style(theme::panel).padding(20).width(760).max_height(640);
    iced::widget::opaque(iced::widget::center(iced::widget::opaque(panel)).style(|_| container::Style {
        background: Some(iced::Background::Color(iced::Color { a: 0.7, ..iced::Color::BLACK })),
        ..container::Style::default()
    }))
}

/// Boîte de dialogue d'ouverture, puis lecture du fichier.
pub async fn pick_and_read() -> Result<Option<(PathBuf, ImportedFile)>, String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .add_filter("Fichiers de données", &["csv", "tsv", "txt", "xlsx", "xlsm", "xls", "ods", "parquet"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = handle.path().to_path_buf();
    let p = path.clone();
    let file = tokio::task::spawn_blocking(move || read_file(&p)).await.map_err(|e| e.to_string())??;
    Ok(Some((path, file)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_typed_from_text() {
        assert!(matches!(parse_cell("42"), Cell::Int(42)));
        assert!(matches!(parse_cell("3,5"), Cell::Float(f) if (f - 3.5).abs() < 1e-9));
        assert!(matches!(parse_cell("vrai"), Cell::Bool(true)));
        assert!(matches!(parse_cell(""), Cell::Null));
        assert!(matches!(parse_cell("Ada"), Cell::Text(_)));
    }

    #[test]
    fn csv_delimiter_is_detected_and_types_inferred() {
        let dir = std::env::temp_dir().join(format!("quarry-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clients.csv");
        std::fs::write(&path, "Nom;Âge;Score\nAda;36;9.5\nLinus;;7\n").unwrap();
        let file = read_csv(&path).unwrap();
        assert_eq!(file.columns, ["Nom", "Âge", "Score"]);
        assert_eq!(file.rows.len(), 2);
        assert!(matches!(file.rows[1][1], Cell::Null));
        assert_eq!(infer_type(0, &file.rows, Engine::Postgres), "TEXT");
        assert_eq!(infer_type(1, &file.rows, Engine::Postgres), "BIGINT");
        assert_eq!(infer_type(2, &file.rows, Engine::Postgres), "DOUBLE PRECISION");
        assert_eq!(normalize("Âge (ans)"), "âge_ans");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn columns_map_by_normalized_name() {
        let col = |n: &str| ColumnInfo { name: n.into(), data_type: "text".into(), nullable: true, default: None, primary_key: false };
        let table = vec![col("id"), col("full_name"), col("score")];
        let mapping = auto_mapping(&["Full Name".into(), "SCORE".into(), "autre".into()], &table);
        assert_eq!(mapping, vec![Some(1), Some(2), None]);
    }
}
