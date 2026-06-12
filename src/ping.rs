use std::path::PathBuf;
use std::process::Stdio;

use anyhow::Result;
use tokio::process::Command;

/// Empty scratch dir the headless pings run inside, so they never touch a real
/// project directory. Created on demand under the system temp dir.
pub fn scratch_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("ai-usage-bar-ping");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Fire a headless `claude -p ping` to anchor the next 5h session window.
pub async fn ping_claude(binary: Option<&str>, model: &str) -> Result<()> {
    let dir = scratch_dir()?;
    let status = Command::new(binary.unwrap_or("claude"))
        .current_dir(&dir)
        .args(["-p", "ping", "--model", model])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("claude exited with {status}");
    }
    Ok(())
}

/// Fire a headless `codex exec ping` to anchor the next 5h session window.
pub async fn ping_codex(binary: Option<&str>, model: &str, reasoning: &str) -> Result<()> {
    let dir = scratch_dir()?;
    let status = Command::new(binary.unwrap_or("codex"))
        .current_dir(&dir)
        .arg("exec")
        .args(["-m", model])
        .arg("-c")
        .arg(format!("model_reasoning_effort=\"{reasoning}\""))
        .arg("--skip-git-repo-check")
        .arg("ping")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("codex exited with {status}");
    }
    Ok(())
}
