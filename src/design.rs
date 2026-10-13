//! Mode conception du diagramme : panneau d'édition de la table
//! sélectionnée, choix du type de relation après un glisser-déposer,
//! génération du script.

use iced::widget::{
    Column, button, center, checkbox, column, container, opaque, pick_list, row, scrollable, text,
    text_input,
};
use iced::{Background, Color, Element, Fill, Font};

use crate::app::Message;
use crate::diagram::{Diagram, LinkKind};
use crate::editing::type_options;
use crate::store::Engine;
use crate::theme;

const MONO: Font = Font::MONOSPACE;
const CUSTOM_TYPE: &str = "Personnalisé…";

#[derive(Debug, Clone)]
pub enum DesignMessage {
    /// Entre ou sort du mode conception.
    Toggle,
    /// Repart d'un diagramme vide.
    Clear,
    AddTable,
    RemoveTable,
    TableName(String),
    ColumnName(usize, String),
    ColumnType(usize, String),
    ColumnCustom(usize),
    ColumnNullable(usize, bool),
    ColumnPrimary(usize, bool),
    AddColumn,
    RemoveColumn(usize),
    RemoveRelation(usize),
    /// Choix du type de relation après un dépôt sur une table.
    LinkChosen(LinkKind),
    LinkCancel,
    /// Génère le script et l'affiche pour confirmation.
    Script,
}

/// État du panneau, en plus du diagramme lui-même.
#[derive(Debug, Default)]
pub struct Design {
    /// Colonnes (table, colonne) en saisie libre du type.
    pub custom: std::collections::HashSet<(usize, usize)>,
    /// Liaison en attente de choix (depuis, vers).
    pub pending_link: Option<(usize, usize)>,
}

pub fn panel<'a>(design: &'a Design, diagram: &'a Diagram, engine: Engine) -> Element<'a, Message> {
    let msg = |m: DesignMessage| Message::Design(m);

    let Some(index) = diagram.selected else {
        return container(
            column![
                text("Conception").size(15).color(theme::PRIMARY),
                text("Clique sur une table pour la modifier, ou sur « + Table » pour en créer une.")
                    .size(12)
                    .color(theme::MUTED),
                text("Pour lier deux tables : tire la poignée ● d'une table jusqu'à l'autre.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(8),
        )
        .style(theme::panel)
        .padding(12)
        .width(340)
        .height(Fill)
        .into();
    };
    let node = &diagram.nodes[index];

    let name = text_input("nom de la table", &node.table.name)
        .on_input(move |v| Message::Design(DesignMessage::TableName(v)))
        .font(MONO)
        .size(13)
        .padding(6);

    let header = row![
        container(text("Nom")).width(Fill),
        container(text("Type")).width(Fill),
        container(text("NULL")).width(50),
        container(text("PK")).width(40),
        container(text("")).width(26),
    ]
    .spacing(6);

    let columns = node.columns.iter().enumerate().map(|(i, col)| {
        let type_widget: Element<'_, Message> = if design.custom.contains(&(index, i)) {
            row![
                text_input("VARCHAR(100)…", &col.data_type)
                    .on_input(move |v| Message::Design(DesignMessage::ColumnType(i, v)))
                    .font(MONO)
                    .size(12)
                    .padding(5)
                    .width(Fill),
                button(text("≡").size(11))
                    .on_press(msg(DesignMessage::ColumnCustom(i)))
                    .style(button::text)
                    .padding([3, 5]),
            ]
            .spacing(2)
            .into()
        } else {
            let mut options: Vec<String> = type_options(engine).iter().map(|t| t.to_string()).collect();
            if !col.data_type.is_empty() && !options.contains(&col.data_type) {
                options.insert(0, col.data_type.clone());
            }
            options.push(CUSTOM_TYPE.into());
            let selected = (!col.data_type.is_empty()).then(|| col.data_type.clone());
            pick_list(options, selected, move |choice: String| {
                if choice == CUSTOM_TYPE {
                    Message::Design(DesignMessage::ColumnCustom(i))
                } else {
                    Message::Design(DesignMessage::ColumnType(i, choice))
                }
            })
            .placeholder("Type")
            .font(MONO)
            .text_size(12)
            .padding(5)
            .width(Fill)
            .into()
        };

        row![
            text_input("nom", &col.name)
                .on_input(move |v| Message::Design(DesignMessage::ColumnName(i, v)))
                .font(MONO)
                .size(12)
                .padding(5)
                .width(Fill),
            container(type_widget).width(Fill),
            container(
                checkbox(col.nullable && !col.primary_key)
                    .on_toggle(move |v| Message::Design(DesignMessage::ColumnNullable(i, v)))
            )
            .width(50),
            container(
                checkbox(col.primary_key).on_toggle(move |v| Message::Design(DesignMessage::ColumnPrimary(i, v)))
            )
            .width(40),
            button(text("×").size(12))
                .on_press(msg(DesignMessage::RemoveColumn(i)))
                .style(button::text)
                .padding([2, 6]),
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into()
    });

    let relations = diagram.relations.iter().enumerate().filter(|(_, r)| r.from == node.table || r.to == node.table).map(
        |(i, r)| {
            let label = if r.from == node.table {
                format!("{} → {}.{}", r.from_columns.join(", "), r.to.name, r.to_columns.join(", "))
            } else {
                format!("{}.{} → {}", r.from.name, r.from_columns.join(", "), r.to_columns.join(", "))
            };
            row![
                text(label).size(11).font(MONO).color(theme::MUTED).width(Fill),
                button(text("×").size(12))
                    .on_press(msg(DesignMessage::RemoveRelation(i)))
                    .style(button::text)
                    .padding([2, 6]),
            ]
            .align_y(iced::Center)
            .into()
        },
    );

    let existing_note = if node.existing {
        "Table déjà en base : elle ne sera pas dans le script (les modifications passent par le menu Édition)."
    } else {
        "Nouvelle table : elle sera créée par le script."
    };

    let content = column![
        row![
            text("Table").size(15).color(theme::PRIMARY).width(Fill),
            button(text("Supprimer").size(12))
                .on_press(msg(DesignMessage::RemoveTable))
                .style(theme::outline(theme::DANGER))
                .padding([3, 8]),
        ]
        .align_y(iced::Center),
        name,
        text(existing_note).size(11).color(theme::MUTED),
        text("COLONNES").size(11).color(theme::MUTED),
        header,
        Column::with_children(columns).spacing(4),
        button(text("+ Colonne").size(12))
            .on_press(msg(DesignMessage::AddColumn))
            .style(button::secondary)
            .padding([3, 8]),
        text("RELATIONS").size(11).color(theme::MUTED),
        Column::with_children(relations).spacing(2),
    ]
    .spacing(8);

    container(scrollable(content)).style(theme::panel).padding(12).width(340).height(Fill).into()
}

/// Fenêtre de choix du type de relation après un glisser-déposer.
pub fn link_prompt<'a>(diagram: &'a Diagram, from: usize, to: usize) -> Element<'a, Message> {
    let (a, b) = (&diagram.nodes[from].table.name, &diagram.nodes[to].table.name);
    let choice = |label: String, detail: String, kind: LinkKind| {
        button(column![text(label).size(14), text(detail).size(11).color(theme::MUTED)].spacing(2))
            .on_press(Message::Design(DesignMessage::LinkChosen(kind)))
            .style(theme::list_item(false))
            .padding([8, 12])
            .width(Fill)
    };
    let content = column![
        text(format!("Relier {a} et {b}")).size(18).color(theme::TEXT),
        choice(
            format!("Un {a} a plusieurs {b}  (1 → n)"),
            format!("Ajoute {}_id dans {b}", singular_hint(a)),
            LinkKind::OneToMany,
        ),
        choice(
            format!("Plusieurs {a} pour un {b}  (n → 1)"),
            format!("Ajoute {}_id dans {a}", singular_hint(b)),
            LinkKind::ManyToOne,
        ),
        choice(
            format!("Plusieurs {a} pour plusieurs {b}  (n ↔ n)"),
            format!("Crée une table de jonction {}_{b}", singular_hint(a)),
            LinkKind::ManyToMany,
        ),
        button(text("Annuler").size(13))
            .on_press(Message::Design(DesignMessage::LinkCancel))
            .style(button::secondary)
            .padding([6, 14]),
    ]
    .spacing(10);

    let panel = container(content).style(theme::panel).padding(20).width(520);
    opaque(center(opaque(panel)).style(|_| container::Style {
        background: Some(Background::Color(Color { a: 0.7, ..Color::BLACK })),
        ..container::Style::default()
    }))
}

fn singular_hint(name: &str) -> String {
    name.strip_suffix('s').unwrap_or(name).to_string()
}
