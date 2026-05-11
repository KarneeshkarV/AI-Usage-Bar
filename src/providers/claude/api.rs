//! Claude.ai web API client. Uses a `sessionKey` cookie obtained from the
//! browser cookie store; surfaces session/weekly/sonnet/extra usage.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::time::Duration;

use super::{ClaudeData, ExtraSpend, Window};

const BASE: &str = "https://claude.ai/api";

pub async fn fetch(session_key: &str) -> Result<ClaudeData> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("ai_bar/0.1 (+https://github.com/karneeshkar/ai_bar)")
        .build()?;

    // 1. Account → email + plan tier from the first membership.
    let account: AccountResp = get(&client, session_key, &format!("{BASE}/account"))
        .await
        .context("GET /account")?;
    let (email, plan_tier) = extract_account(&account);

    // 2. Organizations → pick the chat-capable one.
    let orgs: Vec<OrgResp> = get(&client, session_key, &format!("{BASE}/organizations"))
        .await
        .context("GET /organizations")?;
    let org = pick_org(&orgs).ok_or_else(|| anyhow!("no usable org"))?;

    // 3. Usage (rate windows).
    let usage: UsageResp = get(
        &client,
        session_key,
        &format!("{BASE}/organizations/{}/usage", org.uuid),
    )
    .await
    .context("GET /usage")?;

    // 4. Overage / Extra usage (Claude Extra paid tier). Best-effort — many
    // accounts return 404 here.
    let extra = get::<OverageResp>(
        &client,
        session_key,
        &format!("{BASE}/organizations/{}/overage_spend_limit", org.uuid),
    )
    .await
    .ok()
    .and_then(extract_extra);

    Ok(ClaudeData {
        account_email: email,
        plan_type: plan_tier,
        session: usage.five_hour.map(Window::from),
        weekly: usage.seven_day.map(Window::from),
        sonnet_weekly: usage.seven_day_sonnet.map(Window::from),
        extra,
    })
}

async fn get<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    session_key: &str,
    url: &str,
) -> Result<T> {
    let resp = client
        .get(url)
        .header("Cookie", format!("sessionKey={session_key}"))
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("{status} from {url}: {}", body.chars().take(200).collect::<String>()));
    }
    Ok(resp.json::<T>().await?)
}

// ---- Response shapes ----

#[derive(Deserialize)]
struct AccountResp {
    email_address: Option<String>,
    memberships: Option<Vec<MembershipResp>>,
}

#[derive(Deserialize)]
struct MembershipResp {
    organization: Option<OrgInner>,
}

#[derive(Deserialize)]
struct OrgInner {
    rate_limit_tier: Option<String>,
}

#[derive(Deserialize)]
struct OrgResp {
    uuid: String,
    #[allow(dead_code)]
    name: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Deserialize)]
struct UsageResp {
    five_hour: Option<RateWindowJson>,
    seven_day: Option<RateWindowJson>,
    seven_day_sonnet: Option<RateWindowJson>,
}

#[derive(Deserialize)]
struct RateWindowJson {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

impl From<RateWindowJson> for Window {
    fn from(w: RateWindowJson) -> Self {
        let resets_at = w
            .resets_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        Window {
            used_percent: w.utilization.unwrap_or(0.0),
            resets_at,
        }
    }
}

#[derive(Deserialize)]
struct OverageResp {
    monthly_credit_limit: Option<i64>,
    used_credits: Option<i64>,
    currency: Option<String>,
    is_enabled: Option<bool>,
}

fn extract_account(a: &AccountResp) -> (Option<String>, Option<String>) {
    let email = a.email_address.clone();
    let plan = a
        .memberships
        .as_ref()
        .and_then(|m| m.first())
        .and_then(|m| m.organization.as_ref())
        .and_then(|o| o.rate_limit_tier.clone());
    (email, plan)
}

fn pick_org(orgs: &[OrgResp]) -> Option<&OrgResp> {
    orgs.iter()
        .find(|o| o.capabilities.iter().any(|c| c == "chat"))
        .or_else(|| orgs.first())
}

fn extract_extra(o: OverageResp) -> Option<ExtraSpend> {
    if !o.is_enabled.unwrap_or(false) {
        return None;
    }
    let limit_cents = o.monthly_credit_limit? as f64;
    let used_cents = o.used_credits? as f64;
    Some(ExtraSpend {
        used_usd: used_cents / 100.0,
        limit_usd: limit_cents / 100.0,
        currency: o.currency.unwrap_or_else(|| "USD".into()),
    })
}
