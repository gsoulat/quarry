//! État de l'application, messages et vues (architecture Elm d'iced).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::text_editor::{Action, Binding, Edit, KeyPress};
use iced::widget::{
    Column, Row, button, column, container, pane_grid, row, scrollable, stack, text, text_editor,
    text_input, tooltip,
};
use iced::{Element, Event, Fill, Font, Length, Subscription, Task, Theme, highlighter, padding};

use crate::connector::{
    self, ColumnInfo, Connector, QueryPlan, QueryResult, Result, TableRef, execution_order,
};
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
    Plan,
    Schema,
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
    PaneResized(pane_grid::ResizeEvent),

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
                self.status = Status::Idle;
            }
            Message::TablesLoaded(Ok(tables)) => {
                if let Some(session) = &mut self.session {
                    session.tables = tables;
                }
            }
            Message::TableSelected(table) => {
                let Some(c) = self.connector() else { return Task::none() };
                self.editor = text_editor::Content::with_text(&c.preview_sql(&table));
                self.selected = Some(table.clone());
                self.tab = Tab::Schema;
                return Task::perform(
                    async move { c.describe_table(&table).await },
                    Message::SchemaLoaded,
                );
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
            Message::PaneResized(event) => self.panes.resize(event.split, event.ratio),

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
            Message::TabSelected(tab) => self.tab = tab,
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

        match &self.manager {
            Some(m) => stack![base, manager::view(m, &self.connections)].into(),
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

        row![
            text("Quarry").size(20).color(theme::PRIMARY),
            container(active).width(Fill),
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

        for location in Location::ALL {
            let group: Vec<_> =
                self.connections.iter().filter(|c| c.location() == location).collect();
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

        let entry = button(
            row![
                marker.size(10),
                column![
                    text(&config.name).size(13).wrapping(text::Wrapping::None),
                    text(format!("{} · {}", config.engine, config.summary()))
                        .size(11)
                        .color(theme::MUTED)
                        .wrapping(text::Wrapping::None),
                ],
            ]
            .spacing(8)
            .align_y(iced::Center),
        )
        .on_press(Message::Connect(config.id))
        .style(theme::list_item(session.is_some()))
        .width(Fill)
        .clip(true);

        let Some(session) = session else { return entry.into() };

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
            tab("Plan d'exécution", Tab::Plan),
            tab("Schéma", Tab::Schema),
        ]
        .spacing(4);

        let content = match self.tab {
            Tab::Results => self.results_view(),
            Tab::Plan => self.plan_view(),
            Tab::Schema => self.schema_view(),
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
        let grid = grid(result.columns.clone(), rows);

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
        column![text(table.to_string()).size(15).color(theme::PRIMARY), grid(header, rows)]
            .spacing(8)
            .into()
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

fn hint(message: &str) -> Element<'_, Message> {
    container(text(message).color(theme::MUTED)).center(Fill).into()
}

fn section(title: &str) -> Element<'_, Message> {
    text(title).size(11).color(theme::MUTED).into()
}

/// Grille défilable dans les deux sens. Les colonnes prennent la largeur de
/// leur contenu (bornée) et un clic sur une cellule copie sa valeur.
fn grid<'a>(columns: Vec<String>, rows: impl Iterator<Item = Vec<String>>) -> Element<'a, Message> {
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
            button(label)
                .on_press(Message::CopyCell(value))
                .style(theme::cell)
                .width(w)
                .padding([3, 8])
                .clip(true)
                .into()
        });
        container(Row::with_children(cells)).style(theme::grid_row(i % 2 == 1)).into()
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
