//! État de l'application, messages et vues (architecture Elm d'iced).

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{
    Column, Row, button, column, container, row, scrollable, text, text_editor, text_input,
};
use iced::{Element, Fill, Font, Length, Task, Theme, padding};

use crate::connector::{
    self, ColumnInfo, Connector, QueryPlan, QueryResult, Result, TableRef, execution_order,
};
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
    Busy(&'static str),
    Info(String),
    Error(String),
}

pub struct App {
    url: String,
    connector: Option<Arc<dyn Connector>>,
    tables: Vec<TableRef>,
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
    UrlChanged(String),
    Connect,
    Connected(Result<Arc<dyn Connector>>),
    TablesLoaded(Result<Vec<TableRef>>),
    TableSelected(TableRef),
    SchemaLoaded(Result<Vec<ColumnInfo>>),
    Edit(text_editor::Action),
    Explain,
    Explained(Result<QueryPlan>),
    Run,
    QueryDone(Result<QueryResult>, Duration),
    TabSelected(Tab),
}

impl App {
    pub fn new() -> Self {
        Self {
            url: "sqlite://demo.db?mode=rwc".into(),
            connector: None,
            tables: Vec::new(),
            selected: None,
            schema: Vec::new(),
            editor: text_editor::Content::with_text("SELECT 1 AS hello"),
            result: None,
            plan: None,
            tab: Tab::Results,
            status: Status::Idle,
        }
    }

    pub fn title(&self) -> String {
        match &self.connector {
            Some(c) => format!("Quarry — {}", c.engine()),
            None => "Quarry".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        theme::quarry()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::UrlChanged(url) => self.url = url,

            Message::Connect => {
                self.status = Status::Busy("Connexion…");
                let url = self.url.trim().to_owned();
                return Task::perform(async move { connector::connect(&url).await }, Message::Connected);
            }
            Message::Connected(Ok(c)) => {
                self.status = Status::Info(format!("Connecté à {}", c.engine()));
                self.connector = Some(c.clone());
                self.tables.clear();
                self.selected = None;
                return Task::perform(async move { c.list_tables().await }, Message::TablesLoaded);
            }
            Message::TablesLoaded(Ok(tables)) => self.tables = tables,

            Message::TableSelected(table) => {
                let Some(c) = self.connector.clone() else { return Task::none() };
                self.editor = text_editor::Content::with_text(&c.preview_sql(&table));
                self.selected = Some(table.clone());
                self.tab = Tab::Schema;
                return Task::perform(
                    async move { c.describe_table(&table).await },
                    Message::SchemaLoaded,
                );
            }
            Message::SchemaLoaded(Ok(columns)) => self.schema = columns,

            Message::Edit(action) => self.editor.perform(action),

            Message::Explain => {
                let Some(c) = self.connector.clone() else { return Task::none() };
                self.status = Status::Busy("Analyse du plan…");
                let sql = self.editor.text();
                return Task::perform(async move { c.explain(&sql).await }, Message::Explained);
            }
            Message::Explained(Ok(plan)) => {
                self.status = Status::Idle;
                self.plan = Some(plan);
                self.tab = Tab::Plan;
            }

            Message::Run => {
                let Some(c) = self.connector.clone() else { return Task::none() };
                self.status = Status::Busy("Exécution…");
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

            Message::Connected(Err(e))
            | Message::TablesLoaded(Err(e))
            | Message::SchemaLoaded(Err(e))
            | Message::Explained(Err(e))
            | Message::QueryDone(Err(e), _) => self.status = Status::Error(e),
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let body = row![self.sidebar(), self.workspace()].spacing(12).height(Fill);
        column![self.top_bar(), body].spacing(12).padding(12).into()
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let url = text_input("postgres://user:pass@localhost/db", &self.url)
            .on_input(Message::UrlChanged)
            .on_submit(Message::Connect)
            .font(MONO)
            .padding(8);

        let (label, color) = match &self.status {
            Status::Idle => (String::new(), theme::MUTED),
            Status::Busy(msg) => (msg.to_string(), theme::WARNING),
            Status::Info(msg) => (msg.clone(), theme::SUCCESS),
            Status::Error(msg) => (msg.clone(), theme::DANGER),
        };

        row![
            text("Quarry").size(20).color(theme::PRIMARY),
            url,
            button("Connexion").on_press(Message::Connect).padding([8, 16]),
            container(text(label).size(13).color(color)).max_width(420),
        ]
        .spacing(12)
        .align_y(iced::Center)
        .into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let items = self.tables.iter().map(|table| {
            let active = self.selected.as_ref() == Some(table);
            button(text(table.to_string()).size(13))
                .on_press(Message::TableSelected(table.clone()))
                .style(theme::list_item(active))
                .width(Fill)
                .into()
        });

        let content = column![
            text(format!("TABLES ({})", self.tables.len())).size(11).color(theme::MUTED),
            scrollable(Column::with_children(items).spacing(2)).height(Fill),
        ]
        .spacing(8);

        container(content).style(theme::panel).padding(10).width(240).height(Fill).into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        let connected = self.connector.is_some();

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

        let content = column![
            text(total).size(15).color(theme::PRIMARY),
            section("ARBRE DU PLAN"),
            Column::with_children(tree).spacing(4),
            section("ORDRE D'EXÉCUTION"),
            Column::with_children(order).spacing(4),
        ]
        .spacing(10);

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
