//! Read-only, synchronous stdio LSP sessions for explicitly opened source snapshots.
//! Server provisioning is a trusted host operation, not a tool parameter. Installations and
//! runtime paths must be operator-owned and immutable to project authors. No workspace
//! root, real file URI, environment, config discovery, shell, or command RPC is provided.
//! Servers requiring a disk-backed project or unsupported capabilities may decline queries.
//! The native policy is for trusted workers, not arbitrary binaries. Deadlines bound
//! protocol I/O, not kernel/filesystem latency or the worker's CPU and memory use.
//! Helpers must remain in the worker's process group until it shuts down.
mod preview;
mod wire;

use std::collections::BTreeMap;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::sandbox::{ConfinementPolicy, WorkerIo, confine_command};
use crate::workspace::Workspace;
pub use preview::{ReplacementPreview, preview_replacement};
use wire::Wire;

const MAX_DOCUMENT: usize = 64 * 1024;
const MAX_OPEN: usize = 16;
const MAX_MESSAGES: usize = 32;
const MAX_RESULTS: usize = 256;

fn invalid(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}

fn timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "local LSP deadline expired")
}

fn snapshot(workspace: &Workspace, path: &str) -> io::Result<(String, [u8; 32])> {
    let read = workspace.read_range(path, 0, MAX_DOCUMENT).map_err(io::Error::other)?;
    if read.file_len != read.bytes.len() as u64 {
        return Err(invalid("document exceeds byte limit or changed during read"));
    }
    let text = String::from_utf8(read.bytes).map_err(|_| invalid("document must be UTF-8"))?;
    Ok((text, read.sha256))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ServerKind {
    RustAnalyzer,
    Clangd,
    Kotlin,
    Pyright,
    Vtsls,
}

impl ServerKind {
    fn executable_name(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust-analyzer",
            Self::Clangd => "clangd",
            Self::Kotlin => "kotlin-language-server",
            Self::Pyright => "pyright-langserver",
            Self::Vtsls => "vtsls",
        }
    }

    fn language(self, path: &str) -> io::Result<&'static str> {
        match (self, Path::new(path).extension().and_then(|s| s.to_str())) {
            (Self::RustAnalyzer, Some("rs")) => Ok("rust"),
            (Self::Clangd, Some("c" | "h")) => Ok("c"),
            (Self::Clangd, Some("cc" | "cpp" | "cxx" | "hpp")) => Ok("cpp"),
            (Self::Kotlin, Some("kt" | "kts")) => Ok("kotlin"),
            (Self::Pyright, Some("py" | "pyi")) => Ok("python"),
            (Self::Vtsls, Some("ts")) => Ok("typescript"),
            (Self::Vtsls, Some("js")) => Ok("javascript"),
            (Self::Vtsls, Some("tsx")) => Ok("typescriptreact"),
            (Self::Vtsls, Some("jsx")) => Ok("javascriptreact"),
            _ => Err(invalid("document does not match server language")),
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Pyright | Self::Vtsls => &["--stdio"],
            Self::Clangd => &["--enable-config=0", "--background-index=0", "--clang-tidy=0"],
            _ => &[],
        }
    }
}

/// This object is configured once by the operator. Supply a fixed absolute server
/// executable (or vetted native launcher for script-based servers) and only the
/// trusted runtime files/directories it needs. Never put this API in tool dispatch.
pub struct OperatorServer {
    kind: ServerKind,
    executable: PathBuf,
    runtime: Vec<PathBuf>,
    deadline: Duration,
}

impl OperatorServer {
    pub fn new(kind: ServerKind, executable: &Path, runtime: &[PathBuf], deadline: Duration) -> io::Result<Self> {
        if !executable.is_absolute() || runtime.len() > 24 || deadline.is_zero() || deadline > Duration::from_secs(30) {
            return Err(invalid("invalid server configuration"));
        }
        let executable = executable.canonicalize()?;
        let name = executable.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let expected = kind.executable_name();
        if !(name == expected
            || name
                .strip_prefix(expected)
                .and_then(|s| s.strip_prefix('-'))
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
            || !executable.is_file()
        {
            return Err(invalid("expected the configured language server executable"));
        }
        // execvp may interpret unrecognized executables as shell scripts. Only
        // accept native ELF programs, never shebangs or executable project text.
        use std::io::Read;
        let mut magic = [0; 4];
        std::fs::File::open(&executable)?.read_exact(&mut magic)?;
        if &magic != b"\x7fELF" {
            return Err(invalid("server must be a native executable"));
        }
        let runtime = runtime
            .iter()
            .map(|p| {
                if !p.is_absolute() {
                    return Err(invalid("runtime path must be absolute"));
                }
                p.canonicalize()
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            kind,
            executable,
            runtime,
            deadline,
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    /// UTF-16 code units.
    pub character: u32,
}

impl Position {
    fn param(self) -> io::Result<Value> {
        if self.line > i32::MAX as u32 || self.character > i32::MAX as u32 {
            return Err(invalid("position out of range"));
        }
        Ok(json!({"line":self.line, "character":self.character}))
    }

    fn parse(value: &Value) -> io::Result<Self> {
        let number = |name| {
            value
                .get(name)
                .and_then(Value::as_u64)
                .filter(|n| *n <= i32::MAX as u64)
                .map(|n| n as u32)
                .ok_or_else(|| invalid("invalid position"))
        };
        Ok(Self {
            line: number("line")?,
            character: number("character")?,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    fn parse(value: &Value) -> io::Result<Self> {
        let start = Position::parse(&value["start"])?;
        let end = Position::parse(&value["end"])?;
        if (start.line, start.character) > (end.line, end.character) {
            return Err(invalid("reversed range"));
        }
        Ok(Self { start, end })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Location {
    /// A caller-opened relative path, never a server-controlled path or URI.
    pub relative_path: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    /// Untrusted display text; not an instruction, hyperlink, command or patch.
    pub message: String,
}

fn diagnostics(items: &Value) -> io::Result<Vec<Diagnostic>> {
    items
        .as_array()
        .filter(|a| a.len() <= MAX_RESULTS)
        .ok_or_else(|| invalid("invalid diagnostics"))?
        .iter()
        .map(|d| {
            Ok(Diagnostic {
                range: Range::parse(&d["range"])?,
                message: d["message"]
                    .as_str()
                    .filter(|s| s.len() <= 4096)
                    .ok_or_else(|| invalid("invalid message"))?
                    .into(),
            })
        })
        .collect()
}

struct Document {
    path: String,
    diagnostics: Option<Vec<Diagnostic>>,
}

struct Process(Option<Child>);

impl Process {
    fn reap(&mut self) {
        if let Some(mut child) = self.0.take() {
            // SAFETY: this is our child's private process group. Its PID is
            // still owned and unreaped; do not signal the group after wait.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.reap()
    }
}

/// One initialized server per operator-bound workspace and language. Multiple
/// instances are independent; each admits just one outstanding request at a time.
pub struct LocalLsp {
    child: Process,
    wire: Wire,
    workspace: Workspace,
    docs: BTreeMap<String, Document>,
    virtual_root: String,
    kind: ServerKind,
    deadline: Duration,
    request_id: u64,
    pull_diagnostics: bool,
}

/// One preconfigured session per language. Looking up an unprovisioned kind never
/// starts a process or tries to discover a server in the workspace or PATH.
pub struct LocalLspPool(BTreeMap<ServerKind, LocalLsp>);

impl LocalLspPool {
    pub fn start(configs: &[OperatorServer], root: &Path) -> io::Result<Self> {
        if configs.len() > 5
            || configs
                .iter()
                .enumerate()
                .any(|(i, c)| configs[..i].iter().any(|p| p.kind == c.kind))
        {
            return Err(invalid("duplicate or excessive server configuration"));
        }
        let mut sessions = BTreeMap::new();
        for config in configs {
            sessions.insert(config.kind, LocalLsp::start(config, root)?);
        }
        Ok(Self(sessions))
    }

    pub fn session(&mut self, kind: ServerKind) -> Option<&mut LocalLsp> {
        self.0.get_mut(&kind)
    }

    pub fn shutdown(self) -> io::Result<()> {
        for (_, server) in self.0 {
            server.shutdown()?;
        }
        Ok(())
    }
}

fn virtual_uri(root: &str, relative_path: &str) -> String {
    // Called only for a path validated by Workspace. Preserve the relative
    // layout for servers resolving imports, while escaping URI syntax and UTF-8.
    use std::fmt::Write;
    let mut uri = format!("file://{root}/");
    for b in relative_path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
            uri.push(char::from(b));
        } else {
            write!(&mut uri, "%{b:02X}").expect("String write");
        }
    }
    uri
}

impl LocalLsp {
    /// Returns only after a confined server replies to `initialize` and the
    /// client sends `initialized`. Failure never retries with a weaker policy.
    /// This handshake is not evidence that project indexing or any query works.
    pub fn start(config: &OperatorServer, workspace_root: &Path) -> io::Result<Self> {
        Self::spawn(config, workspace_root, config.kind.args())
    }

    // Not exposed: protocol operations must never be able to supply argv.
    fn spawn(config: &OperatorServer, root: &Path, args: &[&str]) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(invalid("workspace must be absolute"));
        }
        // Do not turn a symlinked root into a different valid Workspace handle.
        let workspace = Workspace::open(root).map_err(io::Error::other)?;
        let root = root.canonicalize()?;
        let virtual_root = format!(
            "/eon-lsp-{:016x}",
            getrandom::u64().map_err(|_| io::Error::other("no session nonce"))?
        );
        let mut read_only = config.runtime.clone();
        read_only.push(config.executable.clone());
        if read_only.iter().any(|p| {
            p.starts_with(&root)
                || root.starts_with(p)
                || p.starts_with(&virtual_root)
                || Path::new(&virtual_root).starts_with(p)
        }) {
            return Err(invalid(
                "server installation and runtime must be disjoint from workspace",
            ));
        }
        let mut command = Command::new(&config.executable);
        command.args(args).process_group(0);
        confine_command(
            &mut command,
            &ConfinementPolicy {
                read_only,
                deny_network: true,
            },
            WorkerIo::StdioProtocol,
        )
        .map_err(io::Error::other)?;
        let mut child = Process(Some(command.spawn().map_err(|error| {
            io::Error::new(error.kind(), format!("confined LSP launch failed: {error}"))
        })?));
        let c = child.0.as_mut().expect("just spawned");
        let wire = Wire::new(
            c.stdin.take().expect("piped stdin"),
            c.stdout.take().expect("piped stdout"),
        )?;
        let mut client = Self {
            child,
            wire,
            workspace,
            kind: config.kind,
            docs: BTreeMap::new(),
            virtual_root,
            deadline: config.deadline,
            request_id: 0,
            pull_diagnostics: false,
        };
        let end = client.end();
        let init = client.request("initialize", json!({
            "processId":null,"rootUri":null,"workspaceFolders":null,
            "capabilities":{"general":{"positionEncodings":["utf-16"]},
                "workspace":{"applyEdit":false,"configuration":false,"workspaceFolders":false},
                "textDocument":{"hover":{"contentFormat":["plaintext"]},"diagnostic":{},"publishDiagnostics":{"versionSupport":true}}}
        }), end).map_err(|error| {
            io::Error::new(error.kind(), format!("confined LSP did not initialize: {error}"))
        })?;
        let caps = init["capabilities"]
            .as_object()
            .ok_or_else(|| invalid("missing server capabilities"))?;
        if caps.get("positionEncoding").is_some_and(|s| s != "utf-16") {
            return Err(invalid("unsupported position encoding"));
        }
        client.pull_diagnostics = caps.get("diagnosticProvider").is_some_and(Value::is_object);
        client.notify("initialized", json!({}), end)?;
        Ok(client)
    }

    fn end(&self) -> Instant {
        Instant::now() + self.deadline
    }

    fn send(&mut self, packet: Value, end: Instant) -> io::Result<()> {
        if self.child.0.is_none() {
            return Err(io::ErrorKind::NotConnected.into());
        }
        self.wire.send(&packet, end).inspect_err(|_| self.child.reap())
    }

    fn notify(&mut self, method: &str, params: Value, end: Instant) -> io::Result<()> {
        let mut packet = json!({"jsonrpc":"2.0","method":method});
        if !params.is_null() {
            packet["params"] = params;
        }
        self.send(packet, end)
    }

    fn request(&mut self, method: &str, params: Value, end: Instant) -> io::Result<Value> {
        self.request_id = self
            .request_id
            .checked_add(1)
            .ok_or_else(|| invalid("request id exhausted"))?;
        let mut packet = json!({"jsonrpc":"2.0","id":self.request_id,"method":method});
        if !params.is_null() {
            packet["params"] = params;
        }
        self.send(packet, end)?;
        for _ in 0..MAX_MESSAGES {
            if let Some(reply) = self
                .receive(end, Some(self.request_id))
                .inspect_err(|_| self.child.reap())?
            {
                if reply.get("error").is_some() {
                    return Err(io::Error::other("LSP request rejected"));
                }
                return Ok(reply["result"].clone());
            }
        }
        self.child.reap();
        Err(invalid("too many unrelated LSP messages"))
    }

    fn receive(&mut self, end: Instant, expected: Option<u64>) -> io::Result<Option<Value>> {
        let msg = self.wire.recv(end)?;
        if msg["jsonrpc"] != "2.0" {
            return Err(invalid("invalid JSON-RPC version"));
        }
        if let Some(method) = msg.get("method") {
            let method = method.as_str().ok_or_else(|| invalid("invalid method"))?;
            if let Some(id) = msg.get("id") {
                if !id.is_i64() && !id.as_str().is_some_and(|s| s.len() <= 128) {
                    return Err(invalid("invalid server request id"));
                }
                // In particular, never honor workspace/applyEdit, executeCommand,
                // dynamic registration, showDocument or configuration requests.
                self.send(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported"}}),
                    end,
                )?;
            } else if method == "textDocument/publishDiagnostics" {
                let p = &msg["params"];
                if let Some(doc) = p["uri"].as_str().and_then(|u| self.docs.get_mut(u))
                    && p.get("version").is_none_or(|v| v.as_u64() == Some(1))
                {
                    doc.diagnostics = Some(diagnostics(&p["diagnostics"])?);
                }
            }
            return Ok(None);
        }
        if expected.is_none()
            || msg["id"].as_u64() != expected
            || msg.get("result").is_some() == msg.get("error").is_some()
        {
            return Err(invalid("unexpected JSON-RPC response"));
        }
        Ok(Some(msg))
    }

    /// Open an immutable workspace snapshot; use a fresh session after editing.
    /// Generated file URIs are under an inaccessible virtual root, not on disk.
    pub fn open(&mut self, relative_path: &str) -> io::Result<()> {
        if self.docs.len() == MAX_OPEN || self.docs.values().any(|d| d.path == relative_path) {
            return Err(invalid("document already open or session full"));
        }
        let language = self.kind.language(relative_path)?;
        let (text, _) = snapshot(&self.workspace, relative_path)?;
        let uri = virtual_uri(&self.virtual_root, relative_path);
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{
                "uri":uri,"languageId":language,"version":1,"text":text
            }}),
            self.end(),
        )?;
        self.docs.insert(
            uri,
            Document {
                path: relative_path.into(),
                diagnostics: None,
            },
        );
        Ok(())
    }

    fn uri(&self, path: &str) -> io::Result<String> {
        self.docs
            .iter()
            .find(|(_, d)| d.path == path)
            .map(|(u, _)| u.clone())
            .ok_or_else(|| invalid("document not open"))
    }

    fn at(&self, path: &str, pos: Position) -> io::Result<Value> {
        Ok(json!({"textDocument":{"uri":self.uri(path)?}, "position":pos.param()?}))
    }

    fn locations(&self, value: &Value) -> io::Result<Vec<Location>> {
        let values = match value {
            Value::Null => return Ok(vec![]),
            Value::Array(a) if a.len() <= MAX_RESULTS => a.as_slice(),
            Value::Object(_) => std::slice::from_ref(value),
            _ => return Err(invalid("invalid location result")),
        };
        values
            .iter()
            .map(|v| {
                let doc = v["uri"]
                    .as_str()
                    .and_then(|s| self.docs.get(s))
                    .ok_or_else(|| invalid("location outside open documents"))?;
                Ok(Location {
                    relative_path: doc.path.clone(),
                    range: Range::parse(&v["range"])?,
                })
            })
            .collect()
    }

    pub fn goto_definition(&mut self, path: &str, pos: Position) -> io::Result<Vec<Location>> {
        let value = self.request("textDocument/definition", self.at(path, pos)?, self.end())?;
        self.locations(&value)
    }

    pub fn find_references(&mut self, path: &str, pos: Position) -> io::Result<Vec<Location>> {
        let mut params = self.at(path, pos)?;
        params["context"] = json!({"includeDeclaration":true});
        let value = self.request("textDocument/references", params, self.end())?;
        self.locations(&value)
    }

    /// Display text only. Never render server output as active HTML or follow links.
    pub fn hover_type(&mut self, path: &str, pos: Position) -> io::Result<Option<String>> {
        let value = self.request("textDocument/hover", self.at(path, pos)?, self.end())?;
        if value.is_null() {
            return Ok(None);
        }
        let content = &value["contents"];
        content
            .as_str()
            .or_else(|| content["value"].as_str())
            .filter(|s| s.len() <= 8192)
            .map(|s| Some(s.into()))
            .ok_or_else(|| invalid("unsupported hover content"))
    }

    /// Prefer LSP 3.17 pull diagnostics when advertised. Otherwise consume push
    /// diagnostics for the opened immutable snapshot, with the same deadline.
    pub fn diagnostics(&mut self, path: &str) -> io::Result<Vec<Diagnostic>> {
        let uri = self.uri(path)?;
        let end = self.end();
        if self.pull_diagnostics {
            let value = self.request("textDocument/diagnostic", json!({"textDocument":{"uri":uri}}), end)?;
            if value["kind"] != "full" {
                return Err(invalid("expected full diagnostic report"));
            }
            return diagnostics(&value["items"]);
        }
        if self.child.0.is_none() {
            return Err(io::ErrorKind::NotConnected.into());
        }
        if let Some(cached) = &self.docs[&uri].diagnostics {
            return Ok(cached.clone());
        }
        for _ in 0..MAX_MESSAGES {
            self.receive(end, None).inspect_err(|_| self.child.reap())?;
            if let Some(items) = &self.docs[&uri].diagnostics {
                return Ok(items.clone());
            }
        }
        self.child.reap();
        Err(invalid("diagnostic message budget exhausted"))
    }

    /// Complete the LSP shutdown/exit handshake. If the server ignores either
    /// step the process group is terminated and the direct child reaped. Drop
    /// provides the same forced cleanup when no graceful shutdown is requested.
    pub fn shutdown(mut self) -> io::Result<()> {
        let end = self.end();
        if !self.request("shutdown", Value::Null, end)?.is_null() {
            return Err(invalid("invalid shutdown response"));
        }
        self.notify("exit", Value::Null, end)?;
        // Even a well-behaved server may have outstanding helpers in the group.
        // Observe exit without reaping, so Drop can signal the group before the
        // PID becomes reusable.
        let pid = self.child.0.as_ref().expect("live server").id() as libc::pid_t;
        loop {
            if Instant::now() >= end {
                return Err(timeout());
            }
            // SAFETY: waitid writes a valid siginfo_t; WNOWAIT keeps ownership of pid.
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            if unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as _,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            } < 0
            {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            if unsafe { info.si_pid() } == pid {
                return if info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0 {
                    Ok(())
                } else {
                    Err(io::Error::other("server exited unsuccessfully"))
                };
            }
            std::thread::sleep(Duration::from_millis(1).min(end.saturating_duration_since(Instant::now())));
        }
    }
}

#[cfg(test)]
mod lsp_and_patcher_tests;
