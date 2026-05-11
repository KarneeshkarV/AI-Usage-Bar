use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::config::cache_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    /// USD per 1 input token (non-cached).
    pub input: f64,
    /// USD per 1 cached input token.
    pub cached_input: f64,
    /// USD per 1 output token.
    pub output: f64,
}

#[derive(Debug, Clone, Default)]
pub struct PricingTable {
    pub models: HashMap<String, ModelPrice>,
    pub source: String,
}

impl PricingTable {
    pub fn get(&self, model: &str) -> Option<&ModelPrice> {
        if let Some(p) = self.models.get(model) {
            return Some(p);
        }
        // Loose match on prefix (e.g. "gpt-4o-mini-2024-07-18" -> "gpt-4o-mini").
        let lower = model.to_lowercase();
        self.models
            .iter()
            .filter(|(k, _)| lower.starts_with(&k.to_lowercase()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, v)| v)
    }
}

const FALLBACK: &[(&str, ModelPrice)] = &[
    // Per-token prices in USD (1e-6 per 1M tokens).
    // GPT-5 / Codex
    ("gpt-5", ModelPrice { input: 1.25e-6, cached_input: 0.125e-6, output: 10.0e-6 }),
    ("gpt-5-codex", ModelPrice { input: 1.25e-6, cached_input: 0.125e-6, output: 10.0e-6 }),
    ("gpt-5-mini", ModelPrice { input: 0.25e-6, cached_input: 0.025e-6, output: 2.0e-6 }),
    ("gpt-4o", ModelPrice { input: 2.5e-6, cached_input: 1.25e-6, output: 10.0e-6 }),
    ("gpt-4o-mini", ModelPrice { input: 0.15e-6, cached_input: 0.075e-6, output: 0.6e-6 }),
    ("o1", ModelPrice { input: 15.0e-6, cached_input: 7.5e-6, output: 60.0e-6 }),
    ("o3-mini", ModelPrice { input: 1.1e-6, cached_input: 0.55e-6, output: 4.4e-6 }),
    // Claude (Anthropic) — note: Claude JSONL carries pre-computed cost, so these
    // only act as a fallback if costNanos is missing.
    ("claude-opus-4", ModelPrice { input: 15.0e-6, cached_input: 1.5e-6, output: 75.0e-6 }),
    ("claude-sonnet-4", ModelPrice { input: 3.0e-6, cached_input: 0.3e-6, output: 15.0e-6 }),
    ("claude-haiku-4", ModelPrice { input: 0.8e-6, cached_input: 0.08e-6, output: 4.0e-6 }),
    ("claude-3-5-sonnet", ModelPrice { input: 3.0e-6, cached_input: 0.3e-6, output: 15.0e-6 }),
    ("claude-3-5-haiku", ModelPrice { input: 0.8e-6, cached_input: 0.08e-6, output: 4.0e-6 }),
];

pub async fn load_pricing() -> Result<PricingTable> {
    if let Ok(t) = load_cached() {
        return Ok(t);
    }
    if let Ok(t) = fetch_models_dev().await {
        let _ = write_cache(&t);
        return Ok(t);
    }
    Ok(fallback())
}

fn fallback() -> PricingTable {
    let mut models = HashMap::new();
    for (k, v) in FALLBACK {
        models.insert((*k).to_string(), v.clone());
    }
    PricingTable {
        models,
        source: "hardcoded fallback".into(),
    }
}

fn cache_file() -> Option<std::path::PathBuf> {
    cache_dir().ok().map(|d| d.join("pricing.json"))
}

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at_secs: u64,
    source: String,
    models: HashMap<String, ModelPrice>,
}

fn load_cached() -> Result<PricingTable> {
    let path = cache_file().ok_or_else(|| anyhow::anyhow!("no cache dir"))?;
    let raw = std::fs::read(&path)?;
    let c: Cached = serde_json::from_slice(&raw)?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(c.fetched_at_secs) > 24 * 3600 {
        anyhow::bail!("pricing cache stale");
    }
    Ok(PricingTable {
        models: c.models,
        source: format!("{} (cached)", c.source),
    })
}

fn write_cache(t: &PricingTable) -> Result<()> {
    let path = cache_file().ok_or_else(|| anyhow::anyhow!("no cache dir"))?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let c = Cached {
        fetched_at_secs: now,
        source: t.source.clone(),
        models: t.models.clone(),
    };
    let bytes = serde_json::to_vec(&c)?;
    std::fs::write(&path, bytes)?;
    Ok(())
}

async fn fetch_models_dev() -> Result<PricingTable> {
    // models.dev exposes a public JSON catalog of providers/models with prices.
    let url = "https://models.dev/api.json";
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let v: serde_json::Value = client.get(url).send().await?.error_for_status()?.json().await?;
    let mut models = HashMap::new();
    // The schema: top-level object keyed by provider id; each has `models` map.
    if let Some(obj) = v.as_object() {
        for (_provider, provider_v) in obj {
            if let Some(models_obj) = provider_v.get("models").and_then(|x| x.as_object()) {
                for (id, mv) in models_obj {
                    if let Some(cost) = mv.get("cost") {
                        // models.dev quotes prices "per 1M tokens", USD.
                        let input = cost.get("input").and_then(|x| x.as_f64()).unwrap_or(0.0);
                        let cached_input =
                            cost.get("cache_read").and_then(|x| x.as_f64()).unwrap_or(input / 10.0);
                        let output = cost.get("output").and_then(|x| x.as_f64()).unwrap_or(0.0);
                        if input == 0.0 && output == 0.0 {
                            continue;
                        }
                        models.insert(
                            id.clone(),
                            ModelPrice {
                                input: input / 1_000_000.0,
                                cached_input: cached_input / 1_000_000.0,
                                output: output / 1_000_000.0,
                            },
                        );
                    }
                }
            }
        }
    }
    if models.is_empty() {
        anyhow::bail!("models.dev returned no usable entries");
    }
    Ok(PricingTable { models, source: "models.dev".into() })
}
