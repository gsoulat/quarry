//! Fenêtre de gestion des connexions : liste à gauche, formulaire à droite.

use iced::widget::{
    Column, button, center, column, container, opaque, pick_list, row, scrollable, text,
    text_input,
};
use iced::{Background, Color, Element, Fill, Font};

use crate::app::Message;
use crate::store::{ConnectionConfig, Engine};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Folder,
    Host,
    Port,
    User,
    Password,
    Database,
    Path,
    ProjectId,
    CredentialsPath,
}

/// Valeurs saisies, gardées en texte tant qu'elles ne sont pas validées.
#[derive(Debug, Clone)]
pub struct Form {
    pub engine: Engine,
    pub name: String,
    pub folder: String,
    pub host: String,
    pub port: String,
    pub user: String,
    /// Vide = mot de passe inchangé (pour une connexion existante).
    pub password: String,
    pub database: String,
    pub path: String,
    pub project_id: String,
    pub credentials_path: String,
}

impl Form {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            name: String::new(),
            folder: String::new(),
            host: "localhost".into(),
            port: engine.default_port().map(|p| p.to_string()).unwrap_or_default(),
            user: String::new(),
            password: String::new(),
            database: String::new(),
            path: String::new(),
            project_id: String::new(),
            credentials_path: String::new(),
        }
    }

    pub fn from_config(config: &ConnectionConfig) -> Self {
        Self {
            engine: config.engine,
            name: config.name.clone(),
            folder: config.folder.clone(),
            host: config.host.clone(),
            port: config.port.map(|p| p.to_string()).unwrap_or_default(),
            user: config.user.clone(),
            password: String::new(),
            database: config.database.clone(),
            path: config.path.clone(),
            project_id: config.project_id.clone(),
            credentials_path: config.credentials_path.clone(),
        }
    }

    pub fn set(&mut self, field: Field, value: String) {
        let target = match field {
            Field::Name => &mut self.name,
            Field::Folder => &mut self.folder,
            Field::Host => &mut self.host,
            Field::Port => &mut self.port,
            Field::User => &mut self.user,
            Field::Password => &mut self.password,
            Field::Database => &mut self.database,
            Field::Path => &mut self.path,
            Field::ProjectId => &mut self.project_id,
            Field::CredentialsPath => &mut self.credentials_path,
        };
        *target = value;
    }

    /// Change de moteur en gardant un port cohérent.
    pub fn set_engine(&mut self, engine: Engine) {
        let old_default = self.engine.default_port().map(|p| p.to_string()).unwrap_or_default();
        if self.port.is_empty() || self.port == old_default {
            self.port = engine.default_port().map(|p| p.to_string()).unwrap_or_default();
        }
        self.engine = engine;
    }

    pub fn to_config(&self, id: u64) -> Result<ConnectionConfig, String> {
        let required = |value: &str, label: &str| {
            if value.trim().is_empty() { Err(format!("Champ « {label} » obligatoire")) } else { Ok(()) }
        };
        required(&self.name, "Nom")?;
        match self.engine {
            Engine::Postgres | Engine::MySql => {
                required(&self.host, "Hôte")?;
                required(&self.user, "Utilisateur")?;
            }
            Engine::Sqlite => required(&self.path, "Fichier")?,
            Engine::BigQuery => required(&self.project_id, "Projet GCP")?,
        }
        let port = match self.port.trim() {
            "" => None,
            p => Some(p.parse::<u16>().map_err(|_| format!("Port invalide : « {p} »"))?),
        };
        Ok(ConnectionConfig {
            id,
            name: self.name.trim().into(),
            engine: self.engine,
            folder: self.folder.trim().into(),
            host: self.host.trim().into(),
            port,
            user: self.user.trim().into(),
            database: self.database.trim().into(),
            path: self.path.trim().into(),
            project_id: self.project_id.trim().into(),
            credentials_path: self.credentials_path.trim().into(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum Feedback {
    Busy(String),
    Ok(String),
    Error(String),
}

pub struct Manager {
    /// `None` : nouvelle connexion.
    pub editing: Option<u64>,
    pub form: Form,
    pub feedback: Option<Feedback>,
    pub show_password: bool,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            editing: None,
            form: Form::new(Engine::Postgres),
            feedback: None,
            show_password: false,
        }
    }

    pub fn edit(config: &ConnectionConfig) -> Self {
        Self {
            editing: Some(config.id),
            form: Form::from_config(config),
            feedback: None,
            show_password: false,
        }
    }
}

pub fn view<'a>(manager: &'a Manager, connections: &'a [ConnectionConfig]) -> Element<'a, Message> {
    let list = connections.iter().map(|c| {
        let active = manager.editing == Some(c.id);
        button(column![
            text(&c.name).size(13),
            text(c.engine.to_string()).size(11).color(theme::MUTED),
        ])
        .on_press(Message::EditConnection(Some(c.id)))
        .style(theme::list_item(active))
        .width(Fill)
        .into()
    });

    let sidebar = column![
        text("CONNEXIONS").size(11).color(theme::MUTED),
        scrollable(Column::with_children(list).spacing(2)).height(Fill),
        button(text("+ Nouvelle connexion").size(13))
            .on_press(Message::EditConnection(None))
            .style(theme::list_item(manager.editing.is_none()))
            .width(Fill),
    ]
    .spacing(8)
    .width(220);

    let content = row![sidebar, form_view(manager)].spacing(20);

    let dialog = container(content)
        .style(theme::panel)
        .padding(20)
        .width(860)
        .height(560);

    // Fond assombri. La fenêtre se ferme par Échap ou par le bouton,
    // jamais par un clic à côté : on ne perd pas ce qu'on a saisi.
    opaque(center(opaque(dialog)).style(|_| container::Style {
        background: Some(Background::Color(Color { a: 0.7, ..Color::BLACK })),
        ..container::Style::default()
    }))
}

fn form_view(manager: &Manager) -> Element<'_, Message> {
    let form = &manager.form;
    let input = |label: &'static str, placeholder: &'static str, value: &str, field: Field| {
        column![
            text(label).size(12).color(theme::MUTED),
            text_input(placeholder, value)
                .on_input(move |v| Message::FormChanged(field, v))
                .secure(field == Field::Password && !manager.show_password)
                .padding(8),
        ]
        .spacing(4)
    };

    let title = match manager.editing {
        Some(_) => "Modifier la connexion",
        None => "Nouvelle connexion",
    };

    let header = row![
        text(title).size(18).color(theme::TEXT).width(Fill),
        button(text("Fermer  ⎋").size(13))
            .on_press(Message::CloseManager)
            .style(button::text),
    ]
    .align_y(iced::Center);

    let mut fields = column![
        header,
        row![
            input("Nom", "Production, Docker local…", &form.name, Field::Name).width(Fill),
            input("Dossier", "Clients, Perso… (optionnel)", &form.folder, Field::Folder).width(Fill),
            column![
                text("Moteur").size(12).color(theme::MUTED),
                pick_list(Engine::ALL, Some(form.engine), Message::FormEngine).padding(8),
            ]
            .spacing(4),
        ]
        .spacing(12),
    ]
    .spacing(12);

    fields = match form.engine {
        Engine::Postgres | Engine::MySql => {
            let password_hint = if manager.editing.is_some() {
                "(inchangé)"
            } else {
                "stocké dans le trousseau"
            };
            fields
                .push(
                    row![
                        input("Hôte", "localhost", &form.host, Field::Host).width(Fill),
                        input("Port", "", &form.port, Field::Port).width(100),
                    ]
                    .spacing(12),
                )
                .push(
                    row![
                        input("Utilisateur", "postgres", &form.user, Field::User).width(Fill),
                        input("Mot de passe", password_hint, &form.password, Field::Password)
                            .width(Fill),
                        column![
                            text("").size(12),
                            button(text(if manager.show_password { "Masquer" } else { "Voir" }).size(13))
                                .on_press(Message::TogglePassword)
                                .style(button::secondary)
                                .padding([8, 10]),
                        ]
                        .spacing(4),
                    ]
                    .align_y(iced::alignment::Vertical::Bottom)
                    .spacing(12),
                )
                .push(input("Base de données", "optionnel", &form.database, Field::Database))
        }
        Engine::Sqlite => fields.push(input(
            "Fichier",
            "/chemin/vers/base.db (créé s'il n'existe pas)",
            &form.path,
            Field::Path,
        )),
        Engine::BigQuery => fields
            .push(input("Projet GCP", "mon-projet-123", &form.project_id, Field::ProjectId))
            .push(input(
                "Clé de compte de service (JSON)",
                "vide = gcloud auth application-default login",
                &form.credentials_path,
                Field::CredentialsPath,
            )),
    };

    let feedback = match &manager.feedback {
        None => text(""),
        Some(Feedback::Busy(msg)) => text(msg).color(theme::WARNING),
        Some(Feedback::Ok(msg)) => text(msg).color(theme::SUCCESS),
        Some(Feedback::Error(msg)) => text(msg).color(theme::DANGER),
    };

    let mut actions = row![
        button("Tester").on_press(Message::TestConnection).style(button::secondary).padding([6, 14]),
        button("Enregistrer")
            .on_press(Message::SaveConnection { connect: false })
            .style(button::secondary)
            .padding([6, 14]),
        button("Enregistrer et se connecter")
            .on_press(Message::SaveConnection { connect: true })
            .padding([6, 14]),
    ]
    .spacing(8);
    if manager.editing.is_some() {
        actions = actions.push(
            button("Supprimer")
                .on_press(Message::DeleteConnection)
                .style(button::danger)
                .padding([6, 14]),
        );
    }

    column![
        fields,
        container(feedback.size(13).font(Font::MONOSPACE)).height(Fill),
        actions,
    ]
    .spacing(16)
    .width(Fill)
    .into()
}
