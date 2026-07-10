//! Cursor web API client (session cookie), ported from CodexBar's CursorStatusProbe.
//!
//! Endpoints (base `https://cursor.com`):
//! - `GET /api/usage-summary` — plan usage percent + billing cycle
//! - `GET /api/auth/me` — account email / sub
//! - `GET /api/usage?user=<id>` — legacy request-count usage (fallback)

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
#[cfg(test)]
use serde_json::Value;

const BASE: &str = "https://cursor.com";
const TIMEOUT_SECS: u64 = 15;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

/// Normalized usage pulled from Cursor's usage-summary (+ optional legacy).
#[derive(Debug, Clone, PartialEq)]
pub struct CursorUsage {
    pub plan_percent_used: f64,
    pub plan_used_usd: Option<f64>,
    pub plan_limit_usd: Option<f64>,
    pub on_demand_used_usd: Option<f64>,
    pub on_demand_limit_usd: Option<f64>,
    pub billing_cycle_start: Option<DateTime<Utc>>,
    pub billing_cycle_end: Option<DateTime<Utc>>,
    pub membership_type: Option<String>,
    pub account_email: Option<String>,
    pub account_name: Option<String>,
    /// Present when a legacy request-based plan was used for the headline percent.
    pub requests_used: Option<i64>,
    pub requests_limit: Option<i64>,
}

// --- Wire types for /api/usage-summary ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummary {
    billing_cycle_start: Option<String>,
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    individual_usage: Option<IndividualUsage>,
    team_usage: Option<TeamUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndividualUsage {
    plan: Option<PlanUsage>,
    on_demand: Option<OnDemandUsage>,
    overall: Option<OnDemandUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanUsage {
    used: Option<i64>,
    limit: Option<i64>,
    auto_percent_used: Option<f64>,
    api_percent_used: Option<f64>,
    total_percent_used: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnDemandUsage {
    used: Option<i64>,
    limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamUsage {
    on_demand: Option<OnDemandUsage>,
    pooled: Option<OnDemandUsage>,
}

// --- Wire types for /api/auth/me ---

#[derive(Debug, Clone, Deserialize)]
struct UserInfo {
    email: Option<String>,
    name: Option<String>,
    sub: Option<String>,
}

// --- Wire types for /api/usage (legacy) ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyUsageResponse {
    #[serde(rename = "gpt-4")]
    gpt4: Option<ModelUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelUsage {
    num_requests: Option<i64>,
    num_requests_total: Option<i64>,
    max_request_usage: Option<i64>,
}

pub async fn fetch_usage(cookie: &str) -> Result<CursorUsage> {
    let cookie = cookie.trim();
    if cookie.is_empty() {
        bail!("empty Cursor session cookie");
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .context("build reqwest client")?;

    let summary_fut = fetch_usage_summary(&client, cookie);
    let me_fut = fetch_user_info(&client, cookie);
    let (summary_res, me_res) = tokio::join!(summary_fut, me_fut);

    let summary = summary_res?;
    let user = me_res.ok().flatten();

    let mut usage = map_summary(&summary, user.as_ref())?;

    // If usage-summary gives a usable percent + cycle end, skip the legacy endpoint.
    let skip_legacy = summary_has_usable_percent(&summary) && usage.billing_cycle_end.is_some();

    if !skip_legacy
        && let Some(user_id) = user.as_ref().and_then(|u| u.sub.as_deref())
        && let Ok(legacy) = fetch_legacy_usage(&client, cookie, user_id).await
    {
        apply_legacy(&mut usage, &legacy);
    }

    Ok(usage)
}

fn summary_has_usable_percent(summary: &UsageSummary) -> bool {
    if let Some(plan) = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.plan.as_ref())
        && (plan.total_percent_used.is_some()
            || plan.auto_percent_used.is_some()
            || plan.api_percent_used.is_some()
            || plan.limit.unwrap_or(0) > 0)
    {
        return true;
    }
    if summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.overall.as_ref())
        .and_then(|o| o.limit)
        .is_some_and(|l| l > 0)
    {
        return true;
    }
    summary
        .team_usage
        .as_ref()
        .and_then(|t| t.pooled.as_ref())
        .and_then(|p| p.limit)
        .is_some_and(|l| l > 0)
}

async fn fetch_usage_summary(client: &reqwest::Client, cookie: &str) -> Result<UsageSummary> {
    let resp = client
        .get(format!("{BASE}/api/usage-summary"))
        .header("Accept", "application/json")
        .header("Cookie", cookie)
        .send()
        .await
        .context("GET /api/usage-summary")?;

    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        bail!("auth: HTTP {status} (session cookie invalid or expired)");
    }
    if !status.is_success() {
        bail!("HTTP {status} from /api/usage-summary");
    }

    let body = resp.bytes().await.context("read usage-summary body")?;
    serde_json::from_slice(&body).context("decode usage-summary JSON")
}

async fn fetch_user_info(client: &reqwest::Client, cookie: &str) -> Result<Option<UserInfo>> {
    let resp = client
        .get(format!("{BASE}/api/auth/me"))
        .header("Accept", "application/json")
        .header("Cookie", cookie)
        .send()
        .await
        .context("GET /api/auth/me")?;

    if !resp.status().is_success() {
        return Ok(None);
    }
    let body = resp.bytes().await.context("read /api/auth/me body")?;
    match serde_json::from_slice::<UserInfo>(&body) {
        Ok(u) => Ok(Some(u)),
        Err(_) => Ok(None),
    }
}

async fn fetch_legacy_usage(
    client: &reqwest::Client,
    cookie: &str,
    user_id: &str,
) -> Result<LegacyUsageResponse> {
    let resp = client
        .get(format!("{BASE}/api/usage"))
        .query(&[("user", user_id)])
        .header("Accept", "application/json")
        .header("Cookie", cookie)
        .send()
        .await
        .context("GET /api/usage")?;

    if !resp.status().is_success() {
        bail!("HTTP {} from /api/usage", resp.status());
    }
    let body = resp.bytes().await.context("read /api/usage body")?;
    serde_json::from_slice(&body).context("decode legacy usage JSON")
}

/// Decode a usage-summary JSON value into [`CursorUsage`] (pure; for tests).
#[cfg(test)]
pub(crate) fn decode_usage_summary(value: Value) -> Result<CursorUsage> {
    let summary: UsageSummary =
        serde_json::from_value(value).context("decode usage-summary fixture")?;
    map_summary(&summary, None)
}

/// Decode legacy `/api/usage` JSON (pure; for tests).
#[cfg(test)]
pub(crate) fn decode_legacy_usage(value: Value) -> Result<(Option<i64>, Option<i64>, f64)> {
    let legacy: LegacyUsageResponse =
        serde_json::from_value(value).context("decode legacy usage fixture")?;
    let used = legacy
        .gpt4
        .as_ref()
        .and_then(|g| g.num_requests_total.or(g.num_requests));
    let limit = legacy.gpt4.as_ref().and_then(|g| g.max_request_usage);
    let pct = match (used, limit) {
        (Some(u), Some(l)) if l > 0 => (u as f64) / (l as f64) * 100.0,
        _ => 0.0,
    };
    Ok((used, limit, pct))
}

fn map_summary(summary: &UsageSummary, user: Option<&UserInfo>) -> Result<CursorUsage> {
    let plan_used_raw = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.plan.as_ref())
        .and_then(|p| p.used)
        .unwrap_or(0) as f64;
    let plan_limit_raw = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.plan.as_ref())
        .and_then(|p| p.limit)
        .unwrap_or(0) as f64;

    let auto_percent = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.plan.as_ref())
        .and_then(|p| p.auto_percent_used)
        .and_then(norm_pct);
    let api_percent = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.plan.as_ref())
        .and_then(|p| p.api_percent_used)
        .and_then(norm_pct);

    let overall_used = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.overall.as_ref())
        .and_then(|o| o.used)
        .map(|v| v as f64);
    let overall_limit = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.overall.as_ref())
        .and_then(|o| o.limit)
        .map(|v| v as f64);

    let pooled_used = summary
        .team_usage
        .as_ref()
        .and_then(|t| t.pooled.as_ref())
        .and_then(|p| p.used)
        .map(|v| v as f64);
    let pooled_limit = summary
        .team_usage
        .as_ref()
        .and_then(|t| t.pooled.as_ref())
        .and_then(|p| p.limit)
        .map(|v| v as f64);

    let plan_percent_used: f64 = if let Some(total) = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.plan.as_ref())
        .and_then(|p| p.total_percent_used)
    {
        normalize_total_percent(total)
    } else if let (Some(a), Some(b)) = (auto_percent, api_percent) {
        ((a + b) / 2.0).clamp(0.0, 100.0)
    } else if let Some(a) = api_percent {
        a.clamp(0.0, 100.0)
    } else if let Some(a) = auto_percent {
        a.clamp(0.0, 100.0)
    } else if plan_limit_raw > 0.0 {
        (plan_used_raw / plan_limit_raw * 100.0).clamp(0.0, 100.0)
    } else if let (Some(used), Some(limit)) = (overall_used, overall_limit) {
        if limit > 0.0 {
            normalize_total_percent(used / limit * 100.0)
        } else {
            0.0
        }
    } else if let (Some(used), Some(limit)) = (pooled_used, pooled_limit) {
        if limit > 0.0 {
            normalize_total_percent(used / limit * 100.0)
        } else {
            0.0
        }
    } else {
        0.0
    };

    let (plan_used_usd, plan_limit_usd) = if plan_limit_raw > 0.0 || plan_used_raw > 0.0 {
        (Some(plan_used_raw / 100.0), Some(plan_limit_raw / 100.0))
    } else if let (Some(used), Some(limit)) = (overall_used, overall_limit) {
        (Some(used / 100.0), Some(limit / 100.0))
    } else if let (Some(used), Some(limit)) = (pooled_used, pooled_limit) {
        (Some(used / 100.0), Some(limit / 100.0))
    } else {
        (None, None)
    };

    let on_demand_used = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.on_demand.as_ref())
        .and_then(|o| o.used)
        .map(|c| c as f64 / 100.0);
    let on_demand_limit = summary
        .individual_usage
        .as_ref()
        .and_then(|i| i.on_demand.as_ref())
        .and_then(|o| o.limit)
        .map(|c| c as f64 / 100.0);

    // Prefer personal on-demand; fall back to team on-demand budget.
    let (on_demand_used_usd, on_demand_limit_usd) =
        if on_demand_limit.unwrap_or(0.0) > 0.0 || on_demand_used.unwrap_or(0.0) > 0.0 {
            (on_demand_used, on_demand_limit)
        } else {
            let team_used = summary
                .team_usage
                .as_ref()
                .and_then(|t| t.on_demand.as_ref())
                .and_then(|o| o.used)
                .map(|c| c as f64 / 100.0);
            let team_limit = summary
                .team_usage
                .as_ref()
                .and_then(|t| t.on_demand.as_ref())
                .and_then(|o| o.limit)
                .map(|c| c as f64 / 100.0);
            (team_used, team_limit)
        };

    Ok(CursorUsage {
        plan_percent_used,
        plan_used_usd,
        plan_limit_usd,
        on_demand_used_usd,
        on_demand_limit_usd,
        billing_cycle_start: parse_iso(summary.billing_cycle_start.as_deref()),
        billing_cycle_end: parse_iso(summary.billing_cycle_end.as_deref()),
        membership_type: summary.membership_type.clone(),
        account_email: user.and_then(|u| u.email.clone()),
        account_name: user.and_then(|u| u.name.clone()),
        requests_used: None,
        requests_limit: None,
    })
}

fn apply_legacy(usage: &mut CursorUsage, legacy: &LegacyUsageResponse) {
    let used = legacy
        .gpt4
        .as_ref()
        .and_then(|g| g.num_requests_total.or(g.num_requests));
    let limit = legacy.gpt4.as_ref().and_then(|g| g.max_request_usage);
    if let (Some(u), Some(l)) = (used, limit)
        && l > 0
    {
        usage.requests_used = Some(u);
        usage.requests_limit = Some(l);
        // Legacy request plans: headline percent is request-based.
        usage.plan_percent_used = (u as f64) / (l as f64) * 100.0;
    }
}

fn norm_pct(v: f64) -> Option<f64> {
    if !v.is_finite() {
        return None;
    }
    Some(v.clamp(0.0, 100.0))
}

fn normalize_total_percent(v: f64) -> f64 {
    v.clamp(0.0, 100.0)
}

fn parse_iso(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    // Prefer RFC3339 with fractional seconds, then without.
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            // Some responses omit the colon in the timezone offset.
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ"))
                .ok()
                .map(|n| n.and_utc())
        })
}
