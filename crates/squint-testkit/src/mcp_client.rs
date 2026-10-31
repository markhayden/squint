//! An independent minimal MCP client for testing `squint serve` (table-manners 5): written
//! against the protocol document for revision `2025-11-25` and nothing else. Newline-delimited
//! JSON-RPC 2.0 over a child process's stdio: `initialize`, `notifications/initialized`,
//! `tools/list` through the cursor, `tools/call`, `resources/read`, with the server's
//! notifications kept as they arrive. It imports nothing from squint or from the SDK the server is
//! built on, so what it proves about the server is what any client sees.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// The protocol revision the client offers.
pub const PROTOCOL: &str = "2025-11-25";

/// A connected server.
pub struct Client {
    child: Child,
    reader: BufReader<ChildStdout>,
    writer: ChildStdin,
    next_id: u64,
    /// The server's `initialize` result.
    pub server: serde_json::Value,
    /// Every notification the server sent, in order.
    pub notifications: Vec<serde_json::Value>,
    /// The roots the client answers `roots/list` with, as `file://` URIs.
    pub roots: Vec<String>,
}

/// A tool as `tools/list` describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub input_schema: serde_json::Value,
}

/// A `tools/call` result.
#[derive(Debug, Clone, PartialEq)]
pub struct CallResult {
    pub structured: Option<serde_json::Value>,
    pub texts: Vec<String>,
    /// Image blocks as (media type, base64 data).
    pub images: Vec<(String, String)>,
    pub is_error: bool,
}

/// A JSON-RPC error from the server, or a transport problem.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Rpc {
        code: i64,
        message: String,
        data: Option<serde_json::Value>,
    },
    Transport(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Rpc {
                code,
                message,
                data,
            } => write!(f, "JSON-RPC error {code}: {message} {data:?}"),
            Failure::Transport(t) => write!(f, "transport: {t}"),
        }
    }
}

impl Client {
    /// Starts the server and completes the handshake at [`PROTOCOL`].
    pub fn start(command: &mut Command) -> Result<Client, Failure> {
        Client::start_at(command, PROTOCOL)
    }

    /// Starts the server and offers `protocol` in `initialize`; the server's answer is kept in
    /// `server` whatever revision it names, and `notifications/initialized` follows only when it
    /// names the one offered.
    pub fn start_at(command: &mut Command, protocol: &str) -> Result<Client, Failure> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Failure::Transport(format!("spawn: {e}")))?;
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| Failure::Transport("no stdin".into()))?;
        let reader = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| Failure::Transport("no stdout".into()))?,
        );
        let mut client = Client {
            child,
            reader,
            writer,
            next_id: 1,
            server: serde_json::Value::Null,
            notifications: Vec::new(),
            roots: Vec::new(),
        };
        let server = client.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": protocol,
                "capabilities": { "roots": {} },
                "clientInfo": { "name": "squint-testkit-independent-client", "version": "0" }
            }),
        )?;
        if server["protocolVersion"] == protocol {
            client.notify("notifications/initialized", serde_json::json!({}))?;
        }
        client.server = server;
        Ok(client)
    }

    /// Every tool, following `nextCursor` until the server gives none.
    pub fn list_tools(&mut self) -> Result<Vec<Tool>, Failure> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            let page = self.request("tools/list", params)?;
            for t in page["tools"].as_array().cloned().unwrap_or_default() {
                tools.push(Tool {
                    name: t["name"].as_str().unwrap_or_default().to_string(),
                    input_schema: t["inputSchema"].clone(),
                });
            }
            match page["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_string()),
                None => return Ok(tools),
            }
        }
    }

    /// Calls a tool.
    pub fn call(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallResult, Failure> {
        let r = self.request(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )?;
        Ok(call_result(&r))
    }

    /// Calls a tool with a progress token; notifications that arrive are kept in `notifications`.
    pub fn call_with_progress(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        token: &str,
    ) -> Result<CallResult, Failure> {
        let r = self.request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": arguments,
                "_meta": { "progressToken": token }
            }),
        )?;
        Ok(call_result(&r))
    }

    /// Reads a resource's text.
    pub fn read_resource(&mut self, uri: &str) -> Result<String, Failure> {
        let r = self.request("resources/read", serde_json::json!({ "uri": uri }))?;
        Ok(r["contents"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// Sends a request and reads lines until its response, keeping notifications; a request the
    /// server sends (`roots/list`) is answered with `roots`.
    pub fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(
            &serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )?;
        loop {
            let message = self.receive()?;
            if message.get("method").is_some() {
                if message.get("id").is_some() {
                    // A request from the server: `roots/list` is the only one a client offering
                    // roots must answer.
                    let roots: Vec<serde_json::Value> = self
                        .roots
                        .iter()
                        .map(|uri| serde_json::json!({ "uri": uri }))
                        .collect();
                    let reply = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": { "roots": roots }
                    });
                    self.send(&reply)?;
                } else {
                    self.notifications.push(message);
                }
                continue;
            }
            if message["id"] != id {
                return Err(Failure::Transport(format!(
                    "a response to {} while waiting for {id}",
                    message["id"]
                )));
            }
            if let Some(error) = message.get("error") {
                return Err(Failure::Rpc {
                    code: error["code"].as_i64().unwrap_or(0),
                    message: error["message"].as_str().unwrap_or_default().to_string(),
                    data: error.get("data").cloned(),
                });
            }
            return Ok(message["result"].clone());
        }
    }

    /// Sends a notification.
    pub fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), Failure> {
        self.send(&serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn send(&mut self, message: &serde_json::Value) -> Result<(), Failure> {
        let line = message.to_string();
        writeln!(self.writer, "{line}")
            .and_then(|()| self.writer.flush())
            .map_err(|e| Failure::Transport(format!("write: {e}")))
    }

    fn receive(&mut self) -> Result<serde_json::Value, Failure> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| Failure::Transport(format!("read: {e}")))?;
        if n == 0 {
            return Err(Failure::Transport("the server closed stdout".into()));
        }
        serde_json::from_str(&line)
            .map_err(|e| Failure::Transport(format!("not JSON-RPC on stdout: {e}: {line}")))
    }

    /// Closes stdin and waits for the server to exit.
    pub fn close(mut self) -> std::io::Result<std::process::ExitStatus> {
        drop(self.writer);
        self.child.wait()
    }
}

fn call_result(r: &serde_json::Value) -> CallResult {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for block in r["content"].as_array().cloned().unwrap_or_default() {
        match block["type"].as_str() {
            Some("text") => texts.push(block["text"].as_str().unwrap_or_default().to_string()),
            Some("image") => images.push((
                block["mimeType"].as_str().unwrap_or_default().to_string(),
                block["data"].as_str().unwrap_or_default().to_string(),
            )),
            _ => {}
        }
    }
    CallResult {
        structured: r.get("structuredContent").cloned(),
        texts,
        images,
        is_error: r["isError"].as_bool().unwrap_or(false),
    }
}
