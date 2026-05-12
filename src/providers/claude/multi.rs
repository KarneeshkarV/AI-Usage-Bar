//! Fan out across all known Claude credentials files and produce one
//! `AccountUsage` per slot. Per-account failures are captured as
//! `AccountUsage.error` rather than aborting the whole batch.
//!
//! Requests are issued sequentially with a small inter-request delay because
//! the OAuth usage endpoint rate-limits aggressive bursts even across
//! distinct tokens from the same IP.

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

use super::credentials::{AccountSlot, TokenInfo, read_token};
use super::usage_api;
use super::{AccountUsage, ExtraSpend, Window};

const INTER_REQUEST_DELAY: Duration = Duration::from_millis(400);
/// Skip the HTTP call once we are within this many ms of expiry — the server
/// rejects expired tokens with a 429 that looks like a generic rate-limit.
const EXPIRY_GRACE_MS: i64 = 30_000;

pub async fn fetch_all(slots: &[AccountSlot]) -> Vec<AccountUsage> {
    let mut out = Vec::with_capacity(slots.len());
    for (i, slot) in slots.iter().enumerate() {
        if i > 0 {
            sleep(INTER_REQUEST_DELAY).await;
        }
        out.push(fetch_one(slot).await);
    }
    out
}

async fn fetch_one(slot: &AccountSlot) -> AccountUsage {
    let info = match read_token(&slot.path) {
        Ok(i) => i,
        Err(e) => return empty_with_error(slot, None, format!("{e}")),
    };
    let TokenInfo {
        access_token,
        tier,
        expires_at_ms,
    } = info;

    if let Some(msg) = expiry_message(expires_at_ms) {
        return empty_with_error(slot, tier, msg);
    }

    match usage_api::fetch(&access_token).await {
        Ok(data) => AccountUsage {
            label: format_label(&slot.label, tier.as_deref()),
            tier,
            session: data.session,
            weekly: data.weekly,
            sonnet_weekly: data.sonnet_weekly,
            extra: data.extra,
            error: None,
            is_active: slot.is_active,
        },
        Err(e) => empty_with_error(slot, tier, format!("{e}")),
    }
}

/// Returns `Some("expired …")` when the token is past expiry, otherwise
/// `None`. A missing `expires_at_ms` is treated as "unknown, try anyway".
fn expiry_message(expires_at_ms: Option<i64>) -> Option<String> {
    let expires = expires_at_ms?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let delta_ms = expires - now;
    if delta_ms > EXPIRY_GRACE_MS {
        return None;
    }
    Some(format!("token expired {} ago", format_ms_ago(-delta_ms)))
}

fn format_ms_ago(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        "just now".into()
    }
}

fn empty_with_error(slot: &AccountSlot, tier: Option<String>, msg: String) -> AccountUsage {
    AccountUsage {
        label: format_label(&slot.label, tier.as_deref()),
        tier,
        session: None::<Window>,
        weekly: None::<Window>,
        sonnet_weekly: None::<Window>,
        extra: None::<ExtraSpend>,
        error: Some(msg),
        is_active: slot.is_active,
    }
}

fn format_label(name: &str, tier: Option<&str>) -> String {
    match tier {
        Some(t) if !t.is_empty() => format!("{name} ({t})"),
        _ => name.to_string(),
    }
}
