use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{oneshot, Mutex, RwLock},
    time::{timeout, Duration},
};

use super::protocol::{route_line, RoutedMessage};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Starting,
    Connected,
    Disconnected,
    CliNotFound,
    NotAuthenticated,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexState {
    pub connection: ConnectionState,
    pub diagnostic: Option<String>,
    pub account: Option<Value>,
    pub rate_limits: Option<Value>,
    pub usage: Option<Value>,
    pub updated_at: Option<u64>,
}

impl Default for CodexState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Starting,
            diagnostic: None,
            account: None,
            rate_limits: None,
            usage: None,
            updated_at: None,
        }
    }
}

type PendingResponse = oneshot::Sender<Result<Value, String>>;

#[derive(Clone)]
pub struct CodexManager {
    app: AppHandle,
    state: Arc<RwLock<CodexState>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    child: Arc<Mutex<Option<Child>>>,
    pending: Arc<Mutex<HashMap<u64, PendingResponse>>>,
    next_id: Arc<AtomicU64>,
    start_lock: Arc<Mutex<()>>,
    refresh_lock: Arc<Mutex<()>>,
    shutting_down: Arc<AtomicBool>,
}

impl CodexManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            state: Arc::new(RwLock::new(CodexState::default())),
            stdin: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            start_lock: Arc::new(Mutex::new(())),
            refresh_lock: Arc::new(Mutex::new(())),
            shutting_down: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn snapshot(&self) -> CodexState {
        self.state.read().await.clone()
    }

    pub async fn start(&self) -> Result<CodexState, String> {
        let _guard = self.start_lock.lock().await;
        if self.snapshot().await.connection == ConnectionState::Connected {
            return Ok(self.snapshot().await);
        }
        self.set_connection(ConnectionState::Starting, None).await;

        if self.stdin.lock().await.is_some() {
            return self.refresh_data().await;
        }

        let cli = match find_codex_cli().await {
            Ok(cli) => cli,
            Err(message) => {
                self.set_connection(ConnectionState::CliNotFound, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        let mut command = match cli {
            CodexExecutable::Direct(path) => {
                let mut command = Command::new(path);
                command.arg("app-server").arg("--stdio");
                command
            }
            CodexExecutable::LoginShell => {
                let mut command = Command::new("/bin/zsh");
                command.args(["-lc", "exec codex app-server --stdio"]);
                command
            }
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or("Codex App Server stdin was unavailable")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Codex App Server stdout was unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("Codex App Server stderr was unavailable")?;
        *self.stdin.lock().await = Some(stdin);
        *self.child.lock().await = Some(child);

        self.spawn_stdout_reader(stdout);
        self.spawn_stderr_reader(stderr);

        let initialize = json!({
            "clientInfo": {
                "name": "usagebar",
                "title": "UsageBar",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "experimentalApi": true,
                "requestAttestation": false
            }
        });
        if let Err(error) = self.request("initialize", Some(initialize)).await {
            self.fail_start(format!("Codex App Server initialization failed: {error}"))
                .await;
            return Err(error);
        }
        self.notify("initialized", None).await?;

        match self.refresh_data().await {
            Ok(state) => Ok(state),
            Err(error) => {
                self.fail_start(error.clone()).await;
                Err(error)
            }
        }
    }

    pub async fn refresh_or_start(&self) -> Result<CodexState, String> {
        if self.snapshot().await.connection == ConnectionState::Connected {
            match self.refresh_data().await {
                Ok(state) => Ok(state),
                Err(error) => {
                    // A request that fails while "connected" usually means the
                    // app-server hung (commonly after system sleep). Retrying
                    // over the same pipe never recovers, so restart the child.
                    self.stop_child().await;
                    self.set_connection(ConnectionState::Disconnected, Some(error)).await;
                    self.start().await
                }
            }
        } else {
            self.start().await
        }
    }

    pub async fn refresh_data(&self) -> Result<CodexState, String> {
        let _guard = self.refresh_lock.lock().await;
        let account = self
            .request("account/read", Some(json!({ "refreshToken": false })))
            .await?;
        if account.get("account").is_none_or(Value::is_null) {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::NotAuthenticated;
            state.diagnostic = Some("Codex is not signed in".into());
            drop(state);
            self.emit_state().await;
            return Ok(self.snapshot().await);
        }

        let (rate_limits, usage) = tokio::join!(
            self.request("account/rateLimits/read", None),
            self.request("account/usage/read", None)
        );
        let rate_limits = rate_limits?;
        let usage = usage.ok();
        {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::Connected;
            state.diagnostic = None;
            state.account = sanitize_account(&account);
            state.rate_limits = Some(rate_limits.clone());
            if usage.is_some() {
                state.usage = usage;
            }
            state.updated_at = Some(now_unix_seconds());
        }
        if let Some(alerts) = self.app.try_state::<crate::alerts::UsageAlerts>() {
            alerts.observe(&self.app, "Codex", &rate_limits).await;
        }
        self.emit_state().await;
        Ok(self.snapshot().await)
    }

    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        let mut message = Map::new();
        message.insert("id".into(), Value::from(id));
        message.insert("method".into(), Value::from(method));
        if let Some(params) = params {
            message.insert("params".into(), params);
        }
        if let Err(error) = self.write_json(Value::Object(message)).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match timeout(Duration::from_secs(15), receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("{method} response channel closed")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut message = Map::new();
        message.insert("method".into(), Value::from(method));
        if let Some(params) = params {
            message.insert("params".into(), params);
        }
        self.write_json(Value::Object(message)).await
    }

    async fn write_json(&self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        let pipe = stdin.as_mut().ok_or("Codex App Server is not running")?;
        pipe.write_all(&bytes)
            .await
            .map_err(|error| error.to_string())?;
        pipe.flush().await.map_err(|error| error.to_string())
    }

    fn spawn_stdout_reader(&self, stdout: tokio::process::ChildStdout) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => match route_line(&line) {
                        Ok(RoutedMessage::Response { id, result }) => {
                            if let Some(sender) = manager.pending.lock().await.remove(&id) {
                                let _ = sender.send(result);
                            }
                        }
                        Ok(RoutedMessage::Notification { method, .. })
                            if method == "account/rateLimits/updated"
                                || method == "account/updated" =>
                        {
                            let refresh_manager = manager.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = refresh_manager.refresh_data().await;
                            });
                        }
                        Ok(RoutedMessage::Notification { .. } | RoutedMessage::Unexpected) => {}
                        Err(_error) => {
                            #[cfg(debug_assertions)]
                            eprintln!("Codex App Server sent malformed JSON: {_error}");
                        }
                    },
                    Ok(None) => break,
                    Err(_error) => {
                        #[cfg(debug_assertions)]
                        eprintln!("Codex App Server stdout failed: {_error}");
                        break;
                    }
                }
            }
            manager
                .handle_disconnect("Codex App Server exited unexpectedly")
                .await;
        });
    }

    fn spawn_stderr_reader(&self, stderr: tokio::process::ChildStderr) {
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(_line)) = lines.next_line().await {
                #[cfg(debug_assertions)]
                eprintln!("codex app-server: {_line}");
            }
        });
    }

    async fn handle_disconnect(&self, message: &str) {
        *self.stdin.lock().await = None;
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for (_, sender) in pending {
            let _ = sender.send(Err(message.to_owned()));
        }
        if !self.shutting_down.load(Ordering::Relaxed) {
            self.set_connection(ConnectionState::Disconnected, Some(message.to_owned()))
                .await;
        }
    }

    async fn fail_start(&self, message: String) {
        self.set_connection(ConnectionState::Error, Some(message))
            .await;
        self.stop_child().await;
    }

    async fn set_connection(&self, connection: ConnectionState, diagnostic: Option<String>) {
        {
            let mut state = self.state.write().await;
            state.connection = connection;
            state.diagnostic = diagnostic;
        }
        self.emit_state().await;
    }

    async fn emit_state(&self) {
        let state = self.snapshot().await;
        let _ = self.app.emit("codex://state", state);
        self.update_tray().await;
    }

    pub async fn update_tray(&self) {
        // Both providers share one menu-bar item now, so the tray is repainted
        // from a single coordinator that reads both managers' state.
        crate::tray::refresh_unified_tray(&self.app).await;
    }

    async fn stop_child(&self) {
        *self.stdin.lock().await = None;
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
        self.stop_child().await;
    }
}

enum CodexExecutable {
    Direct(PathBuf),
    LoginShell,
}

async fn find_codex_cli() -> Result<CodexExecutable, String> {
    let candidates = [
        which::which("codex").ok(),
        Some(PathBuf::from("/opt/homebrew/bin/codex")),
        Some(PathBuf::from("/usr/local/bin/codex")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if !candidate.exists() {
            continue;
        }
        if Command::new(&candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success())
        {
            return Ok(CodexExecutable::Direct(candidate));
        }
    }
    let shell_lookup = Command::new("/bin/zsh")
        .args(["-lc", "command -v codex"])
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if shell_lookup.status.success()
        && Command::new("/bin/zsh")
            .args(["-lc", "codex --version >/dev/null 2>&1"])
            .status()
            .await
            .is_ok_and(|status| status.success())
    {
        return Ok(CodexExecutable::LoginShell);
    }
    Err("The official `codex` command could not be executed. Install or repair the Codex CLI, then reconnect.".into())
}

fn sanitize_account(result: &Value) -> Option<Value> {
    let account = result.get("account")?;
    Some(json!({
        "type": account.get("type").cloned().unwrap_or(Value::Null),
        "planType": account.get("planType").cloned().unwrap_or(Value::Null)
    }))
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
