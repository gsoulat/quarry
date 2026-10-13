//! Fenêtre de gestion des connexions : liste à gauche, formulaire à droite.

use iced::widget::{
    Column, button, center, column, container, opaque, pick_list, row, scrollable, text,
    text_input,
};
use iced::{Background, Color, Element, Fill, Font};

use crate::app::Message;
use crate::plugins::{self, Installed, Registry};
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
    SshHost,
    SshPort,
    SshUser,
    SshKey,
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
    pub ssh_host: String,
    pub ssh_port: String,
    pub ssh_user: String,
    pub ssh_key: String,
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
            ssh_host: String::new(),
            ssh_port: String::new(),
            ssh_user: String::new(),
            ssh_key: String::new(),
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
            ssh_host: config.ssh_host.clone(),
            ssh_port: config.ssh_port.map(|p| p.to_string()).unwrap_or_default(),
            ssh_user: config.ssh_user.clone(),
            ssh_key: config.ssh_key.clone(),
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
            Field::SshHost => &mut self.ssh_host,
            Field::SshPort => &mut self.ssh_port,
            Field::SshUser => &mut self.ssh_user,
            Field::SshKey => &mut self.ssh_key,
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
        let ssh_port = match self.ssh_port.trim() {
            "" => None,
            p => Some(p.parse::<u16>().map_err(|_| format!("Port SSH invalide : « {p} »"))?),
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
            ssh_host: self.ssh_host.trim().into(),
            ssh_port,
            ssh_user: self.ssh_user.trim().into(),
            ssh_key: self.ssh_key.trim().into(),
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
    /// Page « Connecteurs » affichée à la place du formulaire.
    pub plugins_page: bool,
    pub installed: Vec<Installed>,
    pub registry: Option<Registry>,
    /// Connecteur en cours de téléchargement.
    pub installing: Option<String>,
    pub plugins_feedback: Option<Feedback>,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            editing: None,
            form: Form::new(Engine::Postgres),
            feedback: None,
            show_password: false,
            plugins_page: false,
            installed: plugins::installed(),
            registry: None,
            installing: None,
            plugins_feedback: None,
        }
    }

    pub fn edit(config: &ConnectionConfig) -> Self {
        Self { editing: Some(config.id), form: Form::from_config(config), ..Self::new() }
    }

    pub fn is_installed(&self, engine: Engine) -> bool {
        engine.plugin_id().is_none_or(|id| self.installed.iter().any(|i| i.id == id))
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
            .style(theme::list_item(manager.editing.is_none() && !manager.plugins_page))
            .width(Fill),
        button(text(format!("Connecteurs ({} installés)", manager.installed.len())).size(13))
            .on_press(Message::OpenPlugins)
            .style(theme::list_item(manager.plugins_page))
            .width(Fill),
    ]
    .spacing(8)
    .width(220);

    let right: Element<'_, Message> = if manager.plugins_page { plugins_view(manager) } else { form_view(manager) };
    let content = row![sidebar, right].spacing(20);

    let dialog = container(content)
        .style(theme::panel)
        .padding(20)
        .width(880)
        .height(660);

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

    let mut fields = column![header].spacing(12);
    if !manager.is_installed(form.engine) {
        let size = manager
            .registry
            .as_ref()
            .and_then(|r| r.connectors.iter().find(|c| Some(c.id.as_str()) == form.engine.plugin_id()))
            .and_then(|c| c.asset())
            .map(|a| format!(" ({})", plugins::human_size(a.size)))
            .unwrap_or_default();
        fields = fields.push(
            container(
                row![
                    text(format!("Le connecteur {} n'est pas installé.", form.engine)).size(13).color(theme::WARNING).width(Fill),
                    button(text(format!("Télécharger{size}")).size(13))
                        .on_press(Message::OpenPlugins)
                        .padding([5, 12]),
                ]
                .spacing(10)
                .align_y(iced::Center),
            )
            .style(theme::header_cell)
            .padding([8, 12])
            .width(Fill),
        );
    }
    fields = fields.push(
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
    );

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
                .push(text("TUNNEL SSH (optionnel) — hôte et port ci-dessus sont alors vus depuis le serveur SSH ; clés ou agent SSH, pas de mot de passe").size(11).color(theme::MUTED))
                .push(
                    row![
                        input("Serveur SSH", "bastion.exemple.fr", &form.ssh_host, Field::SshHost).width(Fill),
                        input("Port SSH", "22", &form.ssh_port, Field::SshPort).width(100),
                        input("Utilisateur SSH", "ubuntu", &form.ssh_user, Field::SshUser).width(Fill),
                    ]
                    .spacing(12),
                )
                .push(input("Clé privée", "vide = ~/.ssh/id_* ou agent", &form.ssh_key, Field::SshKey))
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

    // Les champs défilent si la fenêtre est trop basse ; les boutons restent visibles.
    column![
        scrollable(fields).height(Fill),
        feedback.size(13).font(Font::MONOSPACE),
        actions,
    ]
    .spacing(14)
    .width(Fill)
    .into()
}

/// Page « Connecteurs » : installés, disponibles, mises à jour.
fn plugins_view(manager: &Manager) -> Element<'_, Message> {
    let header = row![
        text("Connecteurs").size(18).color(theme::TEXT).width(Fill),
        button(text("Actualiser").size(13)).on_press(Message::PluginsRefresh).style(button::secondary).padding([5, 12]),
        button(text("Fermer  ⎋").size(13)).on_press(Message::CloseManager).style(button::text),
    ]
    .spacing(8)
    .align_y(iced::Center);

    let intro = text(
        "SQLite est intégré. Les autres moteurs sont des connecteurs téléchargés depuis GitHub \
         (gsoulat/quarry-connectors), vérifiés par empreinte SHA-256 et installés dans le dossier de données de Quarry.",
    )
    .size(12)
    .color(theme::MUTED);

    let mut list = Column::new().spacing(6);
    match &manager.registry {
        None => list = list.push(text("Chargement du registre…").size(13).color(theme::MUTED)),
        Some(registry) => {
            for entry in &registry.connectors {
                let installed = manager.installed.iter().find(|i| i.id == entry.id);
                let busy = manager.installing.as_deref() == Some(entry.id.as_str());
                let asset = entry.asset();
                let status = match (installed, asset) {
                    (Some(i), _) if plugins::is_newer(&i.version, &entry.version) => {
                        format!("installé {} · mise à jour {} disponible", i.version, entry.version)
                    }
                    (Some(i), _) => format!("installé ({})", i.version),
                    (None, Some(a)) => format!("non installé · {}", plugins::human_size(a.size)),
                    (None, None) => format!("pas de version pour {}", plugins::TARGET),
                };
                let action: Element<'_, Message> = if busy {
                    text("Téléchargement…").size(12).color(theme::WARNING).into()
                } else {
                    match (installed, asset) {
                        (Some(i), Some(_)) if plugins::is_newer(&i.version, &entry.version) => {
                            row![
                                button(text("Mettre à jour").size(12)).on_press(Message::PluginInstall(entry.id.clone())).padding([4, 10]),
                                button(text("Supprimer").size(12))
                                    .on_press(Message::PluginRemove(entry.id.clone()))
                                    .style(theme::outline(theme::DANGER))
                                    .padding([4, 10]),
                            ]
                            .spacing(6)
                            .into()
                        }
                        (Some(_), _) => button(text("Supprimer").size(12))
                            .on_press(Message::PluginRemove(entry.id.clone()))
                            .style(theme::outline(theme::DANGER))
                            .padding([4, 10])
                            .into(),
                        (None, Some(_)) => button(text("Installer").size(12))
                            .on_press(Message::PluginInstall(entry.id.clone()))
                            .padding([4, 10])
                            .into(),
                        (None, None) => text("").into(),
                    }
                };
                list = list.push(
                    container(
                        row![
                            column![
                                text(&entry.engine).size(14).color(theme::TEXT),
                                text(&entry.description).size(12).color(theme::MUTED),
                                text(status).size(11).color(theme::MUTED),
                            ]
                            .spacing(2)
                            .width(Fill),
                            action,
                        ]
                        .spacing(12)
                        .align_y(iced::Center),
                    )
                    .style(theme::header_cell)
                    .padding([10, 12])
                    .width(Fill),
                );
            }
            // Installés mais absents du registre (ancien connecteur, build local).
            for i in manager.installed.iter().filter(|i| !registry.connectors.iter().any(|c| c.id == i.id)) {
                list = list.push(
                    row![
                        text(format!("{} (installé {}, hors registre)", i.id, i.version)).size(13).width(Fill),
                        button(text("Supprimer").size(12))
                            .on_press(Message::PluginRemove(i.id.clone()))
                            .style(theme::outline(theme::DANGER))
                            .padding([4, 10]),
                    ]
                    .align_y(iced::Center),
                );
            }
        }
    }

    let feedback = match &manager.plugins_feedback {
        None => text(""),
        Some(Feedback::Busy(m)) => text(m).color(theme::WARNING),
        Some(Feedback::Ok(m)) => text(m).color(theme::SUCCESS),
        Some(Feedback::Error(m)) => text(m).color(theme::DANGER),
    };

    column![header, intro, scrollable(list).height(Fill), feedback.size(13)]
        .spacing(12)
        .width(Fill)
        .into()
}
