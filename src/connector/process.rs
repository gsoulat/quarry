//! Connecteur externe : un exécutable téléchargé, lancé comme processus
//! enfant, avec lequel on dialogue en JSON ligne par ligne (voir la crate
//! `quarry-protocol`).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use quarry_protocol::{ConnectParams, Envelope, Hello, PROTOCOL_VERSION, Payload, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::{ColumnInfo, Connector, Dialect, QueryPlan, QueryResult, Relation, Result, TableRef};

/// Délai maximal d'une requête au connecteur.
const TIMEOUT: Duration = Duration::from_secs(120);

pub struct ProcessConnector {
    hello: Hello,
    io: Mutex<Io>,
}

struct Io {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl std::fmt::Debug for ProcessConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessConnector").field("engine", &self.hello.engine).finish()
    }
}

impl Io {
    async fn call(&mut self, request: Request) -> Result<Payload> {
        self.next_id += 1;
        let id = self.next_id;
        let mut line = serde_json::to_string(&Envelope { id, request }).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.map_err(|e| format!("Connecteur injoignable : {e}"))?;
        self.stdin.flush().await.map_err(|e| e.to_string())?;

        let reply = tokio::time::timeout(TIMEOUT, self.stdout.next_line())
            .await
            .map_err(|_| "Le connecteur ne répond pas (délai dépassé)".to_string())?
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Le connecteur s'est arrêté".to_string())?;
        let response: Response = serde_json::from_str(&reply).map_err(|e| format!("Réponse illisible : {e}"))?;
        if response.id != id {
            return Err(format!("Réponse désynchronisée ({} attendu, {} reçu)", id, response.id));
        }
        match (response.result, response.error) {
            (Some(payload), _) => Ok(payload),
            (None, Some(error)) => Err(error),
            (None, None) => Err("Réponse vide".into()),
        }
    }
}

impl ProcessConnector {
    /// Lance le connecteur, vérifie le protocole et ouvre la connexion.
    pub async fn launch(binary: &Path, params: ConnectParams) -> Result<Self> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Impossible de lancer {} : {e}", binary.display()))?;
        let stdin = child.stdin.take().ok_or("stdin du connecteur indisponible")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("stdout du connecteur indisponible")?).lines();
        let mut io = Io { child, stdin, stdout, next_id: 0 };

        let hello = match io.call(Request::Hello).await? {
            Payload::Hello(h) => h,
            other => return Err(format!("Réponse inattendue à hello : {other:?}")),
        };
        if hello.protocol != PROTOCOL_VERSION {
            return Err(format!(
                "Connecteur {} : protocole {} alors que Quarry parle le {} ; mets à jour le connecteur.",
                hello.engine, hello.protocol, PROTOCOL_VERSION
            ));
        }
        match io.call(Request::Connect(params)).await? {
            Payload::Ok => {}
            other => return Err(format!("Réponse inattendue à connect : {other:?}")),
        }
        Ok(Self { hello, io: Mutex::new(io) })
    }

    async fn call(&self, request: Request) -> Result<Payload> {
        self.io.lock().await.call(request).await
    }
}

impl Drop for ProcessConnector {
    fn drop(&mut self) {
        // `kill_on_drop` termine le processus ; on tente un arrêt propre avant.
        if let Ok(mut io) = self.io.try_lock() {
            let _ = io.child.start_kill();
        }
    }
}

macro_rules! expect {
    ($payload:expr, $variant:ident) => {
        match $payload {
            Payload::$variant(v) => Ok(v),
            other => Err(format!("Réponse inattendue : {other:?}")),
        }
    };
}

#[async_trait]
impl Connector for ProcessConnector {
    fn engine(&self) -> &str {
        &self.hello.engine
    }

    fn dialect(&self) -> &Dialect {
        &self.hello.dialect
    }

    async fn list_schemas(&self) -> Result<Vec<String>> {
        expect!(self.call(Request::ListSchemas).await?, Schemas)
    }

    async fn list_tables(&self) -> Result<Vec<TableRef>> {
        expect!(self.call(Request::ListTables).await?, Tables)
    }

    async fn describe_table(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        expect!(self.call(Request::DescribeTable(table.clone())).await?, Columns)
    }

    async fn list_relations(&self) -> Result<Vec<Relation>> {
        expect!(self.call(Request::ListRelations).await?, Relations)
    }

    async fn explain(&self, sql: &str) -> Result<QueryPlan> {
        expect!(self.call(Request::Explain { sql: sql.into() }).await?, Plan)
    }

    async fn query(&self, sql: &str) -> Result<QueryResult> {
        expect!(self.call(Request::Query { sql: sql.into() }).await?, Result)
    }

    async fn execute(&self, sql: &str) -> Result<u64> {
        expect!(self.call(Request::Execute { sql: sql.into() }).await?, Affected)
    }
}
