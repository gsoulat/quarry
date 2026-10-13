//! État de l'application, messages et vues (architecture Elm d'iced).

use std::collections::{BTreeMap, HashMap, HashSet};
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
use crate::completion::{self, Completion};
use crate::design::{self, Design, DesignMessage};
use crate::diagram::{self, Diagram};
use crate::editing::{self, Dialog, DialogMessage, Refresh, RenameForm, RowForm, TableForm};
use crate::export::{self, Format};
use crate::import::{self, ImportForm, ImportMessage};
use crate::manager::{self, Feedback, Field, Manager};
use crate::store::{self, ConnectionConfig, Engine, HistoryEntry, Location, SavedQuery};
use crate::theme;
use crate::tunnel::Tunnel;

const MONO: Font = Font::MONOSPACE;
/// Largeur approximative d'un caractère en police mono de 13 px.
const CHAR_WIDTH: f32 = 7.8;
const MIN_CELL: f32 = 70.0;
const MAX_CELL: f32 = 420.0;
const PAGE_SIZE: usize = 200;
const SIDEBAR_RATIO: f32 = 0.2;
const EDITOR_RATIO: f32 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Results,
    Data,
    Plan,
    Schema,
    Diagram,
    History,
}

/// Un onglet de requête : son éditeur et ce qu'il a produit.
pub struct QueryTab {
    id: u64,
    name: String,
    editor: text_editor::Content,
    result: Option<QueryResult>,
    /// Requête qui a produit `result`, pour ne pas confondre avec l'éditeur.
    result_sql: Option<String>,
    page: usize,
    plan: Option<QueryPlan>,
    plan_sql: Option<String>,
    /// Tri côté client : (colonne, croissant).
    sort: Option<(usize, bool)>,
    filter: String,
}

impl QueryTab {
    fn new(id: u64, name: String, sql: &str) -> Self {
        Self {
            id,
            name,
            editor: text_editor::Content::with_text(sql),
            result: None,
            result_sql: None,
            page: 0,
            plan: None,
            plan_sql: None,
            sort: None,
            filter: String::new(),
        }
    }

    fn saved(&self) -> SavedQuery {
        SavedQuery { name: self.name.clone(), sql: self.editor.text() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportSource {
    Results,
    Data,
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

/// Panneaux de l'espace de travail : éditeur en haut, résultats en bas.
#[derive(Debug, Clone, Copy)]
enum WorkPane {
    Editor,
    Results,
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
    schemas: Vec<String>,
    /// Tunnel SSH gardé ouvert le temps de la session.
    _tunnel: Option<Arc<Tunnel>>,
}

pub struct App {
    connections: Vec<ConnectionConfig>,
    session: Option<Session>,
    /// Connexion en cours d'ouverture (pour l'indicateur dans la barre latérale).
    connecting: Option<u64>,
    manager: Option<Manager>,
    tabs: Vec<QueryTab>,
    active: usize,
    next_tab_id: u64,
    history: Vec<HistoryEntry>,
    panes: pane_grid::State<Pane>,
    work_panes: pane_grid::State<WorkPane>,
    sidebar_hidden: bool,
    results_hidden: bool,
    /// Filtre de la liste des tables.
    filter: String,
    /// Schémas repliés dans la barre latérale.
    collapsed: HashSet<String>,
    /// Connexions dont la liste de tables est repliée.
    folded: HashSet<u64>,
    diagram: Option<Diagram>,
    diagram_loading: bool,
    design: Design,
    /// Colonnes connues, pour l'autocomplétion (chargées en arrière-plan).
    columns_cache: HashMap<TableRef, Vec<ColumnInfo>>,
    completion: Option<Completion>,
    /// Lignes de la table sélectionnée (onglet Données).
    data: Option<QueryResult>,
    data_table: Option<TableRef>,
    data_row: Option<usize>,
    data_sort: Option<(usize, bool)>,
    data_filter: String,
    /// Cellule en cours d'édition dans l'onglet Données : (ligne, colonne, texte).
    editing_cell: Option<(usize, usize, String)>,
    /// Colonne sélectionnée dans l'onglet Schéma.
    schema_row: Option<usize>,
    dialog: Option<Dialog>,
    dialog_error: Option<String>,
    import: Option<ImportForm>,
    import_error: Option<String>,
    /// Menu « Édition » déroulé.
    edit_menu: bool,
    selected: Option<TableRef>,
    schema: Vec<ColumnInfo>,
    tab: Tab,
    status: Status,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Barre latérale
    Connect(u64),
    Connected(u64, Result<(Arc<dyn Connector>, Option<Arc<Tunnel>>)>),
    Disconnect,
    TablesLoaded(Result<Vec<TableRef>>),
    SchemasLoaded(Result<Vec<String>>),
    ColumnsCached(Vec<(TableRef, Vec<ColumnInfo>)>),
    CompletionMove(i32),
    CompletionAccept,
    CompletionClose,
    TableSelected(TableRef),
    InsertTable(TableRef),
    SchemaLoaded(Result<Vec<ColumnInfo>>),
    FilterChanged(String),
    ToggleSchema(String),
    ToggleFold(u64),
    ToggleFolder(String),
    PaneResized(pane_grid::ResizeEvent),
    WorkPaneResized(pane_grid::ResizeEvent),
    ToggleSidebar,
    ToggleResults,
    /// Relit tables, schémas, table sélectionnée et diagramme.
    Refresh,

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
    Design(DesignMessage),

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
    Explained(u64, String, Result<QueryPlan>),
    Run,
    QueryDone(u64, String, Result<QueryResult>, Duration),
    QueryTabNew,
    QueryTabSelect(u64),
    QueryTabClose(u64),
    QueryTabRename(String),
    /// Ouvre une requête de l'historique dans un nouvel onglet.
    HistoryOpen(String),
    TabSelected(Tab),
    PageChanged(usize),
    CopyCell(String),
    ResultSort(usize),
    ResultFilter(String),
    DataSort(usize),
    DataFilter(String),
    CellEditStart(usize, usize),
    CellEditChanged(String),
    CellEditSubmit,
    CellEditCancel,
    /// Exporte les lignes affichées (filtrées, triées) de la source.
    Export(ExportSource, Format),
    Exported(Result<Option<std::path::PathBuf>>),
    ImportPick,
    ImportRead(Result<Option<(std::path::PathBuf, import::ImportedFile)>>),
    Import(ImportMessage),
    KeyPressed(Key, Modifiers),
}

/// Ouvre une connexion : mot de passe depuis le trousseau, tunnel SSH si
/// demandé, puis connecteur.
async fn open(
    config: ConnectionConfig,
    typed_password: Option<String>,
) -> Result<(Arc<dyn Connector>, Option<Arc<Tunnel>>)> {
    let password = match typed_password {
        Some(password) => Some(password),
        None if config.engine.is_server() => store::password(config.id)?,
        None => None,
    };
    if config.uses_tunnel() {
        let tunnel = Tunnel::open(&config).await?;
        // La base est atteinte par le port local du tunnel.
        let local = ConnectionConfig { host: "127.0.0.1".into(), port: Some(tunnel.local_port), ..config };
        let connector = connector::connect(&local, password).await?;
        return Ok((connector, Some(Arc::new(tunnel))));
    }
    Ok((connector::connect(&config, password).await?, None))
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
        let work_panes = pane_grid::State::with_configuration(pane_grid::Configuration::Split {
            axis: pane_grid::Axis::Horizontal,
            ratio: EDITOR_RATIO,
            a: Box::new(pane_grid::Configuration::Pane(WorkPane::Editor)),
            b: Box::new(pane_grid::Configuration::Pane(WorkPane::Results)),
        });

        Self {
            connections,
            session: None,
            connecting: None,
            manager,
            panes,
            work_panes,
            sidebar_hidden: false,
            results_hidden: false,
            filter: String::new(),
            collapsed: HashSet::new(),
            folded: HashSet::new(),
            diagram: None,
            diagram_loading: false,
            design: Design::default(),
            columns_cache: HashMap::new(),
            completion: None,
            data: None,
            data_table: None,
            data_row: None,
            data_sort: None,
            data_filter: String::new(),
            editing_cell: None,
            schema_row: None,
            dialog: None,
            dialog_error: None,
            import: None,
            import_error: None,
            edit_menu: false,
            selected: None,
            schema: Vec::new(),
            tabs: vec![QueryTab::new(1, "Requête 1".into(), "SELECT 1 AS hello")],
            active: 0,
            next_tab_id: 2,
            history: Vec::new(),
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

    fn tab(&self) -> &QueryTab {
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }

    fn tab_mut(&mut self) -> &mut QueryTab {
        let i = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[i]
    }

    /// SQL à exécuter : la sélection si elle existe, sinon tout l'éditeur.
    fn current_sql(&self) -> String {
        let editor = &self.tab().editor;
        editor.selection().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| editor.text())
    }

    /// Propositions pour le mot sous le curseur de l'onglet actif.
    fn compute_completion(&self) -> Option<Completion> {
        let editor = &self.tab().editor;
        let cursor = editor.cursor();
        let (line, column) = (cursor.position.line, cursor.position.column);
        let line_text = editor.line(line)?.text.to_string();
        let before: String = line_text.chars().take(column).collect();
        let tables: Vec<TableRef> = self.session.as_ref().map(|s| s.tables.clone()).unwrap_or_default();
        completion::suggest(&before, &editor.text(), line, column, &tables, &self.columns_cache)
    }

    /// Recharge la liste des tables et des schémas de la connexion ouverte.
    fn reload_structure(&mut self) -> Task<Message> {
        let Some(c) = self.connector() else { return Task::none() };
        let c2 = c.clone();
        Task::batch([
            Task::perform(async move { c.list_tables().await }, Message::TablesLoaded),
            Task::perform(async move { c2.list_schemas().await }, Message::SchemasLoaded),
        ])
    }

    /// Ouvre un onglet et l'active.
    fn open_tab(&mut self, name: String, sql: &str) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(QueryTab::new(id, name, sql));
        self.active = self.tabs.len() - 1;
    }

    /// Enregistre les onglets de la connexion ouverte.
    fn persist_tabs(&mut self) {
        let Some(session) = &self.session else { return };
        let queries: Vec<SavedQuery> = self.tabs.iter().map(QueryTab::saved).collect();
        if let Err(e) = store::save_queries(session.id, &queries) {
            self.status = Status::Error(e);
        }
    }

    /// Remplace les onglets par ceux enregistrés pour une connexion.
    fn load_tabs(&mut self, connection: u64) {
        let saved = store::load_queries(connection);
        self.tabs.clear();
        if saved.is_empty() {
            self.open_tab("Requête 1".into(), "");
        } else {
            for q in saved {
                self.open_tab(q.name, &q.sql);
            }
        }
        self.active = 0;
        self.history = store::load_history(connection);
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
            Message::Connected(id, Ok((c, tunnel))) => {
                self.connecting = None;
                self.status = Status::Info(format!(
                    "Connecté ({}){}",
                    c.engine(),
                    if tunnel.is_some() { " via tunnel SSH" } else { "" }
                ));
                self.persist_tabs();
                self.session = Some(Session {
                    id,
                    connector: c.clone(),
                    tables: Vec::new(),
                    schemas: Vec::new(),
                    _tunnel: tunnel,
                });
                self.load_tabs(id);
                self.selected = None;
                self.schema.clear();
                self.collapsed.clear();
                self.folded.remove(&id);
                self.diagram = None;
                self.data = None;
                self.data_table = None;
                return self.reload_structure();
            }
            Message::Connected(_, Err(e)) => {
                self.connecting = None;
                self.status = Status::Error(e);
            }
            Message::Disconnect => {
                self.persist_tabs();
                self.session = None;
                self.history.clear();
                self.selected = None;
                self.schema.clear();
                self.diagram = None;
                self.data = None;
                self.data_table = None;
                self.status = Status::Idle;
            }
            Message::SchemasLoaded(Ok(schemas)) => {
                if let Some(session) = &mut self.session {
                    session.schemas = schemas;
                }
            }
            Message::TablesLoaded(Ok(tables)) => {
                let describe_all = self.connector().map(|c| {
                    let tables = tables.clone();
                    Task::perform(
                        async move {
                            let mut out = Vec::new();
                            for t in tables {
                                if let Ok(cols) = c.describe_table(&t).await {
                                    out.push((t, cols));
                                }
                            }
                            out
                        },
                        Message::ColumnsCached,
                    )
                });
                if let Some(session) = &mut self.session {
                    session.tables = tables;
                }
                self.columns_cache.clear();
                let mut tasks = Vec::new();
                if let Some(t) = describe_all {
                    tasks.push(t);
                }
                // Diagramme ouvert : on le recharge avec la nouvelle structure.
                if self.tab == Tab::Diagram && self.diagram.is_none() && !self.diagram_loading {
                    tasks.push(self.update(Message::LoadDiagram));
                }
                // Table sélectionnée encore présente : on relit son schéma
                // (et ses lignes si l'onglet Données est ouvert).
                let still_there = self
                    .selected
                    .as_ref()
                    .is_some_and(|t| self.session.as_ref().is_some_and(|s| s.tables.contains(t)));
                if still_there {
                    tasks.push(self.update(Message::TableSelected(self.selected.clone().unwrap())));
                } else {
                    self.selected = None;
                }
                return Task::batch(tasks);
            }
            Message::ColumnsCached(all) => {
                self.columns_cache = all.into_iter().collect();
            }
            Message::CompletionMove(delta) => {
                if let Some(c) = &mut self.completion {
                    let n = c.items.len() as i32;
                    c.selected = ((c.selected as i32 + delta).rem_euclid(n)) as usize;
                }
            }
            Message::CompletionClose => self.completion = None,
            Message::CompletionAccept => {
                let Some(c) = self.completion.take() else { return Task::none() };
                let Some(item) = c.items.get(c.selected).cloned() else { return Task::none() };
                let editor = &mut self.tab_mut().editor;
                for _ in 0..c.prefix_len {
                    editor.perform(Action::Edit(Edit::Backspace));
                }
                editor.perform(Action::Edit(Edit::Paste(Arc::new(item))));
            }
            Message::TableSelected(table) => {
                let Some(c) = self.connector() else { return Task::none() };
                let same = self.selected.as_ref() == Some(&table);
                if !same {
                    // On ne détruit pas une requête en cours : l'aperçu remplace
                    // l'onglet actif seulement s'il est vide ou déjà un aperçu.
                    let current = self.tab().editor.text();
                    let sql = c.preview_sql(&table);
                    if is_preview_sql(&current) {
                        self.tab_mut().editor = text_editor::Content::with_text(&sql);
                    } else {
                        self.open_tab(table.name.clone(), &sql);
                    }
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
                self.tab_mut().editor.perform(Action::Edit(Edit::Paste(Arc::new(name))));
                return Task::none();
            }
            Message::SchemaLoaded(Ok(columns)) => {
                if let Some(t) = &self.selected {
                    self.columns_cache.insert(t.clone(), columns.clone());
                }
                self.schema = columns;
            }
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
            Message::WorkPaneResized(event) => self.work_panes.resize(event.split, event.ratio),
            Message::ToggleSidebar => self.sidebar_hidden = !self.sidebar_hidden,
            Message::Refresh => {
                if self.session.is_none() {
                    return Task::none();
                }
                self.status = Status::Busy("Actualisation…".into());
                self.diagram = None;
                self.data = None;
                self.data_table = None;
                return self.reload_structure();
            }
            Message::ToggleResults => self.results_hidden = !self.results_hidden,

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
                self.editing_cell = None;
            }
            Message::DataLoaded(_, Err(e)) => self.status = Status::Error(e),
            Message::DataRowSelected(i) => {
                self.editing_cell = None;
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
                let extra = pending.extra.clone();
                self.dialog = None;
                self.status = Status::Busy(if extra.is_empty() {
                    "Exécution…".into()
                } else {
                    format!("Exécution de {} instructions…", extra.len() + 1)
                });
                return Task::perform(
                    async move {
                        let mut total = c.execute(&sql).await?;
                        for statement in extra {
                            total += c.execute(&statement).await?;
                        }
                        Ok(total)
                    },
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
                        let _ = c;
                        self.reload_structure()
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
            Message::DiagramInteraction(diagram::Interaction::Link { from, to }) => {
                self.design.pending_link = Some((from, to));
            }
            Message::DiagramInteraction(interaction) => {
                if let Some(d) = &mut self.diagram {
                    d.apply(interaction);
                }
            }
            Message::Design(message) => return self.update_design(message),

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
                        let (c, tunnel) = open(config, typed).await?;
                        Ok(format!(
                            "Connexion {} réussie{}",
                            c.engine(),
                            if tunnel.is_some() { " (via tunnel SSH)" } else { "" }
                        ))
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

            Message::Edit(action) => {
                let typing = matches!(action, Action::Edit(Edit::Insert(_) | Edit::Backspace));
                self.tab_mut().editor.perform(action);
                self.completion = if typing { self.compute_completion() } else { None };
            }
            Message::Explain => {
                let Some(c) = self.connector() else { return Task::none() };
                self.persist_tabs();
                self.status = Status::Busy("Analyse du plan…".into());
                let sql = self.current_sql();
                let id = self.tab().id;
                return Task::perform(
                    async move {
                        let plan = c.explain(&sql).await;
                        (sql, plan)
                    },
                    move |(sql, plan)| Message::Explained(id, sql, plan),
                );
            }
            Message::Explained(id, sql, Ok(plan)) => {
                self.status = Status::Idle;
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                    tab.plan = Some(plan);
                    tab.plan_sql = Some(sql);
                }
                self.tab = Tab::Plan;
            }
            Message::Run => {
                let Some(c) = self.connector() else { return Task::none() };
                self.persist_tabs();
                self.status = Status::Busy("Exécution…".into());
                let sql = self.current_sql();
                let id = self.tab().id;
                return Task::perform(
                    async move {
                        let start = Instant::now();
                        let result = c.query(&sql).await;
                        (sql, result, start.elapsed())
                    },
                    move |(sql, result, elapsed)| Message::QueryDone(id, sql, result, elapsed),
                );
            }
            Message::QueryDone(id, sql, Ok(result), elapsed) => {
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
                if let Some(conn) = self.session.as_ref().map(|s| s.id) {
                    let entry = HistoryEntry {
                        sql: sql.clone(),
                        at: chrono::Local::now().format("%d/%m %H:%M:%S").to_string(),
                        rows: result.rows.len(),
                        ms: elapsed.as_millis(),
                    };
                    match store::push_history(conn, entry) {
                        Ok(history) => self.history = history,
                        Err(e) => self.status = Status::Error(e),
                    }
                }
                let is_ddl = is_ddl(&sql);
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                    tab.result = Some(result);
                    tab.result_sql = Some(sql);
                    tab.page = 0;
                }
                self.tab = Tab::Results;
                if is_ddl {
                    // Une instruction de structure tapée à la main : la barre
                    // latérale, le schéma et le diagramme doivent suivre.
                    self.diagram = None;
                    return self.reload_structure();
                }
            }
            Message::QueryTabNew => {
                let n = self.next_tab_id;
                self.open_tab(format!("Requête {n}"), "");
                self.persist_tabs();
            }
            Message::QueryTabSelect(id) => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                    self.active = i;
                }
            }
            Message::QueryTabClose(id) => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                    self.tabs.remove(i);
                    if self.tabs.is_empty() {
                        self.open_tab("Requête 1".into(), "");
                    }
                    self.active = self.active.min(self.tabs.len() - 1);
                    self.persist_tabs();
                }
            }
            Message::QueryTabRename(name) => {
                self.tab_mut().name = name;
                self.persist_tabs();
            }
            Message::HistoryOpen(sql) => {
                let n = self.next_tab_id;
                self.open_tab(format!("Requête {n}"), &sql);
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
            Message::PageChanged(page) => self.tab_mut().page = page,
            Message::ResultSort(col) => {
                let tab = self.tab_mut();
                tab.sort = toggle_sort(tab.sort, col);
                tab.page = 0;
            }
            Message::ResultFilter(f) => {
                let tab = self.tab_mut();
                tab.filter = f;
                tab.page = 0;
            }
            Message::DataSort(col) => self.data_sort = toggle_sort(self.data_sort, col),
            Message::DataFilter(f) => self.data_filter = f,
            Message::CellEditStart(row, col) => {
                let text = self
                    .data
                    .as_ref()
                    .and_then(|d| d.rows.get(row))
                    .and_then(|r| r.get(col))
                    .map(|c| match c {
                        connector::Cell::Null => String::new(),
                        other => other.display(),
                    })
                    .unwrap_or_default();
                self.editing_cell = Some((row, col, text));
            }
            Message::CellEditChanged(text) => {
                if let Some(cell) = &mut self.editing_cell {
                    cell.2 = text;
                }
            }
            Message::CellEditCancel => self.editing_cell = None,
            Message::Export(source, format) => {
                let Some(c) = self.connector() else { return Task::none() };
                let (columns, rows, order, table, name) = match source {
                    ExportSource::Results => {
                        let tab = self.tab();
                        let Some(r) = &tab.result else { return Task::none() };
                        let order = order_rows(&r.rows, tab.sort, &tab.filter);
                        (&r.columns, &r.rows, order, None, tab.name.clone())
                    }
                    ExportSource::Data => {
                        let Some(d) = &self.data else { return Task::none() };
                        let order = order_rows(&d.rows, self.data_sort, &self.data_filter);
                        let name = self.selected.as_ref().map_or("export".into(), |t| t.name.clone());
                        (&d.columns, &d.rows, order, self.selected.as_ref(), name)
                    }
                };
                let refs: Vec<&Vec<connector::Cell>> = order.iter().map(|&i| &rows[i]).collect();
                match export::render(format, columns, &refs, table, c.as_ref()) {
                    Ok(bytes) => {
                        let suggested = name.replace(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-', "_");
                        self.status = Status::Busy(format!("Export de {} ligne(s)…", refs.len()));
                        return Task::perform(export::save_with_dialog(suggested, format, bytes), Message::Exported);
                    }
                    Err(e) => self.status = Status::Error(e),
                }
            }
            Message::Exported(Ok(Some(path))) => {
                self.status = Status::Info(format!("Exporté : {}", path.display()));
            }
            Message::Exported(Ok(None)) => self.status = Status::Idle,
            Message::Exported(Err(e)) => self.status = Status::Error(e),
            Message::ImportPick => {
                if self.session.is_none() {
                    return Task::none();
                }
                self.edit_menu = false;
                self.status = Status::Busy("Choix du fichier…".into());
                return Task::perform(import::pick_and_read(), Message::ImportRead);
            }
            Message::ImportRead(Ok(Some((path, file)))) => {
                self.status = Status::Info(format!("{} ligne(s) lue(s)", file.rows.len()));
                let existing = self.selected.clone().filter(|_| !self.schema.is_empty()).map(|t| (t, self.schema.clone()));
                let engine = self.active_config().map_or(Engine::Postgres, |c| c.engine);
                let mut form = ImportForm::new(path, file, existing, engine);
                if form.new_schema.is_none()
                    && let Some(session) = &self.session
                {
                    form.new_schema = session.schemas.iter().find(|s| *s == "public").or(session.schemas.first()).cloned();
                }
                self.import = Some(form);
                self.import_error = None;
            }
            Message::ImportRead(Ok(None)) => self.status = Status::Idle,
            Message::ImportRead(Err(e)) => self.status = Status::Error(e),
            Message::Import(ImportMessage::Cancel) => {
                self.import = None;
                self.import_error = None;
            }
            Message::Import(ImportMessage::Submit) => {
                let (Some(form), Some(c)) = (&self.import, self.connector()) else { return Task::none() };
                match form.to_sql(c.as_ref()) {
                    Ok(pending) => {
                        self.import = None;
                        self.import_error = None;
                        self.dialog = Some(Dialog::Confirm(pending));
                        self.dialog_error = None;
                    }
                    Err(e) => self.import_error = Some(e),
                }
            }
            Message::Import(message) => {
                if let Some(form) = &mut self.import {
                    form.apply(message);
                }
            }
            Message::CellEditSubmit => {
                let Some((row, col, text)) = self.editing_cell.take() else { return Task::none() };
                let (Some(table), Some(data), Some(c)) = (self.selected.clone(), self.data.as_ref(), self.connector()) else {
                    return Task::none();
                };
                let Some(original) = data.rows.get(row) else { return Task::none() };
                // Même chemin que le formulaire : seule la colonne touchée change.
                let mut form = RowForm::update(table, self.schema.clone(), original);
                if let Some(field) = form.fields.get_mut(col) {
                    field.text = text;
                    field.null = false;
                }
                match form.to_sql(c.as_ref()) {
                    Ok(pending) => {
                        self.dialog = Some(Dialog::Confirm(pending));
                        self.dialog_error = None;
                    }
                    Err(e) => self.status = Status::Error(e),
                }
            }
            Message::CopyCell(value) => {
                let preview: String = value.chars().take(40).collect();
                self.status = Status::Info(format!("Copié : {preview}"));
                return iced::clipboard::write(value);
            }
            Message::KeyPressed(key, modifiers) => return self.handle_key(key, modifiers),

            Message::TablesLoaded(Err(e))
            | Message::SchemasLoaded(Err(e))
            | Message::SchemaLoaded(Err(e))
            | Message::Explained(_, _, Err(e))
            | Message::QueryDone(_, _, Err(e), _) => self.status = Status::Error(e),
        }
        Task::none()
    }

    /// Raccourcis globaux (hors éditeur, qui a les siens).
    fn handle_key(&mut self, key: Key, modifiers: Modifiers) -> Task<Message> {
        match key.as_ref() {
            Key::Named(Named::Escape) if self.import.is_some() => self.update(Message::Import(ImportMessage::Cancel)),
            Key::Named(Named::Escape) if self.editing_cell.is_some() => self.update(Message::CellEditCancel),
            Key::Named(Named::Escape) if self.design.pending_link.is_some() => {
                self.update(Message::Design(DesignMessage::LinkCancel))
            }
            Key::Named(Named::Escape) if self.dialog.is_some() => {
                self.update(Message::Dialog(DialogMessage::Cancel))
            }
            Key::Named(Named::Escape) if self.manager.is_some() => self.update(Message::CloseManager),
            Key::Named(Named::Enter) if modifiers.command() => self.update(Message::Run),
            Key::Character("e") if modifiers.command() => self.update(Message::Explain),
            Key::Character("r") if modifiers.command() => self.update(Message::Refresh),
            _ => Task::none(),
        }
    }

    fn update_design(&mut self, message: DesignMessage) -> Task<Message> {
        use DesignMessage as M;

        let engine = self.active_config().map_or(Engine::Postgres, |c| c.engine);
        let id_type = match engine {
            Engine::Postgres => "SERIAL",
            Engine::MySql => "INT",
            Engine::Sqlite => "INTEGER",
            Engine::BigQuery => "INT64",
        };

        // Le connecteur est pris avant l'emprunt mutable du diagramme.
        let connector = self.connector();

        // Entrée en conception sans diagramme chargé : on part de vide.
        if matches!(message, M::Toggle | M::Clear) && self.diagram.is_none() {
            let mut d = Diagram::empty();
            d.editable = true;
            self.diagram = Some(d);
            if matches!(message, M::Toggle) {
                return Task::none();
            }
        }
        let Some(d) = &mut self.diagram else { return Task::none() };
        let selected = d.selected;

        match message {
            M::Toggle => {
                d.editable = !d.editable;
                self.design.custom.clear();
            }
            M::Clear => {
                let mut fresh = Diagram::empty();
                fresh.editable = true;
                *d = fresh;
                self.design.custom.clear();
            }
            M::AddTable => {
                let schema = d.nodes.iter().find_map(|n| n.table.schema.clone());
                d.add_table(schema, id_type);
            }
            M::RemoveTable => {
                if let Some(i) = selected {
                    d.remove_table(i);
                    self.design.custom.retain(|(t, _)| *t != i);
                }
            }
            M::TableName(name) => {
                if let Some(i) = selected {
                    d.rename_table(i, &name);
                }
            }
            M::ColumnName(c, v) => {
                if let Some(i) = selected {
                    d.edit_column(i, c, |col| col.name = v);
                }
            }
            M::ColumnType(c, v) => {
                if let Some(i) = selected {
                    d.edit_column(i, c, |col| col.data_type = v);
                }
            }
            M::ColumnCustom(c) => {
                if let Some(i) = selected
                    && !self.design.custom.remove(&(i, c))
                {
                    self.design.custom.insert((i, c));
                }
            }
            M::ColumnNullable(c, v) => {
                if let Some(i) = selected {
                    d.edit_column(i, c, |col| col.nullable = v);
                }
            }
            M::ColumnPrimary(c, v) => {
                if let Some(i) = selected {
                    d.edit_column(i, c, |col| {
                        col.primary_key = v;
                        if v {
                            col.nullable = false;
                        }
                    });
                }
            }
            M::AddColumn => {
                if let Some(i) = selected {
                    d.add_column(i);
                }
            }
            M::RemoveColumn(c) => {
                if let Some(i) = selected {
                    d.remove_column(i, c);
                    self.design.custom.remove(&(i, c));
                }
            }
            M::RemoveRelation(r) => d.remove_relation(r),
            M::LinkChosen(kind) => {
                if let Some((from, to)) = self.design.pending_link.take() {
                    match d.link(from, to, kind) {
                        Ok(()) => self.status = Status::Info("Relation ajoutée".into()),
                        Err(e) => self.status = Status::Error(e),
                    }
                }
            }
            M::LinkCancel => self.design.pending_link = None,
            M::Script => {
                let Some(c) = connector else {
                    self.status = Status::Error("Ouvre une connexion pour générer le script".into());
                    return Task::none();
                };
                match d.to_ddl(c.as_ref()) {
                    Ok(sql) => {
                        let count = d.nodes.iter().filter(|n| !n.existing).count();
                        self.dialog = Some(Dialog::Confirm(editing::Pending {
                            title: format!("Créer {count} table(s) d'après le diagramme"),
                            sql,
                            refresh: Refresh::Structure,
                            select: None,
                            extra: Vec::new(),
                        }));
                        self.dialog_error = None;
                    }
                    Err(e) => self.status = Status::Error(e),
                }
            }
        }
        Task::none()
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
        let body: Element<'_, Message> = if self.sidebar_hidden {
            // Barre repliée : une languette verticale pour la rouvrir.
            let strip = button(text("▶\nC\nO\nN\nN\nE\nX\nI\nO\nN\nS").size(11).color(theme::MUTED))
                .on_press(Message::ToggleSidebar)
                .style(theme::list_item(false))
                .padding([8, 4])
                .height(Fill);
            row![container(strip).style(theme::panel), self.workspace()].spacing(10).height(Fill).into()
        } else {
            pane_grid(&self.panes, |_, pane, _| {
                pane_grid::Content::new(match pane {
                    Pane::Sidebar => self.sidebar(),
                    Pane::Workspace => self.workspace(),
                })
            })
            .on_resize(8, Message::PaneResized)
            .style(theme::pane_grid)
            .spacing(10)
            .height(Fill)
            .into()
        };

        let base = column![self.top_bar(), body, self.status_bar()].spacing(10).padding(12);
        let base: Element<'_, Message> = if self.edit_menu && self.session.is_some() {
            stack![base, self.edit_menu()].into()
        } else {
            base.into()
        };

        if let Some(m) = &self.manager {
            return stack![base, manager::view(m, &self.connections)].into();
        }
        let engine = self.active_config().map_or(Engine::Postgres, |c| c.engine);
        if let Some(form) = &self.import {
            return stack![base, import::view(form, self.import_error.as_deref())].into();
        }
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
            Status::Idle => ("Prêt · ⌘↩ exécuter · ⌘E expliquer · ⌘R actualiser".to_string(), theme::MUTED),
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
            tooltip(
                button(text("↻").size(13))
                    .on_press_maybe(self.session.as_ref().map(|_| Message::Refresh))
                    .style(button::text)
                    .padding([2, 4]),
                text("Actualiser (⌘R)").size(12),
                tooltip::Position::Bottom,
            )
            .style(theme::tooltip)
            .padding(6),
            button(text("Gérer").size(12)).on_press(Message::OpenManager).style(button::text),
            button(text("◀").size(11).color(theme::MUTED))
                .on_press(Message::ToggleSidebar)
                .style(button::text)
                .padding([2, 4]),
        ]
        .spacing(4)
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

    fn query_tab_bar(&self) -> Element<'_, Message> {
        let mut bar = Row::new().spacing(2).align_y(iced::Center);
        for (i, tab) in self.tabs.iter().enumerate() {
            let active = i == self.active;
            let label: Element<'_, Message> = if active {
                text_input("nom", &tab.name)
                    .on_input(Message::QueryTabRename)
                    .size(12)
                    .padding([2, 6])
                    .width(150)
                    .into()
            } else {
                button(text(&tab.name).size(12).wrapping(text::Wrapping::None))
                    .on_press(Message::QueryTabSelect(tab.id))
                    .style(theme::list_item(false))
                    .padding([4, 8])
                    .into()
            };
            let close = button(text("×").size(12))
                .on_press(Message::QueryTabClose(tab.id))
                .style(button::text)
                .padding([2, 4]);
            bar = bar.push(
                container(row![label, close].spacing(2).align_y(iced::Center))
                    .style(if active { theme::header_cell } else { |_: &Theme| container::Style::default() })
                    .padding(2),
            );
        }
        bar = bar.push(
            button(text("+").size(13))
                .on_press(Message::QueryTabNew)
                .style(theme::list_item(false))
                .padding([3, 8]),
        );
        scrollable(bar)
            .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(4).scroller_width(4)))
            .into()
    }

    /// Haut de l'espace de travail : onglets de requêtes, éditeur, actions.
    fn editor_pane(&self) -> Element<'_, Message> {
        let connected = self.session.is_some();

        let completing = self.completion.is_some();
        let editor = text_editor(&self.tab().editor)
            .on_action(Message::Edit)
            .key_binding(move |press| editor_key_binding(press, completing))
            .highlight("sql", highlighter::Theme::Base16Ocean)
            .font(MONO)
            .height(Fill)
            .padding(10);

        // Liste de complétion posée sous le curseur (position approchée :
        // police mono de 13 px, interligne 1,3).
        let editor: Element<'_, Message> = match &self.completion {
            Some(c) => {
                let items = c.items.iter().enumerate().map(|(i, item)| {
                    button(text(item).size(12).font(MONO))
                        .on_press(Message::CompletionMove(i as i32 - c.selected as i32))
                        .style(theme::list_item(i == c.selected))
                        .padding([2, 8])
                        .width(Fill)
                        .into()
                });
                let list = container(Column::with_children(items).spacing(1))
                    .style(theme::tooltip)
                    .padding(4)
                    .width(240);
                let x = 10.0 + (c.column.saturating_sub(c.prefix_len)) as f32 * CHAR_WIDTH;
                let y = 10.0 + (c.line as f32 + 1.0) * 13.0 * 1.3 + 2.0;
                stack![
                    editor,
                    container(list).padding(padding::top(y).left(x)),
                ]
                .into()
            }
            None => editor.into(),
        };

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
                .color(theme::MUTED)
                .width(Fill),
            button(text(if self.results_hidden { "▲ Résultats" } else { "▼ Replier les résultats" }).size(11))
                .on_press(Message::ToggleResults)
                .style(button::text)
                .padding([2, 6]),
        ]
        .spacing(10)
        .align_y(iced::Center);

        column![self.query_tab_bar(), editor, actions].spacing(10).width(Fill).height(Fill).into()
    }

    /// Bas de l'espace de travail : les onglets Résultats, Données, Schéma…
    fn results_pane(&self) -> Element<'_, Message> {
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
            tab("Historique", Tab::History),
        ]
        .spacing(4);

        let content = match self.tab {
            Tab::Results => self.results_view(),
            Tab::Data => self.data_view(),
            Tab::Plan => self.plan_view(),
            Tab::Schema => self.schema_view(),
            Tab::Diagram => self.diagram_view(),
            Tab::History => self.history_view(),
        };

        container(column![tabs, content].spacing(8))
            .style(theme::panel)
            .padding(10)
            .width(Fill)
            .height(Fill)
            .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        if self.results_hidden {
            // Partie basse repliée : l'éditeur prend toute la hauteur.
            return self.editor_pane();
        }
        pane_grid(&self.work_panes, |_, pane, _| {
            pane_grid::Content::new(match pane {
                WorkPane::Editor => self.editor_pane(),
                WorkPane::Results => self.results_pane(),
            })
        })
        .on_resize(8, Message::WorkPaneResized)
        .style(theme::pane_grid)
        .spacing(10)
        .width(Fill)
        .height(Fill)
        .into()
    }

    fn history_view(&self) -> Element<'_, Message> {
        if self.session.is_none() {
            return hint("L'historique est propre à chaque connexion.");
        }
        if self.history.is_empty() {
            return hint("Aucune requête exécutée sur cette connexion pour l'instant.");
        }
        let entries = self.history.iter().map(|h| {
            let preview: String = h.sql.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(140).collect();
            button(
                row![
                    text(&h.at).size(11).color(theme::MUTED).width(90),
                    text(preview).size(12).font(MONO).wrapping(text::Wrapping::None).width(Fill),
                    text(format!("{} l. · {} ms", h.rows, h.ms)).size(11).color(theme::MUTED),
                ]
                .spacing(10)
                .align_y(iced::Center),
            )
            .on_press(Message::HistoryOpen(h.sql.clone()))
            .style(theme::list_item(false))
            .width(Fill)
            .clip(true)
            .into()
        });
        column![
            text("Clique sur une requête pour l'ouvrir dans un nouvel onglet.").size(11).color(theme::MUTED),
            scrollable(Column::with_children(entries).spacing(2)).height(Fill),
        ]
        .spacing(6)
        .into()
    }

    fn results_view(&self) -> Element<'_, Message> {
        let tab = self.tab();
        let Some(result) = &tab.result else {
            return hint("Écris une requête puis clique sur « Exécuter » (⌘↩).");
        };
        if result.columns.is_empty() {
            return hint("La requête n'a renvoyé aucune ligne.");
        }

        let order = order_rows(&result.rows, tab.sort, &tab.filter);
        let total = order.len();
        let pages = total.div_ceil(PAGE_SIZE).max(1);
        let page = tab.page.min(pages - 1);
        let start = page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(total);

        let rows = order[start..end].iter().map(|&i| (i, result.rows[i].iter().map(|c| c.display()).collect()));
        let grid = Grid::new(result.columns.clone(), rows)
            .action(CellAction::Copy)
            .sort(tab.sort, Message::ResultSort)
            .build();

        let filter = text_input("Filtrer…", &tab.filter)
            .on_input(Message::ResultFilter)
            .size(12)
            .padding([3, 8])
            .width(180);
        let mut header = row![source_line(tab.result_sql.as_deref()), filter, export_buttons(ExportSource::Results)]
            .spacing(12)
            .align_y(iced::Center);
        if pages > 1 {
            let nav = |label, target: Option<usize>| {
                button(text(label).size(12))
                    .on_press_maybe(target.map(Message::PageChanged))
                    .style(button::secondary)
                    .padding([2, 8])
            };
            header = header.push(container(row![
                nav("‹", page.checked_sub(1)),
                text(format!("lignes {}–{} sur {total}{}", start + 1, end, if total < result.rows.len() { " (filtrées)" } else { "" })).size(12).color(theme::MUTED),
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
        let mut schemas = session.schemas.clone();
        if schemas.is_empty() {
            schemas = session.tables.iter().filter_map(|t| t.schema.clone()).collect();
            schemas.sort_unstable();
            schemas.dedup();
        }
        Some(Dialog::Table(TableForm::new(schemas, config.engine)))
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
            extra: Vec::new(),
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
            extra: Vec::new(),
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
            extra: Vec::new(),
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
            section("FICHIERS"),
            button(text("Importer un fichier (CSV, Excel, Parquet)…").size(13).color(theme::TEXT))
                .on_press(Message::ImportPick)
                .style(theme::list_item(false))
                .padding([5, 12])
                .width(Fill),
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
            Grid::new(header, rows.enumerate())
                .selected(self.schema_row)
                .action(CellAction::SelectRow(Message::SchemaRowSelected))
                .build(),
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

        let mut toolbar = row![text(table.to_string()).size(15).color(theme::PRIMARY).width(Fill)];
        if self.editing_cell.is_some() {
            toolbar = toolbar
                .push(text("Cellule en cours de modification").size(12).color(theme::WARNING))
                .push(button(text("Enregistrer").size(12)).on_press(Message::CellEditSubmit).padding([3, 10]))
                .push(
                    button(text("Annuler").size(12))
                        .on_press(Message::CellEditCancel)
                        .style(button::secondary)
                        .padding([3, 10]),
                );
        }
        let toolbar = toolbar.push(action_button("+ Ligne", self.dialog_insert_row(), false))
            .push(action_button("Modifier", self.dialog_update_row(), false))
            .push(action_button("Supprimer", self.dialog_delete_row(), true));
        let toolbar = row![
            toolbar,
            button(text("Rafraîchir").size(12)).on_press(Message::LoadData).style(button::secondary).padding([3, 10]),
        ]
        .spacing(6)
        .align_y(iced::Center);

        let note = if !has_pk {
            "Cette table n'a pas de clé primaire : on peut insérer, mais pas modifier ni supprimer une ligne sans risque."
        } else if data.rows.len() >= connector::MAX_ROWS {
            "Affichage limité aux premières lignes. Clic : sélectionner · double-clic sur une cellule : la modifier · en-tête : trier."
        } else {
            "Clic : sélectionner · double-clic sur une cellule : la modifier, puis Enregistrer · en-tête : trier."
        };

        let order = order_rows(&data.rows, self.data_sort, &self.data_filter);
        let rows = order.iter().map(|&i| (i, data.rows[i].iter().map(|c| c.display()).collect()));
        let mut grid = Grid::new(data.columns.clone(), rows)
            .selected(self.data_row)
            .action(CellAction::SelectRow(Message::DataRowSelected))
            .sort(self.data_sort, Message::DataSort);
        if has_pk {
            grid = grid.editable(self.editing_cell.as_ref().map(|(r, c, t)| (*r, *c, t.as_str())));
        }

        let filter = text_input("Filtrer…", &self.data_filter)
            .on_input(Message::DataFilter)
            .size(12)
            .padding([3, 8])
            .width(180);

        column![
            toolbar,
            row![text(note).size(11).color(theme::MUTED).width(Fill), filter, export_buttons(ExportSource::Data)]
                .spacing(8)
                .align_y(iced::Center),
            grid.build(),
        ]
        .spacing(6)
        .into()
    }

    fn diagram_view(&self) -> Element<'_, Message> {
        let Some(d) = &self.diagram else {
            return if self.diagram_loading {
                hint("Lecture des tables et des relations…")
            } else {
                let mut actions = row![].spacing(10);
                if self.session.is_some() {
                    actions = actions.push(
                        button(text("Charger le diagramme de la base").size(13))
                            .on_press(Message::LoadDiagram)
                            .padding([6, 14]),
                    );
                }
                actions = actions.push(
                    button(text("Concevoir un nouveau schéma").size(13))
                        .on_press(Message::Design(DesignMessage::Toggle))
                        .style(button::secondary)
                        .padding([6, 14]),
                );
                container(
                    column![
                        actions,
                        text("La conception dessine des tables virtuelles ; le script généré les crée dans la base connectée.")
                            .size(12)
                            .color(theme::MUTED),
                    ]
                    .spacing(10)
                    .align_x(iced::Center),
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
        let design_toggle = button(text(if d.editable { "Conception ✓" } else { "Conception" }).size(12))
            .on_press(Message::Design(DesignMessage::Toggle))
            .style(theme::outline(if d.editable { theme::SUCCESS } else { theme::PRIMARY }))
            .padding([3, 10]);

        let mut toolbar = row![modes, design_toggle].spacing(6).align_y(iced::Center);
        if d.editable {
            toolbar = toolbar
                .push(tool("+ Table", Message::Design(DesignMessage::AddTable)))
                .push(tool("Générer le script", Message::Design(DesignMessage::Script)))
                .push(tool("Vider", Message::Design(DesignMessage::Clear)));
        }
        toolbar = toolbar
            .push(text(d.mode.description()).size(12).color(theme::MUTED).width(Fill))
            .push(tool("Réorganiser", Message::DiagramLayout))
            .push(tool("Ajuster", Message::DiagramFit))
            .push(tool("Recharger", Message::LoadDiagram));

        let hint_text = text(if d.editable {
            "Tire la poignée ● d'une table jusqu'à une autre pour les lier · clic sur une table : la modifier à droite"
        } else if d.relations.is_empty() && !d.nodes.is_empty() {
            "Aucune clé étrangère déclarée : les tables s'affichent sans liens."
        } else {
            "Molette : zoom · glisser le fond : déplacer · glisser une boîte : la ranger"
        })
        .size(11)
        .color(theme::MUTED);

        let canvas = canvas(d).width(Fill).height(Fill);
        let canvas = Element::from(canvas).map(Message::DiagramInteraction);

        let body: Element<'_, Message> = if d.editable {
            let engine = self.active_config().map_or(Engine::Postgres, |c| c.engine);
            row![canvas, design::panel(&self.design, d, engine)].spacing(8).into()
        } else {
            canvas
        };

        let view = column![toolbar, hint_text, body].spacing(6);
        match self.design.pending_link {
            Some((from, to)) if from < d.nodes.len() && to < d.nodes.len() => {
                stack![view, design::link_prompt(d, from, to)].into()
            }
            _ => view.into(),
        }
    }

    fn plan_view(&self) -> Element<'_, Message> {
        let tab = self.tab();
        let Some(plan) = &tab.plan else {
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
            source_line(tab.plan_sql.as_deref()),
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
fn editor_key_binding(press: KeyPress, completing: bool) -> Option<Binding<Message>> {
    if completing {
        match press.key.as_ref() {
            Key::Named(Named::Tab) => return Some(Binding::Custom(Message::CompletionAccept)),
            Key::Named(Named::Enter) if !press.modifiers.command() => {
                return Some(Binding::Custom(Message::CompletionAccept));
            }
            Key::Named(Named::ArrowDown) => return Some(Binding::Custom(Message::CompletionMove(1))),
            Key::Named(Named::ArrowUp) => return Some(Binding::Custom(Message::CompletionMove(-1))),
            Key::Named(Named::Escape) => return Some(Binding::Custom(Message::CompletionClose)),
            _ => {}
        }
    }
    if press.modifiers.command() {
        match press.key.as_ref() {
            Key::Named(Named::Enter) => return Some(Binding::Custom(Message::Run)),
            Key::Character("e") => return Some(Binding::Custom(Message::Explain)),
            Key::Character("r") => return Some(Binding::Custom(Message::Refresh)),
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

/// Instruction qui change la structure de la base : après son exécution,
/// la liste des tables, des schémas et le diagramme doivent être relus.
fn is_ddl(sql: &str) -> bool {
    let head = sql.trim_start().to_uppercase();
    ["CREATE", "DROP", "ALTER", "RENAME", "TRUNCATE"].iter().any(|k| head.starts_with(k))
}

/// Contenu d'onglet que l'on peut remplacer sans perte par l'aperçu d'une
/// table : vide, ou un aperçu généré (`SELECT * FROM … LIMIT 100` sur une
/// ligne). Tout le reste est une requête de l'utilisateur.
fn is_preview_sql(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || (trimmed.starts_with("SELECT * FROM") && trimmed.lines().count() == 1)
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

/// « Exporter : CSV · JSON · SQL · Excel ».
fn export_buttons<'a>(source: ExportSource) -> Element<'a, Message> {
    let mut r = row![text("Exporter :").size(11).color(theme::MUTED)].spacing(4).align_y(iced::Center);
    for format in Format::ALL {
        r = r.push(
            button(text(format.label()).size(11))
                .on_press(Message::Export(source, format))
                .style(theme::list_item(false))
                .padding([2, 6]),
        );
    }
    r.into()
}

/// Bascule le tri sur une colonne : croissant, puis décroissant, puis aucun.
fn toggle_sort(current: Option<(usize, bool)>, col: usize) -> Option<(usize, bool)> {
    match current {
        Some((c, true)) if c == col => Some((col, false)),
        Some((c, false)) if c == col => None,
        _ => Some((col, true)),
    }
}

/// Indices des lignes à afficher : filtrées (sous-chaîne, sans casse, sur
/// toutes les colonnes) puis triées. Les nombres se comparent comme des
/// nombres, NULL en dernier.
fn order_rows(rows: &[Vec<connector::Cell>], sort: Option<(usize, bool)>, filter: &str) -> Vec<usize> {
    use connector::Cell;
    let needle = filter.trim().to_lowercase();
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| needle.is_empty() || rows[i].iter().any(|c| c.display().to_lowercase().contains(&needle)))
        .collect();
    let Some((col, ascending)) = sort else { return order };

    let key = |i: usize| rows[i].get(col);
    order.sort_by(|&a, &b| {
        let ordering = match (key(a), key(b)) {
            (Some(Cell::Null) | None, Some(Cell::Null) | None) => std::cmp::Ordering::Equal,
            (Some(Cell::Null) | None, _) => std::cmp::Ordering::Greater,
            (_, Some(Cell::Null) | None) => std::cmp::Ordering::Less,
            (Some(x), Some(y)) => match (number(x), number(y)) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                _ => x.display().to_lowercase().cmp(&y.display().to_lowercase()),
            },
        };
        // NULL reste en dernier quel que soit le sens.
        let is_null = |i| matches!(key(i), Some(Cell::Null) | None);
        if is_null(a) || is_null(b) || ascending { ordering } else { ordering.reverse() }
    });
    order
}

fn number(cell: &connector::Cell) -> Option<f64> {
    match cell {
        connector::Cell::Int(i) => Some(*i as f64),
        connector::Cell::Float(f) => Some(*f),
        connector::Cell::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Grille défilable dans les deux sens. Les colonnes prennent la largeur de
/// leur contenu (bornée). Chaque ligne porte son indice d'origine, pour que
/// la sélection survive au tri et au filtre.
struct Grid<'a> {
    columns: Vec<String>,
    rows: Vec<(usize, Vec<String>)>,
    selected: Option<usize>,
    action: CellAction,
    sort: Option<(usize, bool)>,
    on_sort: Option<fn(usize) -> Message>,
    /// (ligne, colonne, texte) : cette cellule est un champ de saisie.
    editing: Option<(usize, usize, &'a str)>,
    /// Second clic sur une cellule de la ligne sélectionnée : édition en place.
    editable: bool,
}

impl<'a> Grid<'a> {
    fn new(columns: Vec<String>, rows: impl Iterator<Item = (usize, Vec<String>)>) -> Self {
        Self {
            columns,
            rows: rows.collect(),
            selected: None,
            action: CellAction::Copy,
            sort: None,
            on_sort: None,
            editing: None,
            editable: false,
        }
    }

    fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    fn action(mut self, action: CellAction) -> Self {
        self.action = action;
        self
    }

    fn sort(mut self, sort: Option<(usize, bool)>, on_sort: fn(usize) -> Message) -> Self {
        self.sort = sort;
        self.on_sort = Some(on_sort);
        self
    }

    fn editable(mut self, editing: Option<(usize, usize, &'a str)>) -> Self {
        self.editable = true;
        self.editing = editing;
        self
    }

    fn build(self) -> Element<'a, Message> {
        let widths: Vec<f32> = self
            .columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let longest = self
                    .rows
                    .iter()
                    .filter_map(|(_, r)| r.get(i))
                    .map(|v| v.chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(name.chars().count() + 2);
                (longest as f32 * CHAR_WIDTH + 18.0).clamp(MIN_CELL, MAX_CELL)
            })
            .collect();

        let header = Row::with_children(self.columns.iter().enumerate().zip(&widths).map(|((i, name), &w)| {
            let arrow = match self.sort {
                Some((c, true)) if c == i => " ▲",
                Some((c, false)) if c == i => " ▼",
                _ => "",
            };
            let label = text(format!("{name}{arrow}")).font(MONO).size(13).wrapping(text::Wrapping::None);
            match self.on_sort {
                Some(on_sort) => button(label)
                    .on_press(on_sort(i))
                    .style(theme::cell)
                    .width(w)
                    .padding([4, 8])
                    .clip(true)
                    .into(),
                None => container(label).width(w).padding([4, 8]).clip(true).into(),
            }
        }));
        let header = container(header).style(theme::header_cell);

        let lines = self.rows.into_iter().enumerate().map(|(n, (i, values))| {
            let is_selected = self.selected == Some(i);
            let cells = values.into_iter().enumerate().zip(&widths).map(|((col, value), &w)| {
                if let Some((er, ec, text_value)) = self.editing
                    && er == i
                    && ec == col
                {
                    return text_input("", text_value)
                        .on_input(Message::CellEditChanged)
                        .on_submit(Message::CellEditSubmit)
                        .font(MONO)
                        .size(13)
                        .padding([1, 6])
                        .width(w)
                        .into();
                }
                let label = text(value.clone()).font(MONO).size(13).wrapping(text::Wrapping::None);
                let message = match self.action {
                    CellAction::Copy => Message::CopyCell(value),
                    CellAction::SelectRow(_) if self.editable && is_selected => Message::CellEditStart(i, col),
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
            if is_selected {
                line.style(theme::grid_row_selected).into()
            } else {
                line.style(theme::grid_row(n % 2 == 1)).into()
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_detected_regardless_of_case_and_spacing() {
        assert!(is_ddl("CREATE SCHEMA test"));
        assert!(is_ddl("  create table t (id int)"));
        assert!(is_ddl("ALTER TABLE t ADD COLUMN c TEXT"));
        assert!(is_ddl("drop table t"));
        assert!(!is_ddl("SELECT * FROM t"));
        assert!(!is_ddl("INSERT INTO t VALUES (1)"));
        assert!(!is_ddl("-- CREATE dans un commentaire\nSELECT 1"));
    }

    #[test]
    fn sort_toggles_asc_desc_none() {
        assert_eq!(toggle_sort(None, 2), Some((2, true)));
        assert_eq!(toggle_sort(Some((2, true)), 2), Some((2, false)));
        assert_eq!(toggle_sort(Some((2, false)), 2), None);
        assert_eq!(toggle_sort(Some((2, false)), 0), Some((0, true)));
    }

    #[test]
    fn rows_are_filtered_then_sorted_numerically_with_nulls_last() {
        use connector::Cell;
        let rows = vec![
            vec![Cell::Text("b".into()), Cell::Int(10)],
            vec![Cell::Text("a".into()), Cell::Null],
            vec![Cell::Text("c".into()), Cell::Int(9)],
            vec![Cell::Text("ab".into()), Cell::Float(9.5)],
        ];
        assert_eq!(order_rows(&rows, None, ""), vec![0, 1, 2, 3]);
        assert_eq!(order_rows(&rows, Some((1, true)), ""), vec![2, 3, 0, 1]);
        assert_eq!(order_rows(&rows, Some((1, false)), ""), vec![0, 3, 2, 1]);
        assert_eq!(order_rows(&rows, Some((0, true)), "A"), vec![1, 3]);
    }

    #[test]
    fn preview_replaces_only_empty_or_generated_tabs() {
        assert!(is_preview_sql(""));
        assert!(is_preview_sql("   \n"));
        assert!(is_preview_sql("SELECT * FROM \"public\".\"users\" LIMIT 100"));
        assert!(!is_preview_sql("SELECT * FROM users\nWHERE id = 1"));
        assert!(!is_preview_sql("SELECT name FROM users"));
        assert!(!is_preview_sql("CREATE SCHEMA test"));
    }
}
