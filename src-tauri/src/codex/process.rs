#[cfg(any(target_os = "macos", test))]
use std::path::Path;
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

use super::coalesce::RefreshCoalescer;
use super::protocol::{route_line, unsupported_request_reply, RoutedMessage};

/// How long a rate-limit notification waits before refreshing, so the rest of
/// a burst lands first and one refresh covers all of it. Short enough that
/// the meter still follows a turn as it finishes.
const NOTIFICATION_DEBOUNCE: Duration = Duration::from_millis(300);
/// Longest a `codex --version` probe (direct or through a login shell) may
/// take. A login profile that waits on something used to hold `start`, and
/// with it every later reconnect, forever.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

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
    /// Bumped whenever an App Server process is spawned or stopped. Each
    /// stdout reader remembers the generation it was started for, so a
    /// reader for a process that was already replaced cannot tear down the
    /// pipes of its successor when its own pipe finally closes.
    generation: Arc<AtomicU64>,
    /// Folds bursts of rate-limit notifications into one refresh at a time.
    notified_refresh: Arc<RefreshCoalescer>,
}

impl CodexManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            state: Arc::new(RwLock::new(crate::demo::initial_codex())),
            stdin: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            start_lock: Arc::new(Mutex::new(())),
            refresh_lock: Arc::new(Mutex::new(())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            notified_refresh: Arc::new(RefreshCoalescer::default()),
        }
    }

    pub async fn snapshot(&self) -> CodexState {
        self.state.read().await.clone()
    }

    pub async fn start(&self) -> Result<CodexState, String> {
        // Sample snapshot only. Demo mode must not spawn the Codex CLI.
        if crate::demo::enabled() {
            return Ok(self.snapshot().await);
        }
        let _guard = self.start_lock.lock().await;
        if self.snapshot().await.connection == ConnectionState::Connected {
            return Ok(self.snapshot().await);
        }
        // An App Server that is still running (say, signed out) is reused
        // without flashing "Connecting…". If it stopped answering, it is
        // replaced below: the next watchdog tick would otherwise reach the
        // same hung pipe and leave the meter stuck on the old state.
        if self.stdin.lock().await.is_some() {
            match self.refresh_data().await {
                Ok(state) => return Ok(state),
                Err(_) => self.stop_child().await,
            }
        }
        self.set_connection(ConnectionState::Starting, None).await;

        let cli = match find_codex_cli().await {
            Ok(cli) => cli,
            Err(message) => {
                self.set_connection(ConnectionState::CliNotFound, Some(message))
                    .await;
                return Ok(self.snapshot().await);
            }
        };

        let mut command = match cli {
            CodexExecutable::Direct(path) => {
                let mut command = Command::new(path);
                command.arg("app-server").arg("--stdio");
                command
            }
            #[cfg(target_os = "macos")]
            CodexExecutable::LoginShell(shell) => {
                let mut command = Command::new(shell);
                command.args(["-lc", LOGIN_SHELL_EXEC]);
                command
            }
        };
        #[cfg(target_os = "windows")]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // A spawn failure used to return straight out of here, leaving the
        // state on "Starting" with no diagnostic until the next attempt.
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = format!("Codex App Server could not be started: {error}");
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            // Dropping the child kills it (kill_on_drop).
            let message = "Codex App Server pipes were unavailable".to_owned();
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        };
        // The generation, the pipe and the child change together under the
        // stdin lock; `stop_child` and `handle_disconnect` take the same lock,
        // so neither can act on a half-installed process.
        let generation = {
            let mut slot = self.stdin.lock().await;
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            *slot = Some(stdin);
            *self.child.lock().await = Some(child);
            generation
        };

        self.spawn_stdout_reader(stdout, generation);
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
        // Like a failed initialize: without this, the half-started process
        // stayed up and the state stayed on "Starting".
        if let Err(error) = self.notify("initialized", None).await {
            self.fail_start(format!("Codex App Server initialization failed: {error}"))
                .await;
            return Err(error);
        }

        match self.refresh_data().await {
            Ok(state) => Ok(state),
            Err(error) => {
                self.fail_start(error.clone()).await;
                Err(error)
            }
        }
    }

    pub async fn refresh_or_start(&self) -> Result<CodexState, String> {
        if crate::demo::enabled() {
            return Ok(self.snapshot().await);
        }
        if self.snapshot().await.connection == ConnectionState::Connected {
            match self.refresh_data().await {
                Ok(state) => Ok(state),
                Err(error) => {
                    // A request that fails while "connected" usually means the
                    // app-server hung (commonly after system sleep). Retrying
                    // over the same pipe never recovers, so restart the child.
                    self.stop_child().await;
                    self.set_connection(ConnectionState::Disconnected, Some(error))
                        .await;
                    self.start().await
                }
            }
        } else {
            self.start().await
        }
    }

    pub async fn refresh_data(&self) -> Result<CodexState, String> {
        if crate::demo::enabled() {
            return Ok(self.snapshot().await);
        }
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

    fn spawn_stdout_reader(&self, stdout: tokio::process::ChildStdout, generation: u64) {
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
                            manager.refresh_after_notification();
                        }
                        Ok(RoutedMessage::Request { id, method }) => {
                            // Answered off the reader: a write that waits on a
                            // full stdin pipe must not stop this loop from
                            // draining the stdout the server is blocked on.
                            let reply_manager = manager.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = reply_manager
                                    .write_json(unsupported_request_reply(id, &method))
                                    .await;
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
                .handle_disconnect(generation, "Codex App Server exited unexpectedly")
                .await;
        });
    }

    /// Refreshes after the App Server reported new numbers. Codex sends these
    /// in bursts during heavy use; see `coalesce` for how they are folded.
    fn refresh_after_notification(&self) {
        if !self.notified_refresh.signal() {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager
                .notified_refresh
                .drain(NOTIFICATION_DEBOUNCE, || async {
                    let _ = manager.refresh_data().await;
                })
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

    async fn handle_disconnect(&self, generation: u64, message: &str) {
        // Everything happens under the stdin lock, which `start` holds while
        // installing a new process. Checking the generation first and taking
        // the lock afterwards left a gap in which a replacement could start,
        // only to have its pipe cleared, its initialize request failed and
        // its state overwritten with "Disconnected" by this stale reader.
        let mut stdin = self.stdin.lock().await;
        // A reader for a process that `stop_child` already replaced: its
        // requests were failed there, and the pipes now belong to a newer
        // process.
        if generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        *stdin = None;
        self.fail_pending(message).await;
        if self.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::Disconnected;
            state.diagnostic = Some(message.to_owned());
        }
        drop(stdin);
        self.emit_state().await;
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
            if connection == ConnectionState::CliNotFound {
                state.account = None;
                state.rate_limits = None;
                state.usage = None;
                state.updated_at = None;
            }
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
        // Retire the current generation first, so its reader ignores the
        // pipe closing below instead of racing the next `start`. The child is
        // taken under the same stdin lock as the generation bump, so a
        // `start` that slips in afterwards keeps its own process instead of
        // having it killed here.
        let child = {
            let mut stdin = self.stdin.lock().await;
            self.generation.fetch_add(1, Ordering::SeqCst);
            *stdin = None;
            self.fail_pending("Codex App Server was restarted").await;
            self.child.lock().await.take()
        };
        if let Some(mut child) = child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    async fn fail_pending(&self, message: &str) {
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for (_, sender) in pending {
            let _ = sender.send(Err(message.to_owned()));
        }
    }

    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
        self.stop_child().await;
    }
}

enum CodexExecutable {
    Direct(PathBuf),
    /// Run through this login shell, which puts `codex` on PATH.
    #[cfg(target_os = "macos")]
    LoginShell(PathBuf),
}

/// Login shells known to accept `-lc <script>` and to run the three scripts
/// below unchanged. All three were checked with `codex` reachable only through
/// the shell's own login profile (`.zprofile`, `.bash_profile`,
/// `config.fish`). Anything else (tcsh takes `-l` only on its own, nushell has
/// no `command -v`) keeps the zsh lookup UsageBar always made.
#[cfg(any(target_os = "macos", test))]
const LOGIN_SHELLS: [&str; 3] = ["zsh", "bash", "fish"];
/// macOS's default login shell, and the fallback for an unset or unusable
/// `$SHELL`.
#[cfg(any(target_os = "macos", test))]
const DEFAULT_LOGIN_SHELL: &str = "/bin/zsh";
#[cfg(any(target_os = "macos", test))]
const LOGIN_SHELL_LOOKUP: &str = "command -v codex";
#[cfg(any(target_os = "macos", test))]
const LOGIN_SHELL_VERSION: &str = "codex --version >/dev/null 2>&1";
#[cfg(any(target_os = "macos", test))]
const LOGIN_SHELL_EXEC: &str = "exec codex app-server --stdio";

/// The shell whose login profile is asked to find `codex`: the user's own
/// `$SHELL` when it is a supported shell given as an absolute path to an
/// executable file, otherwise `/bin/zsh`. A bash or fish user's PATH lives in
/// that shell's profile, which zsh never reads.
#[cfg(any(target_os = "macos", test))]
fn pick_login_shell(
    shell_env: Option<&std::ffi::OsStr>,
    is_executable: impl Fn(&Path) -> bool,
) -> PathBuf {
    shell_env
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| LOGIN_SHELLS.contains(&name))
        })
        .filter(|path| is_executable(path))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LOGIN_SHELL))
}

#[cfg(any(target_os = "macos", all(unix, test)))]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Runs a probe with no terminal attached and reports whether it succeeded
/// within `PROBE_TIMEOUT`. A probe that overruns is killed (kill_on_drop).
async fn probe_succeeds(mut command: Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    matches!(
        timeout(PROBE_TIMEOUT, child.wait()).await,
        Ok(Ok(status)) if status.success()
    )
}

async fn find_codex_cli() -> Result<CodexExecutable, String> {
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut candidates = vec![
        std::env::var_os("USAGEBAR_CODEX_PATH").map(PathBuf::from),
        which::which("codex").ok(),
        Some(PathBuf::from("/opt/homebrew/bin/codex")),
        Some(PathBuf::from("/usr/local/bin/codex")),
    ];
    #[cfg(target_os = "windows")]
    {
        if let Some(home) = crate::platform::home_dir() {
            candidates.push(Some(home.join(".local/bin/codex.exe")));
            candidates.push(Some(home.join(".cargo/bin/codex.exe")));
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            candidates.push(Some(PathBuf::from(appdata).join("npm/codex.cmd")));
        }
    }
    for candidate in candidates.into_iter().flatten() {
        if !candidate.exists() {
            continue;
        }
        let mut probe = Command::new(&candidate);
        #[cfg(target_os = "windows")]
        probe.creation_flags(0x08000000);
        probe.arg("--version");
        if probe_succeeds(probe).await {
            return Ok(CodexExecutable::Direct(candidate));
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Apps opened from Finder get launchd's minimal PATH, so a `codex`
        // installed through nvm, asdf or a custom prefix is only reachable
        // through the user's login profile.
        let shell = pick_login_shell(std::env::var_os("SHELL").as_deref(), is_executable_file);
        let mut lookup = Command::new(&shell);
        lookup.args(["-lc", LOGIN_SHELL_LOOKUP]);
        let mut version = Command::new(&shell);
        version.args(["-lc", LOGIN_SHELL_VERSION]);
        if probe_succeeds(lookup).await && probe_succeeds(version).await {
            return Ok(CodexExecutable::LoginShell(shell));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn pick(shell: Option<&str>, executable: bool) -> PathBuf {
        pick_login_shell(shell.map(OsStr::new), |_| executable)
    }

    #[test]
    fn the_users_own_login_shell_is_used_when_it_is_usable() {
        assert_eq!(pick(Some("/bin/bash"), true), PathBuf::from("/bin/bash"));
        assert_eq!(
            pick(Some("/opt/homebrew/bin/fish"), true),
            PathBuf::from("/opt/homebrew/bin/fish")
        );
        assert_eq!(pick(Some("/bin/zsh"), true), PathBuf::from("/bin/zsh"));
    }

    #[test]
    fn an_unusable_login_shell_falls_back_to_zsh() {
        let zsh = PathBuf::from(DEFAULT_LOGIN_SHELL);
        // Unset, empty, or relative: nothing safe to run.
        assert_eq!(pick(None, true), zsh);
        assert_eq!(pick(Some(""), true), zsh);
        assert_eq!(pick(Some("fish"), true), zsh);
        // Missing or not executable.
        assert_eq!(pick(Some("/usr/local/bin/fish"), false), zsh);
        // Shells that do not take `-lc` or `command -v` as written.
        assert_eq!(pick(Some("/bin/tcsh"), true), zsh);
        assert_eq!(pick(Some("/opt/homebrew/bin/nu"), true), zsh);
        // Only the file name counts, not a directory that happens to match.
        assert_eq!(pick(Some("/opt/bash/bin/elvish"), true), zsh);
    }

    #[cfg(unix)]
    #[test]
    fn only_executable_files_count_as_a_shell() {
        assert!(is_executable_file(Path::new("/bin/sh")));
        let dir = std::env::temp_dir().join(format!("usagebar-shell-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let plain = dir.join("fish");
        std::fs::write(&plain, "").expect("plain file");
        assert!(!is_executable_file(&plain));
        assert!(!is_executable_file(&dir));
        assert!(!is_executable_file(&dir.join("missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs the three login-shell scripts through every supported shell that
    /// is installed, with a stand-in `codex` reachable only through that
    /// shell's login profile, the way a Finder-launched app meets it.
    #[cfg(unix)]
    #[test]
    fn login_shell_scripts_work_in_every_supported_shell() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let root = std::env::temp_dir().join(format!("usagebar-login-{}", std::process::id()));
        let home = root.join("home");
        let bin = root.join("bin");
        std::fs::create_dir_all(home.join(".config/fish")).expect("home");
        std::fs::create_dir_all(&bin).expect("bin");
        let codex = bin.join("codex");
        std::fs::write(
            &codex,
            "#!/bin/sh\n\
             [ \"$1\" = --version ] && { echo codex-cli 0.0.0; exit 0; }\n\
             [ \"$1 $2\" = 'app-server --stdio' ] && { read -r line; echo \"got:$line\"; exit 0; }\n\
             exit 3\n",
        )
        .expect("stand-in codex");
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let bin = bin.display();
        let posix_profile = format!("export PATH=\"{bin}:$PATH\"\n");
        std::fs::write(home.join(".zprofile"), &posix_profile).expect("zprofile");
        std::fs::write(home.join(".bash_profile"), &posix_profile).expect("bash_profile");
        std::fs::write(
            home.join(".config/fish/config.fish"),
            format!("set -gx PATH {bin} $PATH\n"),
        )
        .expect("config.fish");

        let login = |shell: &Path, script: &str| {
            let mut command = Command::new(shell);
            command
                .args(["-lc", script])
                .env_clear()
                .env("HOME", &home)
                .env("PATH", "/usr/bin:/bin")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            command
        };

        let mut tried = 0;
        for name in LOGIN_SHELLS {
            let Ok(shell) = which::which(name) else {
                continue;
            };
            tried += 1;
            let status = |script| login(&shell, script).status().expect("shell runs");
            assert!(status(LOGIN_SHELL_LOOKUP).success(), "{name}: lookup");
            assert!(status(LOGIN_SHELL_VERSION).success(), "{name}: version");

            let mut server = login(&shell, LOGIN_SHELL_EXEC).spawn().expect("spawn");
            server
                .stdin
                .take()
                .expect("stdin")
                .write_all(b"{\"id\":1}\n")
                .expect("write");
            let output = server.wait_with_output().expect("app server");
            assert!(output.status.success(), "{name}: exec");
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("got:{\"id\":1}"),
                "{name}: stdin and stdout reach codex"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
        // Every Mac and Linux machine has bash, so a silent pass is impossible.
        assert!(tried > 0, "no supported shell was installed");
    }
}
