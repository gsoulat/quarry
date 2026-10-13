//! Tunnel SSH vers une base distante, via le client `ssh` du système
//! (OpenSSH, présent sur macOS, Linux et Windows 10+). L'authentification
//! passe par les clés ou l'agent SSH : pas de mot de passe interactif.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use crate::store::ConnectionConfig;

/// Délai maximal pour que le port local réponde.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Tunnel {
    child: Child,
    pub local_port: u16,
}

impl fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tunnel").field("local_port", &self.local_port).finish()
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // Ferme le tunnel avec la session ; l'erreur (processus déjà fini) est sans importance.
        let _ = self.child.start_kill();
    }
}

/// Port libre sur l'interface locale.
fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

/// Arguments de la commande `ssh`, séparés pour être testables.
pub fn ssh_args(config: &ConnectionConfig, local_port: u16) -> Vec<String> {
    let db_port = config.port.or(config.engine.default_port()).unwrap_or_default();
    let mut args = vec![
        "-N".to_string(),
        "-o".into(), "BatchMode=yes".into(),
        "-o".into(), "ExitOnForwardFailure=yes".into(),
        "-o".into(), "StrictHostKeyChecking=accept-new".into(),
        "-o".into(), "ServerAliveInterval=30".into(),
        "-p".into(), config.ssh_port.unwrap_or(22).to_string(),
        "-L".into(), format!("127.0.0.1:{local_port}:{}:{db_port}", config.host),
    ];
    if !config.ssh_key.trim().is_empty() {
        args.push("-i".into());
        args.push(config.ssh_key.trim().into());
    }
    let user = config.ssh_user.trim();
    args.push(if user.is_empty() { config.ssh_host.trim().into() } else { format!("{user}@{}", config.ssh_host.trim()) });
    args
}

impl Tunnel {
    pub async fn open(config: &ConnectionConfig) -> Result<Self, String> {
        let local_port = free_port()?;
        let args = ssh_args(config, local_port);

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Impossible de lancer ssh : {e} (OpenSSH est-il installé ?)"))?;

        // On attend que le port local accepte une connexion, ou que ssh échoue.
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr).await;
                }
                let detail = stderr.trim();
                return Err(format!(
                    "Tunnel SSH refusé ({status}){}",
                    if detail.is_empty() { String::new() } else { format!(" : {detail}") }
                ));
            }
            if tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, local_port)).await.is_ok() {
                return Ok(Self { child, local_port });
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.start_kill();
                return Err("Tunnel SSH : délai dépassé (hôte injoignable ou clé refusée)".into());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Engine;

    #[test]
    fn ssh_arguments_forward_the_database_port() {
        let config = ConnectionConfig {
            id: 1,
            name: "prod".into(),
            engine: Engine::builtin("postgres").unwrap(),
            folder: String::new(),
            host: "db.internal".into(),
            port: None,
            user: "app".into(),
            database: "app".into(),
            path: String::new(),
            project_id: String::new(),
            credentials_path: String::new(),
            ssh_host: "bastion.example.com".into(),
            ssh_port: Some(2222),
            ssh_user: "ubuntu".into(),
            ssh_key: "~/.ssh/id_ed25519".into(),
        };
        let args = ssh_args(&config, 50000);
        assert!(args.contains(&"-N".to_string()));
        assert!(args.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(args.windows(2).any(|w| w == ["-L", "127.0.0.1:50000:db.internal:5432"]));
        assert!(args.windows(2).any(|w| w == ["-i", "~/.ssh/id_ed25519"]));
        assert_eq!(args.last().unwrap(), "ubuntu@bastion.example.com");
    }

    #[test]
    fn free_port_is_usable() {
        assert!(free_port().unwrap() > 0);
    }
}
