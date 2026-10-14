//! `klip api` (hidden): a JSON-lines bridge to the control socket and to the file helpers the terminal
//! app has, for the tray app (or any script that prefers JSON to the binary protocol).
//!
//! One JSON object per line on stdin, one per line on stdout; the `id` of a request is echoed.
//! `{"id":1,"ipc":"GetStatus"}` or `{"id":1,"ipc":{"SendClip":{"text":"hi"}}}` forward an
//! `IpcRequest` (serde's JSON form of the Rust type); `{"id":2,"method":"config_get","params":{}}`
//! calls a helper. Answers are `{"id":1,"ok":...}` or `{"id":1,"err":"...","down":true?}`.
//! Requests run concurrently (joining a pairing can wait for a person), and a bad line never ends
//! the bridge. Replies are made friendlier for a UI: a `fingerprint` next to every `node_id`, an
//! `id_hex` next to every clip id, and clip contents removed (`content_len` instead): contents are
//! fetched on purpose with `history_text`.
use crate::api::IpcDaemonApi;
use crate::client::{ClientError, DaemonClient, OLD_DAEMON_REQUEST_CAP};
use klip_daemon::{DaemonConfig, KlipPaths};
use klip_proto::hlc::NodeId;
use klip_proto::ipc::{IpcRequest, IpcResponse, IPC_PROTOCOL_VERSION};
use klip_tui::api::DaemonApi;
use klip_tui::files::ServiceState;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

pub struct BridgeError {
    message: String,
    /// The daemon is not running (the tray shows "stopped" instead of an error).
    down: bool,
}

impl From<String> for BridgeError {
    fn from(message: String) -> Self {
        Self {
            message,
            down: false,
        }
    }
}

impl From<&str> for BridgeError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

/// Read requests until stdin ends, answer each on its own task.
pub async fn run(paths: KlipPaths) {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(line) = rx.recv().await {
            let written = async {
                out.write_all(line.as_bytes()).await?;
                out.write_all(b"\n").await?;
                out.flush().await
            }
            .await;
            if written.is_err() {
                break; // the reader is gone
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = tokio::task::JoinSet::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let tx = tx.clone();
        let paths = paths.clone();
        tasks.spawn(async move {
            let _ = tx.send(answer(&paths, &line).await);
        });
    }
    // stdin ended: let answers that are about to finish go out, but never wait long for a slow one.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    drop(tasks);
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
}

async fn answer(paths: &KlipPaths, line: &str) -> String {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"id": null, "err": format!("not valid JSON: {e}")}).to_string(),
    };
    let mut reply = Map::new();
    reply.insert(
        "id".to_string(),
        request.get("id").cloned().unwrap_or(Value::Null),
    );
    match dispatch(paths, &request).await {
        Ok(v) => {
            reply.insert("ok".to_string(), v);
        }
        Err(e) => {
            reply.insert("err".to_string(), Value::String(e.message));
            if e.down {
                reply.insert("down".to_string(), Value::Bool(true));
            }
        }
    }
    Value::Object(reply).to_string()
}

async fn dispatch(paths: &KlipPaths, request: &Value) -> Result<Value, BridgeError> {
    if let Some(ipc) = request.get("ipc") {
        let request: IpcRequest = serde_json::from_value(ipc.clone())
            .map_err(|e| BridgeError::from(format!("bad request: {e}")))?;
        return Ok(response_json(&call(&request).await?));
    }
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::from("a request needs \"ipc\" or \"method\""))?;
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "version" => Ok(json!({
            "klip": env!("CARGO_PKG_VERSION"),
            "ipc_protocol": IPC_PROTOCOL_VERSION,
        })),
        "config_get" => config_get(paths),
        "config_set" => config_set(paths, &params),
        "log_tail" => log_tail(paths, &params),
        "qr" => qr(&params),
        "history_text" => history_text(&params).await,
        "daemon_start" => daemon_start(paths).await,
        "daemon_restart" => daemon_restart(paths).await,
        "service_state" => Ok(service_state()),
        "service_set" => service_set(&params),
        _ => Err(format!("unknown method {method}").into()),
    }
}

async fn call(request: &IpcRequest) -> Result<IpcResponse, BridgeError> {
    match DaemonClient::for_current_user()
        .request(request, OLD_DAEMON_REQUEST_CAP)
        .await
    {
        Ok(response) => Ok(response),
        Err(ClientError::Down(message)) => Err(BridgeError {
            message,
            down: true,
        }),
        Err(e) => Err(e.to_string().into()),
    }
}

fn response_json(response: &IpcResponse) -> Value {
    let mut v = serde_json::to_value(response).unwrap_or(Value::Null);
    // A thumbnail is a PNG: base64 is far lighter than a JSON array of numbers.
    if let IpcResponse::Thumbnail(png) = response {
        v = json!({"Thumbnail": data_encoding::BASE64.encode(png)});
    }
    decorate(&mut v);
    v
}

fn numbers<const N: usize>(v: &Value) -> Option<[u8; N]> {
    let items = v.as_array()?;
    if items.len() != N {
        return None;
    }
    let mut out = [0u8; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = u8::try_from(item.as_u64()?).ok()?;
    }
    Some(out)
}

fn decorate(v: &mut Value) {
    match v {
        Value::Array(items) => items.iter_mut().for_each(decorate),
        Value::Object(o) => {
            if let Some(bytes) = o.get("node_id").and_then(numbers::<32>) {
                o.insert(
                    "fingerprint".to_string(),
                    NodeId::new(bytes).fingerprint().into(),
                );
            }
            if let Some(bytes) = o.get("origin_node").and_then(numbers::<32>) {
                o.insert(
                    "origin_fingerprint".to_string(),
                    NodeId::new(bytes).fingerprint().into(),
                );
            }
            if let Some(bytes) = o.get("id").and_then(numbers::<16>) {
                o.insert("id_hex".to_string(), hex(&bytes).into());
            }
            if let Some(Value::Array(content)) = o.remove("full_content") {
                o.insert("content_len".to_string(), content.len().into());
            }
            o.values_mut().for_each(decorate);
        }
        _ => {}
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex16(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn config_get(paths: &KlipPaths) -> Result<Value, BridgeError> {
    let config = DaemonConfig::load(paths)?;
    serde_json::to_value(config).map_err(|e| e.to_string().into())
}

fn config_set(paths: &KlipPaths, params: &Value) -> Result<Value, BridgeError> {
    let config: DaemonConfig =
        serde_json::from_value(params.get("config").cloned().unwrap_or_default())
            .map_err(|e| BridgeError::from(format!("not a valid configuration: {e}")))?;
    config.save(paths)?;
    Ok(json!({}))
}

fn log_tail(paths: &KlipPaths, params: &Value) -> Result<Value, BridgeError> {
    let offset = params.get("offset").and_then(Value::as_u64);
    let chunk = klip_tui::files::read_log_chunk(&klip_tui::files::log_path(paths), offset)?;
    Ok(json!({
        "new_offset": chunk.new_offset,
        "text": chunk.text,
        "reset": chunk.reset,
        "missing": chunk.missing,
    }))
}

/// The dark/light modules of a QR code for `text` (low error correction, as the terminal card uses),
/// row by row as '1' (dark) and '0'.
fn qr(params: &Value) -> Result<Value, BridgeError> {
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .ok_or("qr needs a \"text\"")?;
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
        .map_err(|e| BridgeError::from(format!("cannot make a QR code: {e}")))?;
    let modules: String = code
        .to_colors()
        .iter()
        .map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' })
        .collect();
    Ok(json!({"size": code.width(), "modules": modules}))
}

/// The text of one history item, on purpose (the list itself never carries contents).
async fn history_text(params: &Value) -> Result<Value, BridgeError> {
    let id = params
        .get("id_hex")
        .and_then(Value::as_str)
        .and_then(unhex16)
        .ok_or("history_text needs an \"id_hex\" of 32 hex digits")?;
    let IpcResponse::History(items) = call(&IpcRequest::GetHistory { limit: 10_000 }).await? else {
        return Err("unexpected reply from the daemon".into());
    };
    let item = items
        .into_iter()
        .find(|i| i.id == id)
        .ok_or("that clip is no longer in the history")?;
    if item.full_content.is_empty() {
        return Err("that clip was too large to keep in the history".into());
    }
    let text = String::from_utf8(item.full_content).map_err(|_| "that clip is not text")?;
    Ok(json!({"text": text}))
}

// ------------------------------------------------------------------ starting and the login unit

const UNIT: &str = "klip.service";

/// `systemctl --user <args>`; `true` when it exits 0. `KLIP_SYSTEMCTL` replaces the program (tests).
fn systemctl(args: &[&str]) -> Result<bool, String> {
    let program = std::env::var("KLIP_SYSTEMCTL").unwrap_or_else(|_| "systemctl".to_string());
    std::process::Command::new(program)
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .map_err(|e| format!("cannot run systemctl: {e}"))
}

fn unit_installed() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    matches!(
        klip_tui::files::service_state(home.as_deref()),
        ServiceState::Installed(_)
    )
}

/// The systemd unit looks after the daemon of the default directory. A daemon started with its own
/// `KLIP_CONFIG_DIR` (tests, development) is never the unit's business.
fn managed_by_unit() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("KLIP_CONFIG_DIR").is_none() && unit_installed()
}

fn api(paths: &KlipPaths) -> IpcDaemonApi {
    IpcDaemonApi::new(DaemonClient::for_current_user(), paths.clone())
}

async fn wait_until_up(api: &IpcDaemonApi) -> Result<(), BridgeError> {
    for _ in 0..120 {
        if api.status().await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("Klip did not come up within 30 s".into())
}

async fn systemctl_async(args: &'static [&'static str]) -> Result<bool, BridgeError> {
    tokio::task::spawn_blocking(move || systemctl(args))
        .await
        .map_err(|e| BridgeError::from(e.to_string()))?
        .map_err(BridgeError::from)
}

async fn daemon_start(paths: &KlipPaths) -> Result<Value, BridgeError> {
    let api = api(paths);
    if api.status().await.is_ok() {
        return Err("Klip is already running".into());
    }
    if managed_by_unit() {
        if !systemctl_async(&["start", UNIT]).await? {
            return Err("systemd could not start klip.service".into());
        }
        wait_until_up(&api).await?;
        return Ok(json!({"message": "started by systemd"}));
    }
    let pid = api
        .start_daemon()
        .await
        .map_err(|e| BridgeError::from(e.plain()))?;
    Ok(json!({"message": format!("started (pid {pid})")}))
}

async fn daemon_restart(paths: &KlipPaths) -> Result<Value, BridgeError> {
    let api = api(paths);
    if api.status().await.is_ok() {
        // Ask politely first, also when systemd supervises it: `systemctl restart` alone would not
        // replace a daemon that was started by hand.
        let _ = api.stop_daemon().await;
        let mut stopped = false;
        for _ in 0..40 {
            if api.status().await.is_err() {
                stopped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if !stopped {
            return Err("Klip did not stop within 10 s".into());
        }
    }
    if managed_by_unit() {
        if !systemctl_async(&["restart", UNIT]).await? {
            return Err("systemd could not restart klip.service".into());
        }
        wait_until_up(&api).await?;
        return Ok(json!({"message": "restarted by systemd"}));
    }
    let pid = api
        .start_daemon()
        .await
        .map_err(|e| BridgeError::from(e.plain()))?;
    Ok(json!({"message": format!("restarted (pid {pid})")}))
}

fn service_state() -> Value {
    let installed = unit_installed();
    json!({
        "supported": cfg!(target_os = "linux"),
        "installed": installed,
        "enabled": installed && systemctl(&["is-enabled", "--quiet", UNIT]).unwrap_or(false),
        "active": installed && systemctl(&["is-active", "--quiet", UNIT]).unwrap_or(false),
    })
}

/// Start Klip at login: write the unit (if needed) and enable it, or disable it.
fn service_set(params: &Value) -> Result<Value, BridgeError> {
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or("service_set needs \"enabled\": true or false")?;
    if !cfg!(target_os = "linux") {
        return Err("starting at login is set up for Linux (systemd) only".into());
    }
    if enabled {
        crate::ops::install_service()?;
        if !systemctl(&["daemon-reload"])? || !systemctl(&["enable", UNIT])? {
            return Err("systemd could not enable klip.service".into());
        }
    } else if !systemctl(&["disable", UNIT])? {
        return Err("systemd could not disable klip.service".into());
    }
    Ok(service_state())
}
