//! Connexions enregistrées : la configuration va dans un fichier JSON du
//! dossier de configuration, les mots de passe dans le trousseau du système
//! (Keychain sur macOS, Gestionnaire d'identification sur Windows,
//! Secret Service sur Linux). Aucun secret n'est écrit sur le disque en clair.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "quarry";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Engine {
    Postgres,
    MySql,
    Sqlite,
    BigQuery,
}

impl Engine {
    pub const ALL: [Engine; 4] = [Engine::Postgres, Engine::MySql, Engine::Sqlite, Engine::BigQuery];

    pub fn default_port(self) -> Option<u16> {
        match self {
            Engine::Postgres => Some(5432),
            Engine::MySql => Some(3306),
            Engine::Sqlite | Engine::BigQuery => None,
        }
    }

    /// Moteur client/serveur (hôte, port, utilisateur, mot de passe).
    pub fn is_server(self) -> bool {
        matches!(self, Engine::Postgres | Engine::MySql)
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Engine::Postgres => "PostgreSQL",
            Engine::MySql => "MySQL",
            Engine::Sqlite => "SQLite",
            Engine::BigQuery => "BigQuery",
        })
    }
}

/// Groupe d'affichage dans la barre latérale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Local,
    Remote,
    Cloud,
}

impl Location {
    pub const ALL: [Location; 3] = [Location::Local, Location::Remote, Location::Cloud];

    pub fn label(self) -> &'static str {
        match self {
            Location::Local => "LOCAL",
            Location::Remote => "DISTANT",
            Location::Cloud => "CLOUD",
        }
    }
}

/// Une connexion enregistrée. Les champs inutiles pour un moteur restent vides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: u64,
    pub name: String,
    pub engine: Engine,
    /// Dossier de rangement dans la barre latérale (vide = aucun).
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub database: String,
    /// SQLite : chemin du fichier.
    #[serde(default)]
    pub path: String,
    /// BigQuery : projet GCP qui exécute (et paie) les requêtes.
    #[serde(default)]
    pub project_id: String,
    /// BigQuery : clé JSON d'un compte de service. Vide = identifiants par
    /// défaut de l'application (`gcloud auth application-default login`).
    #[serde(default)]
    pub credentials_path: String,
}

impl ConnectionConfig {
    /// Groupe dans lequel ranger la connexion dans la barre latérale.
    pub fn location(&self) -> Location {
        // TODO(toi) : décider ce qui compte comme « local ». Cette version
        // provisoire ne range en local que SQLite.
        match self.engine {
            Engine::Sqlite => Location::Local,
            Engine::BigQuery => Location::Cloud,
            Engine::Postgres | Engine::MySql => Location::Remote,
        }
    }

    /// Résumé affiché sous le nom (« localhost:5432/app », « mon-projet »…).
    pub fn summary(&self) -> String {
        match self.engine {
            Engine::Sqlite => self.path.clone(),
            Engine::BigQuery => self.project_id.clone(),
            Engine::Postgres | Engine::MySql => {
                let port = self.port.or(self.engine.default_port()).unwrap_or_default();
                format!("{}:{port}/{}", self.host, self.database)
            }
        }
    }
}

fn config_file() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("fr", "gsoulat", "Quarry")?;
    Some(dirs.config_dir().join("connections.json"))
}

/// Charge les connexions ; un fichier absent donne une liste vide.
pub fn load() -> Result<Vec<ConnectionConfig>, String> {
    let Some(path) = config_file() else { return Ok(Vec::new()) };
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn save(connections: &[ConnectionConfig]) -> Result<(), String> {
    let path = config_file().ok_or("Dossier de configuration introuvable")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(connections).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))
}

fn keyring_entry(id: u64) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("connection-{id}")).map_err(|e| e.to_string())
}

pub fn password(id: u64) -> Result<Option<String>, String> {
    match keyring_entry(id)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Trousseau : {e}")),
    }
}

pub fn set_password(id: u64, password: &str) -> Result<(), String> {
    keyring_entry(id)?.set_password(password).map_err(|e| format!("Trousseau : {e}"))
}

pub fn delete_password(id: u64) -> Result<(), String> {
    match keyring_entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Trousseau : {e}")),
    }
}

// ----------------------------------------------------------------------
// Requêtes enregistrées et historique, par connexion
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub sql: String,
    /// Horodatage lisible (heure locale).
    pub at: String,
    pub rows: usize,
    pub ms: u128,
}

const HISTORY_MAX: usize = 200;

fn per_connection_file(kind: &str, id: u64) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("fr", "gsoulat", "Quarry")?;
    Some(dirs.config_dir().join(kind).join(format!("{id}.json")))
}

fn read_json<T: serde::de::DeserializeOwned + Default>(path: Option<PathBuf>) -> T {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: Option<PathBuf>, value: &T) -> Result<(), String> {
    let path = path.ok_or("Dossier de configuration introuvable")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load_queries(connection: u64) -> Vec<SavedQuery> {
    read_json(per_connection_file("queries", connection))
}

pub fn save_queries(connection: u64, queries: &[SavedQuery]) -> Result<(), String> {
    write_json(per_connection_file("queries", connection), &queries)
}

pub fn load_history(connection: u64) -> Vec<HistoryEntry> {
    read_json(per_connection_file("history", connection))
}

/// Ajoute une entrée en tête et borne la taille de l'historique.
pub fn push_history(connection: u64, entry: HistoryEntry) -> Result<Vec<HistoryEntry>, String> {
    let mut history = load_history(connection);
    history.insert(0, entry);
    history.truncate(HISTORY_MAX);
    write_json(per_connection_file("history", connection), &history)?;
    Ok(history)
}
