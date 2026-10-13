//! Connecteurs téléchargeables : registre publié sur GitHub, installation
//! dans le dossier de données de Quarry, vérification SHA-256.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Où Quarry lit la liste des connecteurs disponibles.
pub const REGISTRY_URL: &str = "https://github.com/gsoulat/quarry-connectors/releases/latest/download/registry.json";

/// Cible de compilation de cette machine, telle que nommée dans le registre.
pub const TARGET: &str = env!("QUARRY_TARGET");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub engine: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    pub assets: std::collections::HashMap<String, Asset>,
}

impl Entry {
    /// L'exécutable pour cette machine, s'il existe.
    pub fn asset(&self) -> Option<&Asset> {
        self.assets.get(TARGET)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub protocol: u32,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub connectors: Vec<Entry>,
    /// Catalogue des moteurs (alias compris) servis par ces connecteurs.
    #[serde(default)]
    pub engines: Vec<crate::store::Engine>,
}

/// Un connecteur présent sur le disque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub path: PathBuf,
}

fn connectors_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("QUARRY_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("connectors"));
    }
    let dirs = directories::ProjectDirs::from("fr", "gsoulat", "Quarry")?;
    Some(dirs.data_dir().join("connectors"))
}

fn binary_name(id: &str) -> String {
    if cfg!(windows) { format!("quarry-connector-{id}.exe") } else { format!("quarry-connector-{id}") }
}

/// Connecteurs installés : `connectors/<id>/<version>/<binaire>`.
pub fn installed() -> Vec<Installed> {
    let Some(root) = connectors_dir() else { return Vec::new() };
    let Ok(ids) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut out = Vec::new();
    for id_dir in ids.flatten() {
        let id = id_dir.file_name().to_string_lossy().to_string();
        let Ok(versions) = std::fs::read_dir(id_dir.path()) else { continue };
        let mut found: Vec<Installed> = versions
            .flatten()
            .filter_map(|v| {
                let path = v.path().join(binary_name(&id));
                path.is_file().then(|| Installed { id: id.clone(), version: v.file_name().to_string_lossy().to_string(), path })
            })
            .collect();
        // Plusieurs versions : on garde la plus récente (tri sémantique simple).
        found.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
        if let Some(latest) = found.into_iter().next() {
            out.push(latest);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Chemin de l'exécutable d'un connecteur installé.
pub fn installed_binary(id: &str) -> Option<PathBuf> {
    installed().into_iter().find(|i| i.id == id).map(|i| i.path)
}

pub async fn fetch_registry() -> Result<Registry, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("quarry/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(REGISTRY_URL).send().await.map_err(|e| format!("Registre injoignable : {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Registre : HTTP {}", response.status()));
    }
    response.json::<Registry>().await.map_err(|e| format!("Registre illisible : {e}"))
}

/// Télécharge, vérifie l'empreinte et installe. Renvoie le chemin du binaire.
pub async fn install(entry: Entry) -> Result<Installed, String> {
    let asset = entry.asset().ok_or_else(|| format!("Pas de version de {} pour {TARGET}", entry.engine))?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("quarry/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let bytes = client
        .get(&asset.url)
        .send()
        .await
        .map_err(|e| format!("Téléchargement : {e}"))?
        .error_for_status()
        .map_err(|e| format!("Téléchargement : {e}"))?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let digest = hex::encode(Sha256::digest(&bytes));
    if !digest.eq_ignore_ascii_case(&asset.sha256) {
        return Err(format!("Empreinte SHA-256 différente de celle du registre : téléchargement refusé ({})", entry.engine));
    }

    let dir = connectors_dir().ok_or("Dossier de données introuvable")?.join(&entry.id).join(&entry.version);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let path = dir.join(binary_name(&entry.id));
    tokio::fs::write(&path, &bytes).await.map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).await.map_err(|e| e.to_string())?;
    }
    Ok(Installed { id: entry.id, version: entry.version, path })
}

/// Supprime toutes les versions installées d'un connecteur.
pub fn remove(id: &str) -> Result<(), String> {
    let dir = connectors_dir().ok_or("Dossier de données introuvable")?.join(id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

// ----------------------------------------------------------------------
// Mise à jour de Quarry lui-même
// ----------------------------------------------------------------------

/// Dernière Release de Quarry sur GitHub.
pub const APP_RELEASES_API: &str = "https://api.github.com/repos/gsoulat/quarry/releases/latest";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppUpdate {
    pub version: String,
    pub url: String,
}

/// `true` si `latest` est plus récente que `current` (comparaison numérique
/// par segment : 0.10.0 > 0.9.0).
pub fn is_newer(current: &str, latest: &str) -> bool {
    version_key(latest.trim_start_matches('v')) > version_key(current.trim_start_matches('v'))
}

/// Interroge GitHub ; `Ok(None)` si Quarry est à jour. Le dépôt doit être
/// public pour que l'API réponde sans authentification.
pub async fn check_app_update() -> Result<Option<AppUpdate>, String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        html_url: String,
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("quarry/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let release: Release = client
        .get(APP_RELEASES_API)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(is_newer(APP_VERSION, &release.tag_name)
        .then(|| AppUpdate { version: release.tag_name.trim_start_matches('v').to_string(), url: release.html_url }))
}

/// Taille lisible.
pub fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1.0 { format!("{mb:.1} Mo") } else { format!("{:.0} Ko", bytes as f64 / 1024.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_json_is_parsed() {
        let json = r#"{"protocol":1,"tag":"v0.1.0","connectors":[{"id":"postgres","engine":"PostgreSQL","version":"0.1.0",
            "assets":{"aarch64-apple-darwin":{"url":"https://x/y","sha256":"ab","size":12}}}]}"#;
        let registry: Registry = serde_json::from_str(json).unwrap();
        assert_eq!(registry.connectors[0].id, "postgres");
        assert_eq!(registry.connectors[0].assets["aarch64-apple-darwin"].size, 12);
    }

    #[test]
    fn version_comparison_is_numeric() {
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    /// Télécharge vraiment le connecteur PostgreSQL depuis GitHub (réseau).
    #[tokio::test]
    #[ignore = "réseau : cargo test -- --ignored"]
    async fn registry_install_round_trip() {
        let _guard = crate::store::TEST_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("quarry-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("QUARRY_CONFIG_DIR", &dir) };
        let registry = fetch_registry().await.expect("registre");
        let entry = registry.connectors.iter().find(|c| c.id == "postgres").expect("postgres").clone();
        let installed = install(entry).await.expect("installation");
        assert!(installed.path.is_file());
        assert_eq!(installed_binary("postgres"), Some(installed.path.clone()));
        // Le binaire répond bien à `hello`.
        let out = std::process::Command::new(&installed.path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(b"{\"id\":1,\"method\":\"hello\"}\n{\"id\":2,\"method\":\"close\"}\n")?;
                child.wait_with_output()
            })
            .expect("lancement");
        assert!(String::from_utf8_lossy(&out.stdout).contains("\"engine\":\"PostgreSQL\""));
        unsafe { std::env::remove_var("QUARRY_CONFIG_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newest_installed_version_wins() {
        let _guard = crate::store::TEST_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("quarry-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY : test isolé, variable propre à ce processus.
        unsafe { std::env::set_var("QUARRY_CONFIG_DIR", &dir) };
        for v in ["0.1.0", "0.2.0", "0.10.0"] {
            let d = dir.join("connectors").join("postgres").join(v);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(binary_name("postgres")), b"x").unwrap();
        }
        let found = installed();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version, "0.10.0");
        remove("postgres").unwrap();
        assert!(installed().is_empty());
        unsafe { std::env::remove_var("QUARRY_CONFIG_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
