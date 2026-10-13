//! Formulaires d'édition (lignes, tables, colonnes) et fenêtre de
//! confirmation. Chaque formulaire produit du SQL que l'utilisateur voit et
//! valide avant exécution : rien n'est exécuté à son insu.

use iced::widget::{
    Column, button, center, checkbox, column, container, opaque, row, scrollable, text,
    text_input,
};
use iced::{Background, Color, Element, Fill, Font};

use iced::widget::pick_list;

use crate::app::Message;
use crate::connector::{Alter, Cell, ColumnDef, ColumnInfo, Connector, Dialect, Input, TableRef};
use crate::theme;

const MONO: Font = Font::MONOSPACE;
/// Entrée du menu des types qui rend la saisie libre.
const CUSTOM_TYPE: &str = "Personnalisé…";

/// Dialecte par défaut (SQL standard) quand aucune connexion n'est ouverte.
pub fn default_dialect() -> Dialect {
    Dialect {
        identifier_quote: '"',
        backslash_escapes: false,
        has_schemas: true,
        rename_table_statement: false,
        schema_is_database: false,
        types: ["INTEGER", "BIGINT", "NUMERIC", "REAL", "BOOLEAN", "TEXT", "VARCHAR(255)", "DATE", "TIMESTAMP"]
            .map(String::from)
            .to_vec(),
        id_type: "INTEGER".into(),
        import_types: ["BIGINT".into(), "DOUBLE PRECISION".into(), "BOOLEAN".into(), "TEXT".into()],
        create_table_suffix: None,
        preview_sql: None,
        rename_table_sql: None,
        rename_column_sql: None,
    }
}

/// Ce qu'il faut recharger une fois le SQL exécuté.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Liste des tables et schéma (DDL).
    Structure,
    /// Lignes de la table affichée (DML).
    Data,
}

#[derive(Debug, Clone)]
pub struct Pending {
    pub title: String,
    pub sql: String,
    pub refresh: Refresh,
    /// Table à sélectionner une fois le SQL exécuté (créée, renommée…).
    pub select: Option<TableRef>,
    /// Instructions exécutées à la suite de `sql` (import par lots).
    pub extra: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FieldState {
    pub text: String,
    pub null: bool,
}

/// Insertion ou modification d'une ligne.
#[derive(Debug, Clone)]
pub struct RowForm {
    pub table: TableRef,
    pub columns: Vec<ColumnInfo>,
    pub fields: Vec<FieldState>,
    /// Ligne d'origine en cas de modification (clé + détection des changements).
    pub original: Option<Vec<Cell>>,
}

impl RowForm {
    pub fn insert(table: TableRef, columns: Vec<ColumnInfo>) -> Self {
        let fields = columns.iter().map(|_| FieldState { text: String::new(), null: false }).collect();
        Self { table, columns, fields, original: None }
    }

    pub fn update(table: TableRef, columns: Vec<ColumnInfo>, row: &[Cell]) -> Self {
        let fields = columns
            .iter()
            .enumerate()
            .map(|(i, _)| match row.get(i) {
                Some(Cell::Null) | None => FieldState { text: String::new(), null: true },
                Some(cell) => FieldState { text: cell.display(), null: false },
            })
            .collect();
        Self { table, columns, fields, original: Some(row.to_vec()) }
    }

    fn input(&self, i: usize) -> Input {
        let field = &self.fields[i];
        let column = &self.columns[i];
        if field.null {
            Input::Null
        } else if field.text.is_empty() && self.original.is_none() {
            // Insertion : vide = valeur par défaut, ou NULL si la colonne l'accepte.
            if column.default.is_some() || !column.nullable { Input::Default } else { Input::Null }
        } else {
            Input::Text(field.text.clone())
        }
    }

    /// Clé primaire et valeurs d'origine, pour cibler la ligne.
    pub fn key(&self) -> Vec<(String, Cell)> {
        let Some(original) = &self.original else { return Vec::new() };
        self.columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.primary_key)
            .filter_map(|(i, c)| Some((c.name.clone(), original.get(i)?.clone())))
            .collect()
    }

    pub fn to_sql(&self, c: &dyn Connector) -> Result<Pending, String> {
        match &self.original {
            None => {
                let values: Vec<_> = self
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, col)| (col.name.clone(), self.input(i)))
                    .filter(|(_, v)| *v != Input::Default)
                    .collect();
                if values.is_empty() {
                    return Err("Renseigne au moins une colonne".into());
                }
                Ok(Pending {
                    title: format!("Insérer une ligne dans {}", self.table),
                    sql: c.insert_sql(&self.table, &values),
                    refresh: Refresh::Data,
                    select: Some(self.table.clone()),
                    extra: Vec::new(),
                })
            }
            Some(original) => {
                let key = self.key();
                if key.is_empty() {
                    return Err("Cette table n'a pas de clé primaire : modification impossible".into());
                }
                // Seules les colonnes modifiées entrent dans le SET.
                let changed: Vec<_> = self
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        let before = original.get(*i).cloned().unwrap_or(Cell::Null);
                        let was_null = matches!(before, Cell::Null);
                        let f = &self.fields[*i];
                        f.null != was_null || (!f.null && f.text != before.display())
                    })
                    .map(|(i, col)| (col.name.clone(), self.input(i)))
                    .collect();
                if changed.is_empty() {
                    return Err("Aucune valeur modifiée".into());
                }
                Ok(Pending {
                    title: format!("Modifier une ligne de {}", self.table),
                    sql: c.update_sql(&self.table, &changed, &key),
                    refresh: Refresh::Data,
                    select: Some(self.table.clone()),
                    extra: Vec::new(),
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColumnDraft {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    /// Type saisi librement plutôt que choisi dans le menu.
    pub custom_type: bool,
}

impl ColumnDraft {
    pub fn new() -> Self {
        Self { name: String::new(), data_type: String::new(), nullable: true, primary_key: false, custom_type: false }
    }

    fn to_def(&self) -> Result<ColumnDef, String> {
        if self.name.trim().is_empty() {
            return Err("Nom de colonne manquant".into());
        }
        if self.data_type.trim().is_empty() {
            return Err(format!("Type manquant pour « {} »", self.name.trim()));
        }
        Ok(ColumnDef {
            name: self.name.trim().into(),
            data_type: self.data_type.trim().into(),
            nullable: self.nullable && !self.primary_key,
            primary_key: self.primary_key,
        })
    }
}

/// Création d'une table.
#[derive(Debug, Clone)]
pub struct TableForm {
    /// Schémas existants dans la base (vide : le moteur n'en a pas).
    pub schemas: Vec<String>,
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnDraft>,
}

impl TableForm {
    pub fn new(schemas: Vec<String>, id_type: &str) -> Self {
        // `public` d'abord si présent, sinon le premier schéma.
        let schema = schemas
            .iter()
            .find(|s| *s == "public")
            .or(schemas.first())
            .cloned()
            .unwrap_or_default();
        let id = ColumnDraft { name: "id".into(), data_type: id_type.into(), ..ColumnDraft::new() };
        let id = ColumnDraft { nullable: false, primary_key: true, ..id };
        Self { schemas, schema, name: String::new(), columns: vec![id, ColumnDraft::new()] }
    }

    pub fn to_sql(&self, c: &dyn Connector) -> Result<Pending, String> {
        if self.name.trim().is_empty() {
            return Err("Nom de table manquant".into());
        }
        let columns: Vec<_> = self
            .columns
            .iter()
            .filter(|c| !(c.name.trim().is_empty() && c.data_type.trim().is_empty()))
            .map(ColumnDraft::to_def)
            .collect::<Result<_, _>>()?;
        if columns.is_empty() {
            return Err("Ajoute au moins une colonne".into());
        }
        let table = TableRef {
            schema: (!self.schema.trim().is_empty()).then(|| self.schema.trim().into()),
            name: self.name.trim().into(),
        };
        Ok(Pending {
            title: format!("Créer la table {table}"),
            sql: c.create_table_sql(&table, &columns),
            refresh: Refresh::Structure,
            select: Some(table),
            extra: Vec::new(),
        })
    }
}

/// Ajout d'une colonne à une table existante.
#[derive(Debug, Clone)]
pub struct ColumnForm {
    pub table: TableRef,
    pub draft: ColumnDraft,
}

impl ColumnForm {
    pub fn to_sql(&self, c: &dyn Connector) -> Result<Pending, String> {
        let def = self.draft.to_def()?;
        Ok(Pending {
            title: format!("Ajouter une colonne à {}", self.table),
            sql: c.alter_table_sql(&self.table, &Alter::AddColumn(def)),
            refresh: Refresh::Structure,
            select: Some(self.table.clone()),
            extra: Vec::new(),
        })
    }
}

/// Renommage d'une table ou d'une colonne.
#[derive(Debug, Clone)]
pub struct RenameForm {
    pub table: TableRef,
    /// `Some` : on renomme cette colonne ; `None` : la table.
    pub column: Option<String>,
    pub to: String,
}

impl RenameForm {
    pub fn to_sql(&self, c: &dyn Connector) -> Result<Pending, String> {
        let to = self.to.trim();
        if to.is_empty() {
            return Err("Nouveau nom manquant".into());
        }
        let (title, alter, select) = match &self.column {
            Some(column) => (
                format!("Renommer la colonne {column}"),
                Alter::RenameColumn { from: column.clone(), to: to.into() },
                self.table.clone(),
            ),
            None => (
                format!("Renommer la table {}", self.table),
                Alter::RenameTable(to.into()),
                TableRef { schema: self.table.schema.clone(), name: to.into() },
            ),
        };
        Ok(Pending {
            title,
            sql: c.alter_table_sql(&self.table, &alter),
            refresh: Refresh::Structure,
            select: Some(select),
            extra: Vec::new(),
        })
    }
}

/// Création d'un schéma (ou base MySQL, dataset BigQuery).
#[derive(Debug, Clone)]
pub struct SchemaForm {
    pub name: String,
}

impl SchemaForm {
    pub fn to_sql(&self, c: &dyn Connector) -> Result<Pending, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Nom du schéma manquant".into());
        }
        let sql = c.create_schema_sql(name).ok_or("Ce moteur n'a pas de schémas")?;
        Ok(Pending {
            title: format!("Créer le schéma {name}"),
            sql,
            refresh: Refresh::Structure,
            select: None,
            extra: Vec::new(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum Dialog {
    Schema(SchemaForm),
    Row(RowForm),
    Table(TableForm),
    Column(ColumnForm),
    Rename(RenameForm),
    Confirm(Pending),
}

/// Messages des formulaires, relayés par [`Message::Dialog`].
#[derive(Debug, Clone)]
pub enum DialogMessage {
    Field(usize, String),
    ToggleNull(usize),
    TableSchema(String),
    TableName(String),
    ColumnName(usize, String),
    ColumnType(usize, String),
    ColumnNullable(usize, bool),
    ColumnPrimary(usize, bool),
    /// Bascule entre le menu de types et la saisie libre.
    ColumnCustomType(usize),
    AddColumnRow,
    RemoveColumnRow(usize),
    RenameTo(String),
    SchemaName(String),
    /// Génère le SQL et passe à la confirmation.
    Submit,
    /// Exécute le SQL confirmé.
    Execute,
    Cancel,
}

impl Dialog {
    pub fn apply(&mut self, message: DialogMessage) {
        use DialogMessage as M;
        match (self, message) {
            (Dialog::Row(f), M::Field(i, v)) => {
                if let Some(field) = f.fields.get_mut(i) {
                    field.text = v;
                    field.null = false;
                }
            }
            (Dialog::Row(f), M::ToggleNull(i)) => {
                if let Some(field) = f.fields.get_mut(i) {
                    field.null = !field.null;
                }
            }
            (Dialog::Table(f), M::TableSchema(v)) => f.schema = v,
            (Dialog::Table(f), M::TableName(v)) => f.name = v,
            (Dialog::Table(f), M::ColumnName(i, v)) => set_column(&mut f.columns, i, |c| c.name = v),
            (Dialog::Table(f), M::ColumnType(i, v)) => set_column(&mut f.columns, i, |c| c.data_type = v),
            (Dialog::Table(f), M::ColumnNullable(i, v)) => set_column(&mut f.columns, i, |c| c.nullable = v),
            (Dialog::Table(f), M::ColumnPrimary(i, v)) => set_column(&mut f.columns, i, |c| c.primary_key = v),
            (Dialog::Table(f), M::ColumnCustomType(i)) => set_column(&mut f.columns, i, |c| c.custom_type = !c.custom_type),
            (Dialog::Column(f), M::ColumnCustomType(_)) => f.draft.custom_type = !f.draft.custom_type,
            (Dialog::Table(f), M::AddColumnRow) => f.columns.push(ColumnDraft::new()),
            (Dialog::Table(f), M::RemoveColumnRow(i)) => {
                if i < f.columns.len() && f.columns.len() > 1 {
                    f.columns.remove(i);
                }
            }
            (Dialog::Column(f), M::ColumnName(_, v)) => f.draft.name = v,
            (Dialog::Column(f), M::ColumnType(_, v)) => f.draft.data_type = v,
            (Dialog::Column(f), M::ColumnNullable(_, v)) => f.draft.nullable = v,
            (Dialog::Rename(f), M::RenameTo(v)) => f.to = v,
            (Dialog::Schema(f), M::SchemaName(v)) => f.name = v,
            _ => {}
        }
    }

    pub fn to_sql(&self, c: &dyn Connector) -> Result<Pending, String> {
        match self {
            Dialog::Schema(f) => f.to_sql(c),
            Dialog::Row(f) => f.to_sql(c),
            Dialog::Table(f) => f.to_sql(c),
            Dialog::Column(f) => f.to_sql(c),
            Dialog::Rename(f) => f.to_sql(c),
            Dialog::Confirm(p) => Ok(p.clone()),
        }
    }
}

fn set_column(columns: &mut [ColumnDraft], i: usize, f: impl FnOnce(&mut ColumnDraft)) {
    if let Some(c) = columns.get_mut(i) {
        f(c);
    }
}

// ----------------------------------------------------------------------
// Vues
// ----------------------------------------------------------------------

pub fn view<'a>(dialog: &'a Dialog, error: Option<&'a str>, dialect: &'a Dialect) -> Element<'a, Message> {
    let (title, body, submit_label): (String, Element<'a, Message>, &str) = match dialog {
        Dialog::Row(f) => (
            match &f.original {
                None => format!("Nouvelle ligne dans {}", f.table),
                Some(_) => format!("Modifier une ligne de {}", f.table),
            },
            row_form(f),
            "Voir le SQL",
        ),
        Dialog::Table(f) => ("Nouvelle table".into(), table_form(f, dialect), "Voir le SQL"),
        Dialog::Column(f) => (format!("Ajouter une colonne à {}", f.table), column_form(f, dialect), "Voir le SQL"),
        Dialog::Rename(f) => (
            match &f.column {
                Some(c) => format!("Renommer la colonne {c}"),
                None => format!("Renommer la table {}", f.table),
            },
            rename_form(f),
            "Voir le SQL",
        ),
        Dialog::Schema(f) => ("Nouveau schéma".into(), schema_form(f), "Voir le SQL"),
        Dialog::Confirm(p) => (p.title.clone(), confirm_view(p), "Exécuter"),
    };

    let submit = match dialog {
        Dialog::Confirm(_) => DialogMessage::Execute,
        _ => DialogMessage::Submit,
    };

    let feedback = text(error.unwrap_or_default()).size(13).color(theme::DANGER);

    let actions = row![
        button(text(submit_label).size(13)).on_press(Message::Dialog(submit)).padding([6, 14]),
        button(text("Annuler").size(13))
            .on_press(Message::Dialog(DialogMessage::Cancel))
            .style(button::secondary)
            .padding([6, 14]),
        feedback,
    ]
    .spacing(8)
    .align_y(iced::Center);

    let content = column![
        text(title).size(18).color(theme::TEXT),
        scrollable(body).height(Fill),
        actions,
    ]
    .spacing(14);

    let panel = container(content).style(theme::panel).padding(20).width(720).max_height(620);

    opaque(center(opaque(panel)).style(|_| container::Style {
        background: Some(Background::Color(Color { a: 0.7, ..Color::BLACK })),
        ..container::Style::default()
    }))
}

fn label<'a>(s: impl ToString) -> Element<'a, Message> {
    text(s.to_string()).size(12).color(theme::MUTED).into()
}

fn row_form(f: &RowForm) -> Element<'_, Message> {
    let is_insert = f.original.is_none();
    let rows = f.columns.iter().zip(&f.fields).enumerate().map(|(i, (col, field))| {
        let mut hint = col.data_type.clone();
        if col.primary_key {
            hint.push_str(" · PK");
        }
        if let Some(d) = &col.default {
            hint.push_str(&format!(" · défaut {d}"));
        }
        let placeholder = if field.null {
            "NULL"
        } else if is_insert && col.default.is_some() {
            "(valeur par défaut)"
        } else if is_insert && col.nullable {
            "(NULL)"
        } else {
            ""
        };
        let input = text_input(placeholder, &field.text)
            .on_input(move |v| Message::Dialog(DialogMessage::Field(i, v)))
            .font(MONO)
            .size(13)
            .padding(6);
        let null = button(text("NULL").size(11))
            .on_press_maybe(col.nullable.then_some(Message::Dialog(DialogMessage::ToggleNull(i))))
            .style(theme::list_item(field.null))
            .padding([4, 8]);
        row![
            column![text(&col.name).size(13).font(MONO), label(hint)].spacing(2).width(220),
            input,
            null,
        ]
        .spacing(10)
        .align_y(iced::Center)
        .into()
    });
    column![
        label("Vide = valeur par défaut de la colonne (ou NULL si elle l'accepte). Les colonnes non modifiées sont laissées telles quelles."),
        Column::with_children(rows).spacing(8),
    ]
    .spacing(12)
    .into()
}

/// Menu des types, ou champ libre si la colonne est en mode personnalisé.
fn type_picker<'a>(i: usize, c: &'a ColumnDraft, dialect: &'a Dialect) -> Element<'a, Message> {
    if c.custom_type {
        return row![
            text_input("VARCHAR(100), NUMERIC(10,2)…", &c.data_type)
                .on_input(move |v| Message::Dialog(DialogMessage::ColumnType(i, v)))
                .font(MONO)
                .size(13)
                .padding(6)
                .width(Fill),
            button(text("Menu").size(11))
                .on_press(Message::Dialog(DialogMessage::ColumnCustomType(i)))
                .style(button::text)
                .padding([4, 6]),
        ]
        .spacing(4)
        .align_y(iced::Center)
        .into();
    }

    let mut options: Vec<String> = dialect.types.clone();
    // Un type venu d'ailleurs (table existante, saisie libre) reste sélectionnable.
    if !c.data_type.is_empty() && !options.contains(&c.data_type) {
        options.insert(0, c.data_type.clone());
    }
    options.push(CUSTOM_TYPE.into());
    let selected = (!c.data_type.is_empty()).then(|| c.data_type.clone());

    pick_list(options, selected, move |choice: String| {
        if choice == CUSTOM_TYPE {
            Message::Dialog(DialogMessage::ColumnCustomType(i))
        } else {
            Message::Dialog(DialogMessage::ColumnType(i, choice))
        }
    })
    .placeholder("Type")
    .font(MONO)
    .text_size(13)
    .padding(6)
    .width(Fill)
    .into()
}

fn column_rows<'a>(columns: &'a [ColumnDraft], removable: bool, dialect: &'a Dialect) -> Element<'a, Message> {
    let header = row![
        container(label("Nom")).width(Fill),
        container(label("Type")).width(Fill),
        container(label("NULL")).width(60),
        container(label("PK")).width(50),
        container(text("")).width(if removable { 30.0 } else { 0.0 }),
    ]
    .spacing(8);
    let rows = columns.iter().enumerate().map(|(i, c)| {
        let mut r = row![
            text_input("nom", &c.name)
                .on_input(move |v| Message::Dialog(DialogMessage::ColumnName(i, v)))
                .font(MONO)
                .size(13)
                .padding(6)
                .width(Fill),
            container(type_picker(i, c, dialect)).width(Fill),
            container(
                checkbox(c.nullable && !c.primary_key)
                    .on_toggle(move |v| Message::Dialog(DialogMessage::ColumnNullable(i, v)))
            )
            .width(60),
            container(
                checkbox(c.primary_key)
                    .on_toggle(move |v| Message::Dialog(DialogMessage::ColumnPrimary(i, v)))
            )
            .width(50),
        ]
        .spacing(8)
        .align_y(iced::Center);
        if removable {
            r = r.push(
                button(text("×").size(13))
                    .on_press(Message::Dialog(DialogMessage::RemoveColumnRow(i)))
                    .style(button::text)
                    .padding([2, 8]),
            );
        }
        r.into()
    });
    column![header, Column::with_children(rows).spacing(6)].spacing(6).into()
}

fn table_form<'a>(f: &'a TableForm, dialect: &'a Dialect) -> Element<'a, Message> {
    let schema_field: Element<'_, Message> = if f.schemas.is_empty() {
        column![].into()
    } else {
        column![
            label("Schéma"),
            pick_list(f.schemas.clone(), Some(f.schema.clone()), |v| Message::Dialog(DialogMessage::TableSchema(v)))
                .font(MONO)
                .text_size(13)
                .padding(6)
                .width(Fill),
        ]
        .spacing(4)
        .width(200)
        .into()
    };
    column![
        row![
            schema_field,
            column![
                label("Nom de la table"),
                text_input("clients", &f.name)
                    .on_input(|v| Message::Dialog(DialogMessage::TableName(v)))
                    .font(MONO)
                    .size(13)
                    .padding(6),
            ]
            .spacing(4)
            .width(Fill),
        ]
        .spacing(12),
        label("Colonnes — choisis le type dans le menu, ou « Personnalisé… » pour le saisir."),
        column_rows(&f.columns, true, dialect),
        button(text("+ Colonne").size(13))
            .on_press(Message::Dialog(DialogMessage::AddColumnRow))
            .style(button::secondary)
            .padding([4, 10]),
    ]
    .spacing(12)
    .into()
}

fn column_form<'a>(f: &'a ColumnForm, dialect: &'a Dialect) -> Element<'a, Message> {
    column![
        label("Une nouvelle colonne doit accepter NULL ou avoir une valeur par défaut si la table contient déjà des lignes."),
        column_rows(std::slice::from_ref(&f.draft), false, dialect),
    ]
    .spacing(12)
    .into()
}

fn schema_form(f: &SchemaForm) -> Element<'_, Message> {
    column![
        label("Nom du schéma (base pour MySQL, dataset pour BigQuery)"),
        text_input("ventes", &f.name)
            .on_input(|v| Message::Dialog(DialogMessage::SchemaName(v)))
            .font(MONO)
            .size(13)
            .padding(6),
    ]
    .spacing(4)
    .into()
}

fn rename_form(f: &RenameForm) -> Element<'_, Message> {
    column![
        label("Nouveau nom"),
        text_input("", &f.to)
            .on_input(|v| Message::Dialog(DialogMessage::RenameTo(v)))
            .font(MONO)
            .size(13)
            .padding(6),
    ]
    .spacing(4)
    .into()
}

fn confirm_view(p: &Pending) -> Element<'_, Message> {
    let warning = if p.sql.starts_with("DROP") || p.sql.starts_with("DELETE") {
        Some(text("Cette action est irréversible.").size(13).color(theme::DANGER))
    } else {
        None
    };
    let shown: String = p.sql.chars().take(4000).collect();
    let truncated = p.sql.chars().count() > 4000;
    let mut content = column![
        label("Voici le SQL qui sera exécuté :"),
        container(text(format!("{shown}{}", if truncated { "\n…" } else { "" })).font(MONO).size(13))
            .style(theme::header_cell)
            .padding(12)
            .width(Fill),
    ]
    .spacing(10);
    if !p.extra.is_empty() {
        content = content.push(
            text(format!("… suivi de {} autre(s) instruction(s) du même type.", p.extra.len())).size(12).color(theme::MUTED),
        );
    }
    if let Some(w) = warning {
        content = content.push(w);
    }
    content.into()
}
