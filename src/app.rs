//! État de l'application, messages et vues (architecture Elm d'iced).

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{
    Column, Row, button, column, container, row, scrollable, stack, text, text_editor,
};
use iced::{Element, Fill, Font, Length, Task, Theme, padding};

use crate::connector::{
    self, ColumnInfo, Connector, QueryPlan, QueryResult, Result, TableRef, execution_order,
};
use crate::manager::{self, Feedback, Field, Manager};
use crate::store::{self, ConnectionConfig, Engine, Location};
use crate::theme;

const CELL_WIDTH: f32 = 180.0;
const MONO: Font = Font::MONOSPACE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Results,
    Plan,
    Schema,
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
    selected: Option<TableRef>,
    schema: Vec<ColumnInfo>,
    editor: text_editor::Content,
    result: Option<QueryResult>,
    plan: Option<QueryPlan>,
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
    SchemaLoaded(Result<Vec<ColumnInfo>>),

    // Gestionnaire de connexions
    OpenManager,
    CloseManager,
    EditConnection(Option<u64>),
    FormChanged(Field, String),
    FormEngine(Engine),
    TestConnection,
    TestDone(Result<String>),
    SaveConnection { connect: bool },
    DeleteConnection,

    // Espace de travail
    Edit(text_editor::Action),
    Explain,
    Explained(Result<QueryPlan>),
    Run,
    QueryDone(Result<QueryResult>, Duration),
    TabSelected(Tab),
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

        Self {
            connections,
            session: None,
            connecting: None,
            manager,
            selected: None,
            schema: Vec::new(),
            editor: text_editor::Content::with_text("SELECT 1 AS hello"),
            result: None,
            plan: None,
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

    fn active_config(&self) -> Option<&ConnectionConfig> {
        let id = self.session.as_ref()?.id;
        self.connections.iter().find(|c| c.id == id)
    }

    fn connector(&self) -> Option<Arc<dyn Connector>> {
        self.session.as_ref().map(|s| s.connector.clone())
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
            Message::SchemaLoaded(Ok(columns)) => self.schema = columns,

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
                let sql = self.editor.text();
                return Task::perform(async move { c.explain(&sql).await }, Message::Explained);
            }
            Message::Explained(Ok(plan)) => {
                self.status = Status::Idle;
                self.plan = Some(plan);
                self.tab = Tab::Plan;
            }
            Message::Run => {
                let Some(c) = self.connector() else { return Task::none() };
                self.status = Status::Busy("Exécution…".into());
                let sql = self.editor.text();
                return Task::perform(
                    async move {
                        let start = Instant::now();
                        let result = c.query(&sql).await;
                        (result, start.elapsed())
                    },
                    |(result, elapsed)| Message::QueryDone(result, elapsed),
                );
            }
            Message::QueryDone(Ok(result), elapsed) => {
                self.status = Status::Info(format!(
                    "{} ligne(s) en {} ms",
                    result.rows.len(),
                    elapsed.as_millis()
                ));
                self.result = Some(result);
                self.tab = Tab::Results;
            }
            Message::TabSelected(tab) => self.tab = tab,

            Message::TablesLoaded(Err(e))
            | Message::SchemaLoaded(Err(e))
            | Message::Explained(Err(e))
            | Message::QueryDone(Err(e), _) => self.status = Status::Error(e),
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

    pub fn view(&self) -> Element<'_, Message> {
        let body = row![self.sidebar(), self.workspace()].spacing(12).height(Fill);
        let base = column![self.top_bar(), body].spacing(12).padding(12);

        match &self.manager {
            Some(m) => stack![base, manager::view(m, &self.connections)].into(),
            None => base.into(),
        }
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let (label, color) = match &self.status {
            Status::Idle => (String::new(), theme::MUTED),
            Status::Busy(msg) => (msg.clone(), theme::WARNING),
            Status::Info(msg) => (msg.clone(), theme::SUCCESS),
            Status::Error(msg) => (msg.clone(), theme::DANGER),
        };

        let active = match self.active_config() {
            Some(c) => row![
                text(&c.name).size(14).color(theme::TEXT),
                text(c.summary()).size(12).font(MONO).color(theme::MUTED),
                button(text("Déconnecter").size(12))
                    .on_press(Message::Disconnect)
                    .style(button::text),
            ]
            .spacing(10)
            .align_y(iced::Center),
            None => row![text("Aucune connexion").size(14).color(theme::MUTED)],
        };

        row![
            text("Quarry").size(20).color(theme::PRIMARY),
            container(active).width(Fill),
            container(text(label).size(13).color(color)).max_width(520),
        ]
        .spacing(16)
        .align_y(iced::Center)
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

        container(column![header, scrollable(list).height(Fill)].spacing(4))
            .style(theme::panel)
            .padding(10)
            .width(260)
            .height(Fill)
            .into()
    }

    /// Une connexion ; si elle est ouverte, ses tables s'affichent dessous.
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
                    text(&config.name).size(13),
                    text(format!("{} · {}", config.engine, config.summary()))
                        .size(11)
                        .color(theme::MUTED),
                ],
            ]
            .spacing(8)
            .align_y(iced::Center),
        )
        .on_press(Message::Connect(config.id))
        .style(theme::list_item(session.is_some()))
        .width(Fill);

        let Some(session) = session else { return entry.into() };

        let tables = session.tables.iter().map(|table| {
            let active = self.selected.as_ref() == Some(table);
            button(text(table.to_string()).size(12).font(MONO))
                .on_press(Message::TableSelected(table.clone()))
                .style(theme::list_item(active))
                .width(Fill)
                .into()
        });

        column![
            entry,
            container(Column::with_children(tables).spacing(1)).padding(padding::left(18)),
        ]
        .spacing(2)
        .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        let connected = self.session.is_some();

        let editor = text_editor(&self.editor)
            .on_action(Message::Edit)
            .font(MONO)
            .height(160)
            .padding(10);

        let actions = row![
            button("Expliquer")
                .on_press_maybe(connected.then_some(Message::Explain))
                .style(button::secondary)
                .padding([6, 14]),
            button("Exécuter")
                .on_press_maybe(connected.then_some(Message::Run))
                .padding([6, 14]),
        ]
        .spacing(8);

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
            return hint("Écris une requête puis clique sur « Exécuter ».");
        };
        if result.columns.is_empty() {
            return hint("La requête n'a renvoyé aucune ligne.");
        }
        let rows = result.rows.iter().map(|r| r.iter().map(|c| c.display()).collect());
        grid(result.columns.clone(), rows)
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
            return hint("Clique sur « Expliquer » pour voir le plan sans exécuter la requête.");
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
                text(format!("└ {}", node.label)).font(MONO).size(13).width(Fill),
                text(stats.join(" · ")).size(12).color(theme::MUTED),
            ]
            .padding(padding::left(node.depth as f32 * 20.0))
            .into()
        });

        let order = execution_order(&plan.nodes).into_iter().enumerate().map(|(step, i)| {
            text(format!("{}. {}", step + 1, plan.nodes[i].label)).font(MONO).size(13).into()
        });

        let mut content = column![text(total).size(15).color(theme::PRIMARY)].spacing(10);
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

fn hint(message: &str) -> Element<'_, Message> {
    container(text(message).color(theme::MUTED)).center(Fill).into()
}

fn section(title: &str) -> Element<'_, Message> {
    text(title).size(11).color(theme::MUTED).into()
}

/// Grille défilable dans les deux sens, avec en-tête et lignes alternées.
fn grid<'a>(
    columns: Vec<String>,
    rows: impl Iterator<Item = Vec<String>>,
) -> Element<'a, Message> {
    let cell = |value: String| {
        container(text(value).font(MONO).size(13).wrapping(text::Wrapping::None))
            .width(CELL_WIDTH)
            .padding([4, 8])
            .clip(true)
    };

    let header = Row::with_children(columns.into_iter().map(|c| cell(c).into()));
    let header = container(header).style(theme::header_cell);

    let lines = rows.enumerate().map(|(i, values)| {
        container(Row::with_children(values.into_iter().map(|v| cell(v).into())))
            .style(theme::grid_row(i % 2 == 1))
            .into()
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
