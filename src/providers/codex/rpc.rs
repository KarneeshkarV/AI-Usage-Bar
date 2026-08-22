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

/// Codex CLI 0.149.0 removed `-a untrusted`. Remaining values are `on-request`
/// and `never`. `never` is the non-interactive policy CodexBar uses with the
/// read-only sandbox.
pub const APP_SERVER_ARGS: &[&str] = &["-s", "read-only", "-a", "never", "app-server"];

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>;

pub struct RpcClient {
    next_id: AtomicI64,
    pending: Pending,
    stdin: ChildStdin,
    _child: Child,
}

impl RpcClient {
    pub async fn spawn(binary_override: Option<&str>) -> Result<Self> {
        let bin = resolve_binary("codex", binary_override)
            .ok_or_else(|| anyhow!("codex binary not on PATH"))?;

        let mut child = tokio::process::Command::new(&bin)
            .args(APP_SERVER_ARGS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
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

        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let stderr_for_task = stderr_buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(line = %line, "codex stderr");
                let mut buf = stderr_for_task.lock().unwrap();
                if buf.len() >= 2000 {
                    continue;
                }
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&line);
            }
        });

        let next_id = AtomicI64::new(1);
        let init_params = json!({
            "clientInfo": {
                "name": "ai-usage-bar",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        // If the CLI rejects flags (as 0.149.0 did for `-a untrusted`), the
        // process exits immediately. Surface that instead of waiting 8s for
        // initialize to time out on a closed pipe.
        let init = tokio::select! {
            res = rpc_call(
                &mut stdin,
                &pending,
                &next_id,
                "initialize",
                init_params,
                Duration::from_secs(8),
            ) => res.map_err(|e| {
                let stderr = stderr_preview(&stderr_buf);
                if stderr.is_empty() {
                    e
                } else {
                    anyhow!("{e}; stderr: {stderr}")
                }
            })?,
            status = child.wait() => {
                let status = status.map_err(|e| anyhow!("codex app-server wait failed: {e}"))?;
                let stderr = stderr_preview(&stderr_buf);
                bail!("codex app-server exited ({status}): {stderr}");
            }
        };
        tracing::debug!(init = %init, "codex initialized");

        rpc_notify(&mut stdin, "initialized", json!({})).await?;

        Ok(Self {
            next_id,
            pending,
            stdin,
            _child: child,
        })
    }

    pub async fn call(&mut self, method: &str, params: Value, t: Duration) -> Result<Value> {
        rpc_call(
            &mut self.stdin,
            &self.pending,
            &self.next_id,
            method,
            params,
            t,
        )
        .await
    }
}

async fn rpc_call(
    stdin: &mut ChildStdin,
    pending: &Pending,
    next_id: &AtomicI64,
    method: &str,
    params: Value,
    t: Duration,
) -> Result<Value> {
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending.lock().unwrap().insert(id, tx);

    let req = json!({ "id": id, "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&req)?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await?;
    stdin.flush().await?;

    let v = match timeout(t, rx).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => bail!("rpc channel dropped"),
        Err(_) => {
            pending.lock().unwrap().remove(&id);
            bail!("rpc timeout for {method}");
        }
    };
    if let Some(err) = v.get("error") {
        bail!("{}", err);
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

async fn rpc_notify(stdin: &mut ChildStdin, method: &str, params: Value) -> Result<()> {
    let req = json!({ "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&req)?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await?;
    stdin.flush().await?;
    Ok(())
}

fn stderr_preview(buf: &Arc<Mutex<String>>) -> String {
    buf.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_server_args_match_codex_0_149() {
        assert_eq!(
            APP_SERVER_ARGS,
            &["-s", "read-only", "-a", "never", "app-server"]
        );
        assert!(
            !APP_SERVER_ARGS.contains(&"untrusted"),
            "Codex CLI 0.149.0 removed -a untrusted"
        );
    }
}
