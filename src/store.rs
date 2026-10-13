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

    /// Identifiant du connecteur téléchargeable ; `None` pour SQLite, intégré.
    pub fn plugin_id(self) -> Option<&'static str> {
        match self {
            Engine::Postgres => Some("postgres"),
            Engine::MySql => Some("mysql"),
            Engine::BigQuery => Some("bigquery"),
            Engine::Sqlite => None,
        }
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
    /// Tunnel SSH (vide = connexion directe). `host`/`port` désignent alors
    /// la base **vue depuis** le serveur SSH.
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default)]
    pub ssh_port: Option<u16>,
    #[serde(default)]
    pub ssh_user: String,
    /// Chemin d'une clé privée ; vide = clés par défaut ou agent SSH.
    #[serde(default)]
    pub ssh_key: String,
}

impl ConnectionConfig {
    pub fn uses_tunnel(&self) -> bool {
        self.engine.is_server() && !self.ssh_host.trim().is_empty()
    }
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
                let via = if self.uses_tunnel() { format!(" via ssh {}", self.ssh_host.trim()) } else { String::new() };
                format!("{}:{port}/{}{via}", self.host, self.database)
            }
        }
    }
}

/// Verrou partagé par les tests qui modifient `QUARRY_CONFIG_DIR` (une
/// variable d'environnement est globale au processus).
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Dossier de configuration. `QUARRY_CONFIG_DIR` le remplace (tests,
/// profils séparés).
fn config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("QUARRY_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let dirs = directories::ProjectDirs::from("fr", "gsoulat", "Quarry")?;
    Some(dirs.config_dir().to_path_buf())
}

fn config_file() -> Option<PathBuf> {
    Some(config_dir()?.join("connections.json"))
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

/// Tous les mots de passe tiennent dans **un seul** élément du trousseau
/// (un JSON `{ "<id>": "mot de passe" }`). Sur macOS, chaque élément
/// déclenche sa propre demande d'autorisation : un seul élément, lu une
/// fois par session, évite de retaper le mot de passe du trousseau pour
/// chaque base.
const KEYRING_ACCOUNT: &str = "connections";

type Vault = std::collections::HashMap<u64, String>;

fn vault_cache() -> &'static std::sync::Mutex<Option<Vault>> {
    static CACHE: std::sync::Mutex<Option<Vault>> = std::sync::Mutex::new(None);
    &CACHE
}

fn vault_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|e| e.to_string())
}

/// Ancien schéma (un élément par connexion), conservé pour la migration.
fn legacy_entry(id: u64) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("connection-{id}")).map_err(|e| e.to_string())
}

/// Lit le coffre (une seule fois par session, puis depuis la mémoire).
fn vault() -> Result<Vault, String> {
    let mut cache = vault_cache().lock().map_err(|_| "Cache du trousseau verrouillé")?;
    if let Some(v) = cache.as_ref() {
        return Ok(v.clone());
    }
    let vault: Vault = match vault_entry()?.get_password() {
        Ok(json) => serde_json::from_str(&json).map_err(|e| format!("Trousseau illisible : {e}"))?,
        Err(keyring::Error::NoEntry) => Vault::new(),
        Err(e) => return Err(format!("Trousseau : {e}")),
    };
    *cache = Some(vault.clone());
    Ok(vault)
}

fn write_vault(vault: Vault) -> Result<(), String> {
    let json = serde_json::to_string(&vault).map_err(|e| e.to_string())?;
    vault_entry()?.set_password(&json).map_err(|e| format!("Trousseau : {e}"))?;
    if let Ok(mut cache) = vault_cache().lock() {
        *cache = Some(vault);
    }
    Ok(())
}

pub fn password(id: u64) -> Result<Option<String>, String> {
    let mut vault = vault()?;
    if let Some(p) = vault.get(&id) {
        return Ok(Some(p.clone()));
    }
    // Migration depuis l'ancien élément par connexion, s'il existe.
    match legacy_entry(id)?.get_password() {
        Ok(p) => {
            vault.insert(id, p.clone());
            write_vault(vault)?;
            let _ = legacy_entry(id)?.delete_credential();
            Ok(Some(p))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Trousseau : {e}")),
    }
}

pub fn set_password(id: u64, password: &str) -> Result<(), String> {
    let mut vault = vault()?;
    vault.insert(id, password.to_owned());
    write_vault(vault)
}

pub fn delete_password(id: u64) -> Result<(), String> {
    let mut vault = vault()?;
    if vault.remove(&id).is_some() {
        write_vault(vault)?;
    }
    let _ = legacy_entry(id).and_then(|e| e.delete_credential().map_err(|e| e.to_string()));
    Ok(())
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
    Some(config_dir()?.join(kind).join(format!("{id}.json")))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Chaque test écrit dans son propre dossier temporaire.
    fn with_temp_config<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let dir = std::env::temp_dir().join(format!("quarry-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY : les tests de ce module tournent en série (voir `serial`).
        unsafe { std::env::set_var("QUARRY_CONFIG_DIR", &dir) };
        let result = f();
        unsafe { std::env::remove_var("QUARRY_CONFIG_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    use super::TEST_ENV_LOCK as SERIAL;

    #[test]
    fn queries_round_trip_per_connection() {
        let _guard = SERIAL.lock().unwrap();
        with_temp_config("queries", || {
            assert!(load_queries(7).is_empty());
            let queries = vec![
                SavedQuery { name: "Clients".into(), sql: "SELECT * FROM clients".into() },
                SavedQuery { name: "Brouillon".into(), sql: String::new() },
            ];
            save_queries(7, &queries).unwrap();
            let loaded = load_queries(7);
            assert_eq!(loaded.len(), 2);
            assert_eq!(loaded[0].name, "Clients");
            assert_eq!(loaded[1].sql, "");
            // Une autre connexion ne voit pas ces onglets.
            assert!(load_queries(8).is_empty());
        });
    }

    #[test]
    fn history_is_newest_first_and_bounded() {
        let _guard = SERIAL.lock().unwrap();
        with_temp_config("history", || {
            for i in 0..(HISTORY_MAX + 5) {
                push_history(1, HistoryEntry { sql: format!("SELECT {i}"), at: "t".into(), rows: 1, ms: 1 }).unwrap();
            }
            let history = load_history(1);
            assert_eq!(history.len(), HISTORY_MAX);
            assert_eq!(history[0].sql, format!("SELECT {}", HISTORY_MAX + 4));
        });
    }

    #[test]
    fn connections_survive_a_reload() {
        let _guard = SERIAL.lock().unwrap();
        with_temp_config("connections", || {
            assert!(load().unwrap().is_empty());
            let config = ConnectionConfig {
                id: 1,
                name: "Local".into(),
                engine: Engine::Postgres,
                folder: "Perso".into(),
                host: "localhost".into(),
                port: Some(5432),
                user: "me".into(),
                database: "app".into(),
                path: String::new(),
                project_id: String::new(),
                credentials_path: String::new(),
                ssh_host: String::new(),
                ssh_port: None,
                ssh_user: String::new(),
                ssh_key: String::new(),
            };
            save(std::slice::from_ref(&config)).unwrap();
            let loaded = load().unwrap();
            assert_eq!(loaded, vec![config]);
        });
    }
}
