use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};

use crate::util::path::resolve_binary;

pub struct RpcClient {
    next_id: AtomicI64,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    stdin: ChildStdin,
    _child: Child,
}

impl RpcClient {
    pub async fn spawn(binary_override: Option<&str>) -> Result<Self> {
        let bin = resolve_binary("codex", binary_override)
            .ok_or_else(|| anyhow!("codex binary not on PATH"))?;

        let mut child = tokio::process::Command::new(&bin)
            .args(["-s", "read-only", "-a", "untrusted", "app-server"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_for_task = pending.clone();

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => {
                        tracing::trace!(line = %line, "codex non-json line");
                        continue;
                    }
                };
                if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
                    if let Some(tx) = pending_for_task.lock().unwrap().remove(&id) {
                        let _ = tx.send(v);
                    }
                } else {
                    tracing::trace!(notif = %v, "codex notification");
                }
            }
            tracing::debug!("codex stdout closed");
        });

        let mut client = Self {
            next_id: AtomicI64::new(1),
            pending,
            stdin,
            _child: child,
        };

        // Handshake
        let init = client
            .call(
                "initialize",
                json!({ "clientInfo": { "name": "ai-usage-bar", "version": env!("CARGO_PKG_VERSION") } }),
                Duration::from_secs(8),
            )
            .await?;
        tracing::debug!(init = %init, "codex initialized");
        client.notify("initialized", json!({})).await?;
        Ok(client)
    }

    pub async fn call(&mut self, method: &str, params: Value, t: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let req = json!({ "id": id, "method": method, "params": params });
        let mut bytes = serde_json::to_vec(&req)?;
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await?;
        self.stdin.flush().await?;

        let v = match timeout(t, rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => bail!("rpc channel dropped"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                bail!("rpc timeout for {method}");
            }
        };
        if let Some(err) = v.get("error") {
            bail!("{}", err);
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let req = json!({ "method": method, "params": params });
        let mut bytes = serde_json::to_vec(&req)?;
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}
