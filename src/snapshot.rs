use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;

use crate::config::cache_dir;
use crate::cost::CostReport;
use crate::providers::{claude::ClaudeSnapshot, codex::CodexSnapshot, grok::GrokSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub refreshed_at: DateTime<Utc>,
    pub codex: Option<CodexSnapshot>,
    pub claude: Option<ClaudeSnapshot>,
    /// Present only when the Grok provider is active and a refresh was attempted.
    #[serde(default)]
    pub grok: Option<GrokSnapshot>,
    pub cost: Option<CostReport>,
}

impl Snapshot {
    pub fn new() -> Self {
        Self {
            refreshed_at: Utc::now(),
            codex: None,
            claude: None,
            grok: None,
            cost: None,
        }
    }

    pub fn is_stale(&self, max_age: std::time::Duration) -> bool {
        let age = Utc::now().signed_duration_since(self.refreshed_at);
        age.num_seconds().max(0) as u64 > max_age.as_secs()
    }
}

pub fn snapshot_path() -> Result<std::path::PathBuf> {
    Ok(cache_dir()?.join("snapshot.json"))
}

pub fn write(snap: &Snapshot) -> Result<()> {
    let path = snapshot_path()?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f =
            std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        let json = serde_json::to_vec(snap)?;
        f.write_all(&json)?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

pub fn read() -> Result<Snapshot> {
    let path = snapshot_path()?;
    let raw = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let snap = serde_json::from_slice(&raw)?;
    Ok(snap)
}
