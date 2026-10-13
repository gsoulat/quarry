//! État de l'application, messages et vues (architecture Elm d'iced).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::text_editor::{Action, Binding, Edit, KeyPress};
use iced::widget::{
    Column, Row, button, canvas, column, container, pane_grid, row, scrollable, stack, text,
    text_editor, text_input, tooltip,
};
use iced::{Color, Element, Event, Fill, Font, Length, Subscription, Task, Theme, highlighter, padding};

use crate::connector::{
    self, ColumnInfo, Connector, QueryPlan, QueryResult, Relation, Result, TableRef,
    execution_order,
};
use crate::diagram::{self, Diagram};
use crate::editing::{self, Dialog, DialogMessage, Refresh, RenameForm, RowForm, TableForm};
use crate::manager::{self, Feedback, Field, Manager};
use crate::store::{self, ConnectionConfig, Engine, Location};
use crate::theme;

const MONO: Font = Font::MONOSPACE;
/// Largeur approximative d'un caractère en police mono de 13 px.
const CHAR_WIDTH: f32 = 7.8;
const MIN_CELL: f32 = 70.0;
const MAX_CELL: f32 = 420.0;
const PAGE_SIZE: usize = 200;
const SIDEBAR_RATIO: f32 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Results,
    Data,
    Plan,
    Schema,
    Diagram,
}

/// Que fait un clic sur une cellule de la grille.
enum CellAction {
    Copy,
    SelectRow(fn(usize) -> Message),
}

#[derive(Debug, Clone, Copy)]
enum Pane {
    Sidebar,
    Workspace,
}

#[derive(Debug, Clone)]
enum Status {
    Idle,
    Busy(String),
    Info(String),
    Error(String),
}

/// La connexion ouverte et ce qu'on en a chargé.
struct Session {
    id: u64,
    connector: Arc<dyn Connector>,
    tables: Vec<TableRef>,
}

pub struct App {
    connections: Vec<ConnectionConfig>,
    session: Option<Session>,
    /// Connexion en cours d'ouverture (pour l'indicateur dans la barre latérale).
    connecting: Option<u64>,
    manager: Option<Manager>,
    panes: pane_grid::State<Pane>,
    /// Filtre de la liste des tables.
    filter: String,
    /// Schémas repliés dans la barre latérale.
    collapsed: HashSet<String>,
    /// Connexions dont la liste de tables est repliée.
    folded: HashSet<u64>,
    diagram: Option<Diagram>,
    diagram_loading: bool,
    /// Lignes de la table sélectionnée (onglet Données).
    data: Option<QueryResult>,
    data_table: Option<TableRef>,
    data_row: Option<usize>,
    /// Colonne sélectionnée dans l'onglet Schéma.
    schema_row: Option<usize>,
    dialog: Option<Dialog>,
    dialog_error: Option<String>,
    /// Menu « Édition » déroulé.
    edit_menu: bool,
    selected: Option<TableRef>,
    schema: Vec<ColumnInfo>,
    editor: text_editor::Content,
    result: Option<QueryResult>,
    /// Requête qui a produit `result`, pour ne pas confondre avec l'éditeur.
    result_sql: Option<String>,
    page: usize,
    plan: Option<QueryPlan>,
    plan_sql: Option<String>,
    tab: Tab,
    status: Status,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Barre latérale
    Connect(u64),
    Connected(u64, Result<Arc<dyn Connector>>),
    Disconnect,
    TablesLoaded(Result<Vec<TableRef>>),
    TableSelected(TableRef),
    InsertTable(TableRef),
    SchemaLoaded(Result<Vec<ColumnInfo>>),
    FilterChanged(String),
    ToggleSchema(String),
    ToggleFold(u64),
    ToggleFolder(String),
    PaneResized(pane_grid::ResizeEvent),

    // Édition
    LoadData,
    DataLoaded(TableRef, Result<QueryResult>),
    DataRowSelected(usize),
    SchemaRowSelected(usize),
    OpenDialog(Dialog),
    Dialog(DialogMessage),
    Executed(Refresh, Option<TableRef>, Result<u64>),
    ToggleEditMenu,
    /// Absorbe un clic (par exemple dans le menu) sans rien faire.
    Noop,

    // Diagramme
    LoadDiagram,
    DiagramLoaded(Result<(Vec<(TableRef, Vec<ColumnInfo>)>, Vec<Relation>)>),
    DiagramMode(diagram::Mode),
    DiagramLayout,
    DiagramFit,
    DiagramInteraction(diagram::Interaction),

    // Gestionnaire de connexions
    OpenManager,
    CloseManager,
    EditConnection(Option<u64>),
    FormChanged(Field, String),
    FormEngine(Engine),
    TogglePassword,
    TestConnection,
    TestDone(Result<String>),
    SaveConnection { connect: bool },
    DeleteConnection,

    // Espace de travail
    Edit(Action),
    Explain,
    Explained(String, Result<QueryPlan>),
    Run,
    QueryDone(String, Result<QueryResult>, Duration),
    TabSelected(Tab),
    PageChanged(usize),
    CopyCell(String),
    KeyPressed(Key, Modifiers),
}

/// Ouvre une connexion : mot de passe depuis le trousseau, puis connecteur.
async fn open(config: ConnectionConfig, typed_password: Option<String>) -> Result<Arc<dyn Connector>> {
    let password = match typed_password {
        Some(password) => Some(password),
        None if config.engine.is_server() => store::password(config.id)?,
        None => None,
    };
    connector::connect(&config, password).await
}

impl App {
    pub fn new() -> Self {
        crate::icon::set_dock_icon();

        let (connections, status) = match store::load() {
            Ok(connections) => (connections, Status::Idle),
            Err(e) => (Vec::new(), Status::Error(e)),
        };
        // Premier lancement : on ouvre directement le gestionnaire.
        let manager = connections.is_empty().then(Manager::new);

        let panes = pane_grid::State::with_configuration(pane_grid::Configuration::Split {
            axis: pane_grid::Axis::Vertical,
            ratio: SIDEBAR_RATIO,
            a: Box::new(pane_grid::Configuration::Pane(Pane::Sidebar)),
            b: Box::new(pane_grid::Configuration::Pane(Pane::Workspace)),
        });

        Self {
            connections,
            session: None,
            connecting: None,
            manager,
            panes,
            filter: String::new(),
            collapsed: HashSet::new(),
            folded: HashSet::new(),
            diagram: None,
            diagram_loading: false,
            data: None,
            data_table: None,
            data_row: None,
            schema_row: None,
            dialog: None,
            dialog_error: None,
            edit_menu: false,
            selected: None,
            schema: Vec::new(),
            editor: text_editor::Content::with_text("SELECT 1 AS hello"),
            result: None,
            result_sql: None,
            page: 0,
            plan: None,
            plan_sql: None,
            tab: Tab::Results,
            status,
        }
    }

    pub fn title(&self) -> String {
        match self.active_config() {
            Some(c) => format!("Quarry — {}", c.name),
            None => "Quarry".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        theme::quarry()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // Seuls les événements clavier ignorés par les widgets arrivent ici :
        // l'éditeur gère lui-même ses raccourcis via `key_binding`.
        iced::event::listen_with(|event, status, _| match (event, status) {
            (
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }),
                iced::event::Status::Ignored,
            ) => Some(Message::KeyPressed(key, modifiers)),
            _ => None,
        })
    }

    fn active_config(&self) -> Option<&ConnectionConfig> {
        let id = self.session.as_ref()?.id;
        self.connections.iter().find(|c| c.id == id)
    }

    fn connector(&self) -> Option<Arc<dyn Connector>> {
        self.session.as_ref().map(|s| s.connector.clone())
    }

    /// SQL à exécuter : la sélection si elle existe, sinon tout l'éditeur.
    fn current_sql(&self) -> String {
        self.editor
            .selection()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| self.editor.text())
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Connect(id) => {
                let Some(config) = self.connections.iter().find(|c| c.id == id).cloned() else {
                    return Task::none();
                };
                self.connecting = Some(id);
                self.status = Status::Busy(format!("Connexion à {}…", config.name));
                return Task::perform(open(config, None), move |r| Message::Connected(id, r));
            }
            Message::Connected(id, Ok(c)) => {
                self.connecting = None;
                self.status = Status::Info(format!("Connecté ({})", c.engine()));
                self.session = Some(Session { id, connector: c.clone(), tables: Vec::new() });
                self.selected = None;
                self.schema.clear();
                self.collapsed.clear();
                self.folded.remove(&id);
                self.diagram = None;
                self.data = None;
                self.data_table = None;
                return Task::perform(async move { c.list_tables().await }, Message::TablesLoaded);
            }
            Message::Connected(_, Err(e)) => {
                self.connecting = None;
                self.status = Status::Error(e);
            }
            Message::Disconnect => {
                self.session = None;
                self.selected = None;
                self.schema.clear();
                self.diagram = None;
                self.data = None;
                self.data_table = None;
                self.status = Status::Idle;
            }
            Message::TablesLoaded(Ok(tables)) => {
                if let Some(session) = &mut self.session {
                    session.tables = tables;
                }
                // Table sélectionnée encore présente : on relit son schéma
                // (et ses lignes si l'onglet Données est ouvert).
                let still_there = self
                    .selected
                    .as_ref()
                    .is_some_and(|t| self.session.as_ref().is_some_and(|s| s.tables.contains(t)));
                if still_there {
                    return self.update(Message::TableSelected(self.selected.clone().unwrap()));
                }
                self.selected = None;
            }
            Message::TableSelected(table) => {
                let Some(c) = self.connector() else { return Task::none() };
                let same = self.selected.as_ref() == Some(&table);
                if !same {
                    self.editor = text_editor::Content::with_text(&c.preview_sql(&table));
                }
                self.selected = Some(table.clone());
                self.schema_row = None;
                if self.tab != Tab::Data && !same {
                    self.tab = Tab::Schema;
                }
                let describe = Task::perform(
                    async move { c.describe_table(&table).await },
                    Message::SchemaLoaded,
                );
                if self.tab == Tab::Data {
                    return Task::batch([describe, self.update(Message::LoadData)]);
                }
                return describe;
            }
            Message::InsertTable(table) => {
                let Some(c) = self.connector() else { return Task::none() };
                let name = match &table.schema {
                    Some(schema) => format!(
                        "{}.{}",
                        c.quote_identifier(schema),
                        c.quote_identifier(&table.name)
                    ),
                    None => c.quote_identifier(&table.name),
                };
                self.editor.perform(Action::Edit(Edit::Paste(Arc::new(name))));
                return Task::none();
            }
            Message::SchemaLoaded(Ok(columns)) => self.schema = columns,
            Message::FilterChanged(filter) => self.filter = filter,
            Message::ToggleSchema(schema) => {
                if !self.collapsed.remove(&schema) {
                    self.collapsed.insert(schema);
                }
            }
            Message::ToggleFold(id) => {
                if !self.folded.remove(&id) {
                    self.folded.insert(id);
                }
            }
            Message::ToggleFolder(folder) => {
                let key = format!("folder:{folder}");
                if !self.collapsed.remove(&key) {
                    self.collapsed.insert(key);
                }
            }
            Message::PaneResized(event) => self.panes.resize(event.split, event.ratio),

            Message::LoadData => {
                let (Some(c), Some(table)) = (self.connector(), self.selected.clone()) else {
                    return Task::none();
                };
                self.status = Status::Busy(format!("Lecture de {table}…"));
                let sql = c.data_sql(&table);
                let t = table.clone();
                return Task::perform(
                    async move { c.query(&sql).await },
                    move |r| Message::DataLoaded(t.clone(), r),
                );
            }
            Message::DataLoaded(table, Ok(result)) => {
                self.status = Status::Info(format!("{} ligne(s) dans {table}", result.rows.len()));
                self.data = Some(result);
                self.data_table = Some(table);
                self.data_row = None;
            }
            Message::DataLoaded(_, Err(e)) => self.status = Status::Error(e),
            Message::DataRowSelected(i) => {
                self.data_row = if self.data_row == Some(i) { None } else { Some(i) };
            }
            Message::SchemaRowSelected(i) => {
                self.schema_row = if self.schema_row == Some(i) { None } else { Some(i) };
            }
            Message::ToggleEditMenu => self.edit_menu = !self.edit_menu,
            Message::Noop => {}
            Message::OpenDialog(dialog) => {
                self.dialog = Some(dialog);
                self.dialog_error = None;
                self.edit_menu = false;
            }
            Message::Dialog(DialogMessage::Cancel) => {
                self.dialog = None;
                self.dialog_error = None;
            }
            Message::Dialog(DialogMessage::Submit) => {
                let (Some(dialog), Some(c)) = (&self.dialog, self.connector()) else {
                    return Task::none();
                };
                match dialog.to_sql(c.as_ref()) {
                    Ok(pending) => {
                        self.dialog = Some(Dialog::Confirm(pending));
                        self.dialog_error = None;
                    }
                    Err(e) => self.dialog_error = Some(e),
                }
            }
            Message::Dialog(DialogMessage::Execute) => {
                let (Some(Dialog::Confirm(pending)), Some(c)) = (&self.dialog, self.connector()) else {
                    return Task::none();
                };
                let (sql, refresh, select) = (pending.sql.clone(), pending.refresh, pending.select.clone());
                self.dialog = None;
                self.status = Status::Busy("Exécution…".into());
                return Task::perform(
                    async move { c.execute(&sql).await },
                    move |r| Message::Executed(refresh, select.clone(), r),
                );
            }
            Message::Dialog(message) => {
                if let Some(dialog) = &mut self.dialog {
                    dialog.apply(message);
                }
            }
            Message::Executed(refresh, select, Ok(affected)) => {
                self.status = Status::Info(format!("Exécuté · {affected} ligne(s) touchée(s)"));
                let Some(c) = self.connector() else { return Task::none() };
                return match refresh {
                    Refresh::Data => self.update(Message::LoadData),
                    Refresh::Structure => {
                        // On recharge la liste, puis le schéma de la table
                        // concernée (créée, modifiée ou renommée) ; `TablesLoaded`
                        // s'en charge d'après `selected`.
                        self.selected = select;
                        self.schema.clear();
                        self.schema_row = None;
                        self.data = None;
                        self.data_table = None;
                        self.diagram = None;
                        Task::perform(async move { c.list_tables().await }, Message::TablesLoaded)
                    }
                };
            }
            Message::Executed(_, _, Err(e)) => self.status = Status::Error(e),

            Message::LoadDiagram => {
                let Some(session) = &self.session else { return Task::none() };
                let c = session.connector.clone();
                let tables = session.tables.clone();
                self.diagram_loading = true;
                self.status = Status::Busy(format!("Lecture de {} tables…", tables.len()));
                return Task::perform(
                    async move {
                        let described = futures_util::future::try_join_all(tables.into_iter().map(|t| {
                            let c = c.clone();
                            async move { c.describe_table(&t).await.map(|cols| (t, cols)) }
                        }))
                        .await?;
                        let relations = c.list_relations().await?;
                        Ok((described, relations))
                    },
                    Message::DiagramLoaded,
                );
            }
            Message::DiagramLoaded(Ok((tables, relations))) => {
                self.diagram_loading = false;
                self.status = Status::Info(format!(
                    "{} tables, {} relations",
                    tables.len(),
                    relations.len()
                ));
                self.diagram = Some(Diagram::new(tables, relations));
            }
            Message::DiagramLoaded(Err(e)) => {
                self.diagram_loading = false;
                self.status = Status::Error(e);
            }
            Message::DiagramMode(mode) => {
                if let Some(d) = &mut self.diagram {
                    d.set_mode(mode);
                }
            }
            Message::DiagramLayout => {
                if let Some(d) = &mut self.diagram {
                    d.auto_layout();
                }
            }
            Message::DiagramFit => {
                if let Some(d) = &mut self.diagram {
                    d.fit();
                }
            }
            Message::DiagramInteraction(interaction) => {
                if let Some(d) = &mut self.diagram {
                    d.apply(interaction);
                }
            }

            Message::OpenManager => {
                self.manager = Some(match self.active_config() {
                    Some(config) => Manager::edit(config),
                    None => Manager::new(),
                });
            }
            Message::CloseManager => self.manager = None,
            Message::EditConnection(id) => {
                let config = id.and_then(|id| self.connections.iter().find(|c| c.id == id));
                self.manager = Some(config.map_or_else(Manager::new, Manager::edit));
            }
            Message::FormChanged(field, value) => {
                if let Some(m) = &mut self.manager {
                    m.form.set(field, value);
                }
            }
            Message::FormEngine(engine) => {
                if let Some(m) = &mut self.manager {
                    m.form.set_engine(engine);
                }
            }
            Message::TogglePassword => {
                if let Some(m) = &mut self.manager {
                    m.show_password = !m.show_password;
                }
            }
            Message::TestConnection => {
                let Some(m) = &mut self.manager else { return Task::none() };
                let config = match m.form.to_config(m.editing.unwrap_or(0)) {
                    Ok(config) => config,
                    Err(e) => {
                        m.feedback = Some(Feedback::Error(e));
                        return Task::none();
                    }
                };
                let typed = (!m.form.password.is_empty()).then(|| m.form.password.clone());
                m.feedback = Some(Feedback::Busy("Test en cours…".into()));
                return Task::perform(
                    async move {
                        let c = open(config, typed).await?;
                        Ok(format!("Connexion {} réussie", c.engine()))
                    },
                    Message::TestDone,
                );
            }
            Message::TestDone(result) => {
                if let Some(m) = &mut self.manager {
                    m.feedback = Some(match result {
                        Ok(msg) => Feedback::Ok(msg),
                        Err(e) => Feedback::Error(e),
                    });
                }
            }
            Message::SaveConnection { connect } => return self.save_connection(connect),
            Message::DeleteConnection => {
                let Some(id) = self.manager.as_ref().and_then(|m| m.editing) else {
                    return Task::none();
                };
                self.connections.retain(|c| c.id != id);
                if self.session.as_ref().is_some_and(|s| s.id == id) {
                    self.session = None;
                }
                let result = store::delete_password(id).and(store::save(&self.connections));
                let mut manager = Manager::new();
                manager.feedback = Some(match result {
                    Ok(()) => Feedback::Ok("Connexion supprimée".into()),
                    Err(e) => Feedback::Error(e),
                });
                self.manager = Some(manager);
            }

            Message::Edit(action) => self.editor.perform(action),
            Message::Explain => {
                let Some(c) = self.connector() else { return Task::none() };
                self.status = Status::Busy("Analyse du plan…".into());
                let sql = self.current_sql();
                return Task::perform(
                    async move {
                        let plan = c.explain(&sql).await;
                        (sql, plan)
                    },
                    |(sql, plan)| Message::Explained(sql, plan),
                );
            }
            Message::Explained(sql, Ok(plan)) => {
                self.status = Status::Idle;
                self.plan = Some(plan);
                self.plan_sql = Some(sql);
                self.tab = Tab::Plan;
            }
            Message::Run => {
                let Some(c) = self.connector() else { return Task::none() };
                self.status = Status::Busy("Exécution…".into());
                let sql = self.current_sql();
                return Task::perform(
                    async move {
                        let start = Instant::now();
                        let result = c.query(&sql).await;
                        (sql, result, start.elapsed())
                    },
                    |(sql, result, elapsed)| Message::QueryDone(sql, result, elapsed),
                );
            }
            Message::QueryDone(sql, Ok(result), elapsed) => {
                let truncated = if result.rows.len() >= connector::MAX_ROWS {
                    format!(" (affichage limité à {})", connector::MAX_ROWS)
                } else {
                    String::new()
                };
                self.status = Status::Info(format!(
                    "{} ligne(s) en {} ms{truncated}",
                    result.rows.len(),
                    elapsed.as_millis()
                ));
                self.result = Some(result);
                self.result_sql = Some(sql);
                self.page = 0;
                self.tab = Tab::Results;
            }
            Message::TabSelected(tab) => {
                self.tab = tab;
                if tab == Tab::Diagram && self.diagram.is_none() && !self.diagram_loading {
                    return self.update(Message::LoadDiagram);
                }
                if tab == Tab::Data && self.data_table != self.selected {
                    return self.update(Message::LoadData);
                }
            }
            Message::PageChanged(page) => self.page = page,
            Message::CopyCell(value) => {
                let preview: String = value.chars().take(40).collect();
                self.status = Status::Info(format!("Copié : {preview}"));
                return iced::clipboard::write(value);
            }
            Message::KeyPressed(key, modifiers) => return self.handle_key(key, modifiers),

            Message::TablesLoaded(Err(e))
            | Message::SchemaLoaded(Err(e))
            | Message::Explained(_, Err(e))
            | Message::QueryDone(_, Err(e), _) => self.status = Status::Error(e),
        }
        Task::none()
    }

    /// Raccourcis globaux (hors éditeur, qui a les siens).
    fn handle_key(&mut self, key: Key, modifiers: Modifiers) -> Task<Message> {
        match key.as_ref() {
            Key::Named(Named::Escape) if self.dialog.is_some() => {
                self.update(Message::Dialog(DialogMessage::Cancel))
            }
            Key::Named(Named::Escape) if self.manager.is_some() => self.update(Message::CloseManager),
            Key::Named(Named::Enter) if modifiers.command() => self.update(Message::Run),
            Key::Character("e") if modifiers.command() => self.update(Message::Explain),
            _ => Task::none(),
        }
    }

    fn save_connection(&mut self, connect: bool) -> Task<Message> {
        let Some(m) = &mut self.manager else { return Task::none() };
        let id = m
            .editing
            .unwrap_or_else(|| self.connections.iter().map(|c| c.id).max().unwrap_or(0) + 1);

        let config = match m.form.to_config(id) {
            Ok(config) => config,
            Err(e) => {
                m.feedback = Some(Feedback::Error(e));
                return Task::none();
            }
        };

        let mut result = Ok(());
        if config.engine.is_server() && !m.form.password.is_empty() {
            result = store::set_password(id, &m.form.password);
        }
        match self.connections.iter_mut().find(|c| c.id == id) {
            Some(existing) => *existing = config,
            None => self.connections.push(config),
        }
        let result = result.and(store::save(&self.connections));

        m.editing = Some(id);
        m.form.password.clear();
        m.feedback = Some(match &result {
            Ok(()) => Feedback::Ok("Connexion enregistrée".into()),
            Err(e) => Feedback::Error(e.clone()),
        });

        if connect && result.is_ok() {
            self.manager = None;
            return self.update(Message::Connect(id));
        }
        Task::none()
    }

    // ----------------------------------------------------------------------
    // Vues
    // ----------------------------------------------------------------------

    pub fn view(&self) -> Element<'_, Message> {
        let panes = pane_grid(&self.panes, |_, pane, _| {
            pane_grid::Content::new(match pane {
                Pane::Sidebar => self.sidebar(),
                Pane::Workspace => self.workspace(),
            })
        })
        .on_resize(8, Message::PaneResized)
        .style(theme::pane_grid)
        .spacing(10)
        .height(Fill);

        let base = column![self.top_bar(), panes, self.status_bar()].spacing(10).padding(12);
        let base: Element<'_, Message> = if self.edit_menu && self.session.is_some() {
            stack![base, self.edit_menu()].into()
        } else {
            base.into()
        };

        if let Some(m) = &self.manager {
            return stack![base, manager::view(m, &self.connections)].into();
        }
        let engine = self.active_config().map_or(Engine::Postgres, |c| c.engine);
        match &self.dialog {
            Some(d) => stack![base, editing::view(d, self.dialog_error.as_deref(), engine)].into(),
            None => base.into(),
        }
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let active: Element<'_, Message> = match self.active_config() {
            Some(c) => row![
                text("●").size(10).color(theme::SUCCESS),
                text(&c.name).size(15).color(theme::TEXT),
                text(c.summary()).size(12).font(MONO).color(theme::MUTED),
            ]
            .spacing(10)
            .align_y(iced::Center)
            .into(),
            None => text("Aucune connexion ouverte").size(14).color(theme::MUTED).into(),
        };

        let disconnect = button(text("Déconnecter").size(13))
            .on_press_maybe(self.session.as_ref().map(|_| Message::Disconnect))
            .style(theme::outline(theme::DANGER))
            .padding([5, 12]);

        let edit = button(text(if self.edit_menu { "Édition ▴" } else { "Édition ▾" }).size(13))
            .on_press_maybe(self.session.as_ref().map(|_| Message::ToggleEditMenu))
            .style(theme::outline(theme::PRIMARY))
            .padding([5, 12]);

        row![
            text("Quarry").size(20).color(theme::PRIMARY),
            container(active).width(Fill),
            edit,
            disconnect,
        ]
        .spacing(16)
        .align_y(iced::Center)
        .into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let (label, color) = match &self.status {
            Status::Idle => ("Prêt · ⌘↩ exécuter · ⌘E expliquer".to_string(), theme::MUTED),
            Status::Busy(msg) => (msg.clone(), theme::WARNING),
            Status::Info(msg) => (msg.clone(), theme::SUCCESS),
            Status::Error(msg) => (msg.clone(), theme::DANGER),
        };
        container(text(label).size(12).font(MONO).color(color))
            .style(theme::status_bar)
            .padding([6, 10])
            .width(Fill)
            .into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let mut list = Column::new().spacing(2);

        // Les dossiers d'abord (repliables), puis les connexions sans dossier
        // par emplacement.
        let mut folders: Vec<&str> = self
            .connections
            .iter()
            .map(|c| c.folder.as_str())
            .filter(|f| !f.is_empty())
            .collect();
        folders.sort_unstable();
        folders.dedup();

        for folder in folders {
            let collapsed = self.collapsed.contains(&format!("folder:{folder}"));
            let count = self.connections.iter().filter(|c| c.folder == folder).count();
            list = list.push(
                button(
                    text(format!("{} {folder}  ({count})", if collapsed { "▸" } else { "▾" }))
                        .size(12)
                        .color(theme::TEXT)
                        .wrapping(text::Wrapping::None),
                )
                .on_press(Message::ToggleFolder(folder.to_owned()))
                .style(theme::list_item(false))
                .padding([4, 6])
                .width(Fill)
                .clip(true),
            );
            if collapsed {
                continue;
            }
            for config in self.connections.iter().filter(|c| c.folder == folder) {
                list = list.push(container(self.connection_entry(config)).padding(padding::left(10)));
            }
        }

        for location in Location::ALL {
            let group: Vec<_> = self
                .connections
                .iter()
                .filter(|c| c.folder.is_empty() && c.location() == location)
                .collect();
            if group.is_empty() {
                continue;
            }
            list = list.push(
                container(text(location.label()).size(11).color(theme::MUTED))
                    .padding(padding::top(10).bottom(4)),
            );
            for config in group {
                list = list.push(self.connection_entry(config));
            }
        }

        if self.connections.is_empty() {
            list = list.push(text("Aucune connexion enregistrée.").size(13).color(theme::MUTED));
        }

        let header = row![
            text("CONNEXIONS").size(11).color(theme::MUTED).width(Fill),
            button(text("Gérer").size(12)).on_press(Message::OpenManager).style(button::text),
        ]
        .align_y(iced::Center);

        let filter = text_input("Filtrer les tables…", &self.filter)
            .on_input(Message::FilterChanged)
            .size(13)
            .padding(6);

        container(column![header, filter, scrollable(list).height(Fill)].spacing(6))
            .style(theme::panel)
            .padding(10)
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Une connexion ; si elle est ouverte, ses tables s'affichent dessous,
    /// groupées par schéma.
    fn connection_entry<'a>(&'a self, config: &'a ConnectionConfig) -> Element<'a, Message> {
        let session = self.session.as_ref().filter(|s| s.id == config.id);
        let marker = if session.is_some() {
            text("●").color(theme::SUCCESS)
        } else if self.connecting == Some(config.id) {
            text("●").color(theme::WARNING)
        } else {
            text("○").color(theme::MUTED)
        };

        let folded = self.folded.contains(&config.id);
        let entry = button(
            row![
                text(if session.is_none() { " " } else if folded { "▸" } else { "▾" })
                    .size(12)
                    .color(theme::MUTED),
                marker.size(10),
                column![
                    text(&config.name).size(13).wrapping(text::Wrapping::None),
                    text(format!("{} · {}", config.engine, config.summary()))
                        .size(11)
                        .color(theme::MUTED)
                        .wrapping(text::Wrapping::None),
                ]
                .width(Fill),
            ]
            .spacing(8)
            .align_y(iced::Center),
        )
        .on_press_maybe(session.is_none().then_some(Message::Connect(config.id)))
        .style(theme::list_item(session.is_some()))
        .width(Fill)
        .clip(true);

        let Some(session) = session else { return entry.into() };

        // Connexion ouverte : la flèche devant le nom replie sa liste de tables.
        let entry = entry.on_press(Message::ToggleFold(config.id));
        if folded {
            return entry.into();
        }

        let filter = self.filter.trim().to_lowercase();
        let mut by_schema: BTreeMap<String, Vec<&TableRef>> = BTreeMap::new();
        for table in &session.tables {
            if !filter.is_empty() && !table.to_string().to_lowercase().contains(&filter) {
                continue;
            }
            by_schema.entry(table.schema.clone().unwrap_or_default()).or_default().push(table);
        }

        let mut tree = Column::new().spacing(1);
        for (schema, tables) in by_schema {
            let collapsed = self.collapsed.contains(&schema) && filter.is_empty();
            if !schema.is_empty() {
                let arrow = if collapsed { "▸" } else { "▾" };
                tree = tree.push(
                    button(
                        text(format!("{arrow} {schema}  ({})", tables.len()))
                            .size(12)
                            .color(theme::MUTED)
                            .wrapping(text::Wrapping::None),
                    )
                    .on_press(Message::ToggleSchema(schema.clone()))
                    .style(theme::list_item(false))
                    .padding([3, 6])
                    .width(Fill)
                    .clip(true),
                );
            }
            if collapsed {
                continue;
            }
            for table in tables {
                tree = tree.push(self.table_entry(table, !schema.is_empty()));
            }
        }

        if session.tables.is_empty() {
            tree = tree.push(text("Aucune table").size(12).color(theme::MUTED));
        }

        tree = tree.push(
            button(text("+ Nouvelle table").size(12))
                .on_press_maybe(self.dialog_new_table().map(Message::OpenDialog))
                .style(theme::list_item(false))
                .padding([3, 6])
                .width(Fill),
        );

        column![entry, container(tree).padding(padding::left(12))].spacing(2).into()
    }

    /// Une table : clic = schéma + requête d'aperçu, « + » = insérer le nom
    /// au curseur de l'éditeur.
    fn table_entry<'a>(&'a self, table: &'a TableRef, indent: bool) -> Element<'a, Message> {
        let active = self.selected.as_ref() == Some(table);
        let name = button(
            text(&table.name).size(12).font(MONO).wrapping(text::Wrapping::None),
        )
        .on_press(Message::TableSelected(table.clone()))
        .style(theme::list_item(active))
        .padding([3, 6])
        .width(Fill)
        .clip(true);

        let name = tooltip(name, text(table.to_string()).size(12).font(MONO), tooltip::Position::Right)
            .style(theme::tooltip)
            .padding(6)
            .gap(8);

        let insert = tooltip(
            button(text("+").size(12))
                .on_press(Message::InsertTable(table.clone()))
                .style(theme::list_item(false))
                .padding([3, 7]),
            text("Insérer dans l'éditeur").size(12),
            tooltip::Position::Right,
        )
        .style(theme::tooltip)
        .padding(6)
        .gap(8);

        row![name, insert]
            .spacing(2)
            .align_y(iced::Center)
            .padding(padding::left(if indent { 10.0 } else { 0.0 }))
            .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        let connected = self.session.is_some();

        let editor = text_editor(&self.editor)
            .on_action(Message::Edit)
            .key_binding(editor_key_binding)
            .highlight("sql", highlighter::Theme::Base16Ocean)
            .font(MONO)
            .height(170)
            .padding(10);

        let actions = row![
            button(text("Expliquer").size(13))
                .on_press_maybe(connected.then_some(Message::Explain))
                .style(button::secondary)
                .padding([6, 14]),
            button(text("Exécuter  ⌘↩").size(13))
                .on_press_maybe(connected.then_some(Message::Run))
                .padding([6, 14]),
            text("La sélection seule est exécutée si elle existe.")
                .size(12)
                .color(theme::MUTED),
        ]
        .spacing(10)
        .align_y(iced::Center);

        let tab = |label, tab| {
            button(text(label).size(13))
                .on_press(Message::TabSelected(tab))
                .style(theme::list_item(self.tab == tab))
                .padding([4, 12])
        };
        let tabs = row![
            tab("Résultats", Tab::Results),
            tab("Données", Tab::Data),
            tab("Schéma", Tab::Schema),
            tab("Plan d'exécution", Tab::Plan),
            tab("Diagramme", Tab::Diagram),
        ]
        .spacing(4);

        let content = match self.tab {
            Tab::Results => self.results_view(),
            Tab::Data => self.data_view(),
            Tab::Plan => self.plan_view(),
            Tab::Schema => self.schema_view(),
            Tab::Diagram => self.diagram_view(),
        };

        column![
            editor,
            actions,
            container(column![tabs, content].spacing(8))
                .style(theme::panel)
                .padding(10)
                .width(Fill)
                .height(Fill),
        ]
        .spacing(10)
        .width(Fill)
        .into()
    }

    fn results_view(&self) -> Element<'_, Message> {
        let Some(result) = &self.result else {
            return hint("Écris une requête puis clique sur « Exécuter » (⌘↩).");
        };
        if result.columns.is_empty() {
            return hint("La requête n'a renvoyé aucune ligne.");
        }

        let total = result.rows.len();
        let pages = total.div_ceil(PAGE_SIZE).max(1);
        let page = self.page.min(pages - 1);
        let start = page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(total);

        let rows = result.rows[start..end].iter().map(|r| r.iter().map(|c| c.display()).collect());
        let grid = grid(result.columns.clone(), rows, None, CellAction::Copy);

        let mut header = row![source_line(self.result_sql.as_deref())].spacing(12).align_y(iced::Center);
        if pages > 1 {
            let nav = |label, target: Option<usize>| {
                button(text(label).size(12))
                    .on_press_maybe(target.map(Message::PageChanged))
                    .style(button::secondary)
                    .padding([2, 8])
            };
            header = header.push(container(row![
                nav("‹", page.checked_sub(1)),
                text(format!("lignes {}–{} sur {total}", start + 1, end)).size(12).color(theme::MUTED),
                nav("›", (page + 1 < pages).then_some(page + 1)),
            ]
            .spacing(8)
            .align_y(iced::Center))
            .align_right(Fill));
        }

        column![header, grid].spacing(6).into()
    }

    // ------------------------------------------------------------------
    // Actions d'édition, partagées par le menu « Édition » et les onglets.
    // `None` : l'action ne s'applique pas dans l'état courant.
    // ------------------------------------------------------------------

    fn dialog_new_table(&self) -> Option<Dialog> {
        let session = self.session.as_ref()?;
        let config = self.active_config()?;
        let schema = session.tables.iter().find_map(|t| t.schema.as_deref());
        Some(Dialog::Table(TableForm::new(schema, config.engine)))
    }

    fn dialog_rename_table(&self) -> Option<Dialog> {
        let table = self.selected.clone()?;
        Some(Dialog::Rename(RenameForm { table, column: None, to: String::new() }))
    }

    fn dialog_drop_table(&self) -> Option<Dialog> {
        let (table, c) = (self.selected.as_ref()?, self.connector()?);
        Some(Dialog::Confirm(editing::Pending {
            title: format!("Supprimer la table {table}"),
            sql: c.drop_table_sql(table),
            refresh: Refresh::Structure,
            select: None,
        }))
    }

    fn dialog_add_column(&self) -> Option<Dialog> {
        let table = self.selected.clone()?;
        Some(Dialog::Column(editing::ColumnForm { table, draft: editing::ColumnDraft::new() }))
    }

    fn selected_column(&self) -> Option<String> {
        self.schema_row.and_then(|i| self.schema.get(i)).map(|c| c.name.clone())
    }

    fn dialog_rename_column(&self) -> Option<Dialog> {
        let (table, column) = (self.selected.clone()?, self.selected_column()?);
        Some(Dialog::Rename(RenameForm { table, column: Some(column), to: String::new() }))
    }

    fn dialog_drop_column(&self) -> Option<Dialog> {
        let (table, column, c) = (self.selected.as_ref()?, self.selected_column()?, self.connector()?);
        Some(Dialog::Confirm(editing::Pending {
            title: format!("Supprimer la colonne {column}"),
            sql: c.alter_table_sql(table, &connector::Alter::DropColumn(column)),
            refresh: Refresh::Structure,
            select: Some(table.clone()),
        }))
    }

    fn selected_data_row(&self) -> Option<&Vec<connector::Cell>> {
        let data = self.data.as_ref().filter(|_| self.data_table == self.selected)?;
        data.rows.get(self.data_row?)
    }

    fn dialog_insert_row(&self) -> Option<Dialog> {
        let table = self.selected.clone()?;
        if self.schema.is_empty() {
            return None;
        }
        Some(Dialog::Row(RowForm::insert(table, self.schema.clone())))
    }

    fn dialog_update_row(&self) -> Option<Dialog> {
        let (table, row) = (self.selected.clone()?, self.selected_data_row()?);
        if !self.schema.iter().any(|c| c.primary_key) {
            return None;
        }
        Some(Dialog::Row(RowForm::update(table, self.schema.clone(), row)))
    }

    fn dialog_delete_row(&self) -> Option<Dialog> {
        let (table, row, c) = (self.selected.as_ref()?, self.selected_data_row()?, self.connector()?);
        let key: Vec<_> = self
            .schema
            .iter()
            .enumerate()
            .filter(|(_, col)| col.primary_key)
            .filter_map(|(i, col)| Some((col.name.clone(), row.get(i)?.clone())))
            .collect();
        if key.is_empty() {
            return None;
        }
        Some(Dialog::Confirm(editing::Pending {
            title: format!("Supprimer une ligne de {table}"),
            sql: c.delete_sql(table, &key),
            refresh: Refresh::Data,
            select: Some(table.clone()),
        }))
    }

    /// Menu « Édition » déroulé sous le bouton de la barre du haut.
    fn edit_menu(&self) -> Element<'_, Message> {
        let item = |label: &'static str, dialog: Option<Dialog>, danger: bool| {
            let color = match (dialog.is_some(), danger) {
                (false, _) => Color { a: 0.4, ..theme::MUTED },
                (true, true) => theme::DANGER,
                (true, false) => theme::TEXT,
            };
            button(text(label).size(13).color(color))
                .on_press_maybe(dialog.map(Message::OpenDialog))
                .style(theme::list_item(false))
                .padding([5, 12])
                .width(Fill)
        };
        let section = |label: &'static str| {
            container(text(label).size(11).color(theme::MUTED)).padding(padding::top(8).bottom(2).left(12))
        };
        let table_name = self.selected.as_ref().map_or("aucune table sélectionnée".to_string(), |t| t.to_string());

        let menu = column![
            container(text(table_name).size(12).color(theme::PRIMARY)).padding([6, 12]),
            section("TABLE"),
            item("Nouvelle table…", self.dialog_new_table(), false),
            item("Renommer la table…", self.dialog_rename_table(), false),
            item("Supprimer la table…", self.dialog_drop_table(), true),
            section("COLONNES"),
            item("Ajouter une colonne…", self.dialog_add_column(), false),
            item("Renommer la colonne sélectionnée…", self.dialog_rename_column(), false),
            item("Supprimer la colonne sélectionnée…", self.dialog_drop_column(), true),
            section("LIGNES"),
            item("Ajouter une ligne…", self.dialog_insert_row(), false),
            item("Modifier la ligne sélectionnée…", self.dialog_update_row(), false),
            item("Supprimer la ligne sélectionnée…", self.dialog_delete_row(), true),
        ]
        .width(320);

        let panel = container(menu).style(theme::panel).padding(6);

        // Un clic hors du menu le referme ; dans le menu, il est absorbé.
        iced::widget::mouse_area(
            container(iced::widget::mouse_area(panel).on_press(Message::Noop))
            .align_right(Fill)
            .padding(padding::top(52).right(12))
            .width(Fill)
            .height(Fill),
        )
        .on_press(Message::ToggleEditMenu)
        .into()
    }

    fn schema_view(&self) -> Element<'_, Message> {
        let Some(table) = &self.selected else {
            return hint("Sélectionne une table dans la liste pour voir son schéma.");
        };
        let header = ["Colonne", "Type", "NULL", "Défaut", "Clé"].map(String::from).to_vec();
        let rows = self.schema.iter().map(|c| {
            vec![
                c.name.clone(),
                c.data_type.clone(),
                if c.nullable { "oui" } else { "non" }.into(),
                c.default.clone().unwrap_or_default(),
                if c.primary_key { "PK" } else { "" }.into(),
            ]
        });

        let toolbar = row![
            text(table.to_string()).size(15).color(theme::PRIMARY).width(Fill),
            action_button("+ Colonne", self.dialog_add_column(), false),
            action_button("Renommer la colonne", self.dialog_rename_column(), false),
            action_button("Supprimer la colonne", self.dialog_drop_column(), true),
            action_button("Renommer la table", self.dialog_rename_table(), false),
            action_button("Supprimer la table", self.dialog_drop_table(), true),
        ]
        .spacing(6)
        .align_y(iced::Center);

        column![
            toolbar,
            text("Clique sur une colonne pour la sélectionner.").size(11).color(theme::MUTED),
            grid(header, rows, self.schema_row, CellAction::SelectRow(Message::SchemaRowSelected)),
        ]
        .spacing(6)
        .into()
    }

    fn data_view(&self) -> Element<'_, Message> {
        let Some(table) = &self.selected else {
            return hint("Sélectionne une table dans la liste pour voir et modifier ses lignes.");
        };
        let Some(data) = self.data.as_ref().filter(|_| self.data_table.as_ref() == Some(table)) else {
            return hint("Lecture des lignes…");
        };

        let has_pk = self.schema.iter().any(|c| c.primary_key);

        let toolbar = row![
            text(table.to_string()).size(15).color(theme::PRIMARY).width(Fill),
            action_button("+ Ligne", self.dialog_insert_row(), false),
            action_button("Modifier", self.dialog_update_row(), false),
            action_button("Supprimer", self.dialog_delete_row(), true),
            button(text("Rafraîchir").size(12)).on_press(Message::LoadData).style(button::secondary).padding([3, 10]),
        ]
        .spacing(6)
        .align_y(iced::Center);

        let note = if !has_pk {
            "Cette table n'a pas de clé primaire : on peut insérer, mais pas modifier ni supprimer une ligne sans risque."
        } else if data.rows.len() >= connector::MAX_ROWS {
            "Affichage limité aux premières lignes. Clique sur une ligne pour la sélectionner."
        } else {
            "Clique sur une ligne pour la sélectionner, puis Modifier ou Supprimer."
        };

        let rows = data.rows.iter().map(|r| r.iter().map(|c| c.display()).collect());
        column![
            toolbar,
            text(note).size(11).color(theme::MUTED),
            grid(data.columns.clone(), rows, self.data_row, CellAction::SelectRow(Message::DataRowSelected)),
        ]
        .spacing(6)
        .into()
    }

    fn diagram_view(&self) -> Element<'_, Message> {
        if self.session.is_none() {
            return hint("Ouvre une connexion pour voir le diagramme de ses tables.");
        }
        let Some(d) = &self.diagram else {
            return if self.diagram_loading {
                hint("Lecture des tables et des relations…")
            } else {
                container(
                    button(text("Charger le diagramme").size(13))
                        .on_press(Message::LoadDiagram)
                        .padding([6, 14]),
                )
                .center(Fill)
                .into()
            };
        };

        let mode = |m: diagram::Mode| {
            button(text(m.label()).size(12))
                .on_press(Message::DiagramMode(m))
                .style(theme::list_item(d.mode == m))
                .padding([3, 10])
        };
        let tool = |label, message| {
            button(text(label).size(12))
                .on_press(message)
                .style(button::secondary)
                .padding([3, 10])
        };
        let modes = Row::with_children(diagram::Mode::ALL.map(|m| mode(m).into())).spacing(4);
        let toolbar = row![
            modes,
            text(d.mode.description()).size(12).color(theme::MUTED).width(Fill),
            tool("Réorganiser", Message::DiagramLayout),
            tool("Ajuster", Message::DiagramFit),
            tool("Recharger", Message::LoadDiagram),
        ]
        .spacing(6)
        .align_y(iced::Center);

        let mut info = Vec::new();
        if d.relations.is_empty() {
            info.push("Aucune clé étrangère déclarée : les tables s'affichent sans liens.");
        }
        let hint_text = text(if info.is_empty() {
            "Molette : zoom · glisser le fond : déplacer · glisser une boîte : la ranger"
        } else {
            info[0]
        })
        .size(11)
        .color(theme::MUTED);

        let canvas = canvas(d).width(Fill).height(Fill);
        let canvas = Element::from(canvas).map(Message::DiagramInteraction);

        column![toolbar, hint_text, canvas].spacing(6).into()
    }

    fn plan_view(&self) -> Element<'_, Message> {
        let Some(plan) = &self.plan else {
            return hint("Clique sur « Expliquer » (⌘E) pour voir le plan sans exécuter la requête.");
        };

        let total = match plan.total_cost {
            Some(cost) => format!("Coût total estimé : {cost:.2} {}", plan.cost_unit),
            None => "Ce moteur ne fournit pas d'estimation de coût.".into(),
        };

        let tree = plan.nodes.iter().map(|node| {
            let mut stats = Vec::new();
            if let Some(cost) = node.cost {
                stats.push(format!("coût {cost:.2}"));
            }
            if let Some(rows) = node.rows {
                stats.push(format!("~{rows:.0} lignes"));
            }
            row![
                text(format!("└ {}", node.label)).font(MONO).size(13),
                text(stats.join(" · ")).size(12).color(theme::MUTED),
            ]
            .spacing(14)
            .align_y(iced::Center)
            .padding(padding::left(node.depth as f32 * 20.0))
            .into()
        });

        let order = execution_order(&plan.nodes).into_iter().enumerate().map(|(step, i)| {
            text(format!("{}. {}", step + 1, plan.nodes[i].label)).font(MONO).size(13).into()
        });

        let mut content = column![
            source_line(self.plan_sql.as_deref()),
            text(total).size(15).color(theme::PRIMARY),
        ]
        .spacing(10);
        if let Some(note) = &plan.note {
            content = content.push(text(note).size(12).color(theme::MUTED));
        }
        let content = content
            .push(section("ARBRE DU PLAN"))
            .push(Column::with_children(tree).spacing(4))
            .push(section("ORDRE D'EXÉCUTION"))
            .push(Column::with_children(order).spacing(4));

        scrollable(content).height(Fill).into()
    }
}

/// Raccourcis de l'éditeur : ⌘↩ exécute, ⌘E explique, le reste est standard.
fn editor_key_binding(press: KeyPress) -> Option<Binding<Message>> {
    if press.modifiers.command() {
        match press.key.as_ref() {
            Key::Named(Named::Enter) => return Some(Binding::Custom(Message::Run)),
            Key::Character("e") => return Some(Binding::Custom(Message::Explain)),
            _ => {}
        }
    }
    Binding::from_key_press(press)
}

/// « Requête : SELECT … » au-dessus d'un résultat, pour savoir d'où il vient.
fn source_line(sql: Option<&str>) -> Element<'_, Message> {
    let sql = sql.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
    let preview: String = sql.chars().take(120).collect();
    let suffix = if sql.chars().count() > 120 { "…" } else { "" };
    text(format!("Requête : {preview}{suffix}"))
        .size(12)
        .font(MONO)
        .color(theme::MUTED)
        .wrapping(text::Wrapping::None)
        .into()
}

/// Bouton d'action d'un onglet : grisé quand l'action ne s'applique pas.
fn action_button<'a>(label: &'static str, dialog: Option<Dialog>, danger: bool) -> Element<'a, Message> {
    let b = button(text(label).size(12)).on_press_maybe(dialog.map(Message::OpenDialog)).padding([3, 10]);
    if danger { b.style(theme::outline(theme::DANGER)).into() } else { b.style(button::secondary).into() }
}

fn hint(message: &str) -> Element<'_, Message> {
    container(text(message).color(theme::MUTED)).center(Fill).into()
}

fn section(title: &str) -> Element<'_, Message> {
    text(title).size(11).color(theme::MUTED).into()
}

/// Grille défilable dans les deux sens. Les colonnes prennent la largeur de
/// leur contenu (bornée) et un clic sur une cellule copie sa valeur.
fn grid<'a>(
    columns: Vec<String>,
    rows: impl Iterator<Item = Vec<String>>,
    selected: Option<usize>,
    action: CellAction,
) -> Element<'a, Message> {
    let rows: Vec<Vec<String>> = rows.collect();

    let widths: Vec<f32> = columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let longest = rows
                .iter()
                .filter_map(|r| r.get(i))
                .map(|v| v.chars().count())
                .max()
                .unwrap_or(0)
                .max(name.chars().count());
            (longest as f32 * CHAR_WIDTH + 18.0).clamp(MIN_CELL, MAX_CELL)
        })
        .collect();

    let header = Row::with_children(columns.iter().zip(&widths).map(|(name, &w)| {
        container(text(name.clone()).font(MONO).size(13).wrapping(text::Wrapping::None))
            .width(w)
            .padding([4, 8])
            .clip(true)
            .into()
    }));
    let header = container(header).style(theme::header_cell);

    let lines = rows.into_iter().enumerate().map(|(i, values)| {
        let cells = values.into_iter().zip(&widths).map(|(value, &w)| {
            let label = text(value.clone()).font(MONO).size(13).wrapping(text::Wrapping::None);
            let message = match action {
                CellAction::Copy => Message::CopyCell(value),
                CellAction::SelectRow(f) => f(i),
            };
            button(label)
                .on_press(message)
                .style(theme::cell)
                .width(w)
                .padding([3, 8])
                .clip(true)
                .into()
        });
        let line = container(Row::with_children(cells));
        if selected == Some(i) {
            line.style(theme::grid_row_selected).into()
        } else {
            line.style(theme::grid_row(i % 2 == 1)).into()
        }
    });

    let body = column![header, Column::with_children(lines)].width(Length::Shrink);

    scrollable(body)
        .direction(scrollable::Direction::Both {
            vertical: scrollable::Scrollbar::new(),
            horizontal: scrollable::Scrollbar::new(),
        })
        .width(Fill)
        .height(Fill)
        .into()
}
