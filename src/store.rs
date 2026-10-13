//! Connexions enregistrées : la configuration va dans un fichier JSON du
//! dossier de configuration, les mots de passe dans le trousseau du système
//! (Keychain sur macOS, Gestionnaire d'identification sur Windows,
//! Secret Service sur Linux). Aucun secret n'est écrit sur le disque en clair.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "quarry";

/// Type de formulaire de connexion d'un moteur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    /// Hôte, port, utilisateur, mot de passe, base.
    Server,
    /// Chemin d'un fichier.
    File,
    /// Projet et clé (BigQuery…).
    Cloud,
}

/// Un moteur de base de données, tel que décrit par le catalogue (moteurs
/// intégrés + registre des connecteurs). Chaque connexion enregistrée en
/// garde une copie : elle reste utilisable hors ligne et si le registre change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Engine {
    pub id: String,
    pub name: String,
    /// Connecteur téléchargeable qui sert ce moteur ; `None` = intégré (SQLite).
    pub connector: Option<String>,
    pub form: Form,
    pub default_port: Option<u16>,
    #[serde(default)]
    pub description: String,
}

impl Engine {
    /// Moteurs connus sans le registre (hors ligne, premier lancement).
    pub fn builtins() -> Vec<Engine> {
        let e = |id: &str, name: &str, connector: Option<&str>, form: Form, port: Option<u16>, desc: &str| Engine {
            id: id.into(),
            name: name.into(),
            connector: connector.map(String::from),
            form,
            default_port: port,
            description: desc.into(),
        };
        vec![
            e("sqlite", "SQLite", None, Form::File, None, "Intégré à Quarry, aucun téléchargement."),
            e("postgres", "PostgreSQL", Some("postgres"), Form::Server, Some(5432), "PostgreSQL 10 et plus."),
            e("mysql", "MySQL", Some("mysql"), Form::Server, Some(3306), "MySQL 8 ; le plan nécessite 8.0.18 ou plus."),
            e("bigquery", "Google BigQuery", Some("bigquery"), Form::Cloud, None, "SQL standard, coût estimé par dry run."),
        ]
    }

    pub fn builtin(id: &str) -> Option<Engine> {
        Self::builtins().into_iter().find(|e| e.id == id)
    }

    pub fn default_port(&self) -> Option<u16> {
        self.default_port
    }

    /// Moteur client/serveur (hôte, port, utilisateur, mot de passe).
    pub fn is_server(&self) -> bool {
        self.form == Form::Server
    }

    /// Identifiant du connecteur téléchargeable ; `None` pour un moteur intégré.
    pub fn plugin_id(&self) -> Option<&str> {
        self.connector.as_deref()
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// Accepte l'ancien format (`"Postgres"`, `"MySql"`, `"Sqlite"`, `"BigQuery"`)
/// comme le nouveau (objet complet).
impl<'de> Deserialize<'de> for Engine {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Legacy(String),
            Full {
                id: String,
                name: String,
                #[serde(default)]
                connector: Option<String>,
                form: Form,
                #[serde(default)]
                default_port: Option<u16>,
                #[serde(default)]
                description: String,
            },
        }
        match Raw::deserialize(deserializer)? {
            Raw::Legacy(name) => {
                let id = match name.as_str() {
                    "Postgres" => "postgres",
                    "MySql" => "mysql",
                    "Sqlite" => "sqlite",
                    "BigQuery" => "bigquery",
                    other => other,
                };
                Engine::builtin(id).ok_or_else(|| serde::de::Error::custom(format!("moteur inconnu : {name}")))
            }
            Raw::Full { id, name, connector, form, default_port, description } => {
                Ok(Engine { id, name, connector, form, default_port, description })
            }
        }
    }
}

/// Catalogue des moteurs : intégrés, complétés par le registre (mis en cache
/// dans `engines.json` pour fonctionner hors ligne).
pub fn load_catalog() -> Vec<Engine> {
    let cached: Vec<Engine> = read_json(config_dir().map(|d| d.join("engines.json")));
    merge_catalog(Engine::builtins(), cached)
}

pub fn save_catalog(engines: &[Engine]) -> Result<(), String> {
    write_json(config_dir().map(|d| d.join("engines.json")), &engines)
}

/// Les entrées de `extra` remplacent celles de `base` de même id ; les
/// nouvelles s'ajoutent à la fin.
pub fn merge_catalog(base: Vec<Engine>, extra: Vec<Engine>) -> Vec<Engine> {
    let mut out = base;
    for e in extra {
        match out.iter_mut().find(|b| b.id == e.id) {
            Some(slot) => *slot = e,
            None => out.push(e),
        }
    }
    out
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
        match self.engine.form {
            Form::File => Location::Local,
            Form::Cloud => Location::Cloud,
            Form::Server => Location::Remote,
        }
    }

    /// Résumé affiché sous le nom (« localhost:5432/app », « mon-projet »…).
    pub fn summary(&self) -> String {
        match self.engine.form {
            Form::File => self.path.clone(),
            Form::Cloud => self.project_id.clone(),
            Form::Server => {
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
    fn legacy_engine_names_still_load() {
        let json = r#"[{"id":1,"name":"old","engine":"Postgres","host":"h","port":5432,"user":"u","database":"d",
            "path":"","project_id":"","credentials_path":""}]"#;
        let loaded: Vec<ConnectionConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(loaded[0].engine.id, "postgres");
        assert!(loaded[0].engine.is_server());
    }

    #[test]
    fn catalog_merge_replaces_by_id() {
        let mut extra = Engine::builtin("postgres").unwrap();
        extra.name = "PostgreSQL 17".into();
        let cockroach = Engine { id: "cockroachdb".into(), name: "CockroachDB".into(), ..extra.clone() };
        let merged = merge_catalog(Engine::builtins(), vec![extra, cockroach]);
        assert_eq!(merged.len(), 5);
        assert_eq!(merged.iter().find(|e| e.id == "postgres").unwrap().name, "PostgreSQL 17");
    }

    #[test]
    fn connections_survive_a_reload() {
        let _guard = SERIAL.lock().unwrap();
        with_temp_config("connections", || {
            assert!(load().unwrap().is_empty());
            let config = ConnectionConfig {
                id: 1,
                name: "Local".into(),
                engine: Engine::builtin("postgres").unwrap(),
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
