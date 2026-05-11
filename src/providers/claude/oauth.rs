use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use secret_service::{EncryptionType, SecretService};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Default, Clone)]
pub struct OAuthMeta {
    pub email: Option<String>,
    pub plan: Option<String>,
    #[allow(dead_code)]
    pub account_id: Option<String>,
}

#[derive(Deserialize)]
struct StoredCreds {
    #[serde(rename = "id_token", alias = "idToken")]
    id_token: Option<String>,
    #[serde(rename = "access_token", alias = "accessToken")]
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct IdTokenClaims {
    email: Option<String>,
    #[serde(rename = "chatgpt_plan_type")]
    plan_type: Option<String>,
    #[serde(rename = "chatgpt_account_id")]
    account_id: Option<String>,
}

pub async fn fetch() -> Result<OAuthMeta> {
    if let Ok(meta) = from_file() {
        return Ok(meta);
    }
    from_secret_service().await
}

fn from_file() -> Result<OAuthMeta> {
    let home = dirs::home_dir().context("no home")?;
    for candidate in [
        home.join(".claude").join("oauth_store"),
        home.join(".claude").join(".credentials.json"),
    ] {
        if candidate.is_file() {
            let raw = std::fs::read_to_string(&candidate)?;
            return parse_creds_blob(&raw, &candidate);
        }
    }
    anyhow::bail!("no oauth file found");
}

fn parse_creds_blob(raw: &str, path: &PathBuf) -> Result<OAuthMeta> {
    let creds: StoredCreds = serde_json::from_str(raw)
        .with_context(|| format!("parse {}", path.display()))?;
    let token = creds.id_token.or(creds.access_token).context("no token")?;
    decode_id_token(&token)
}

async fn from_secret_service() -> Result<OAuthMeta> {
    let ss = SecretService::connect(EncryptionType::Dh)
        .await
        .context("secret service connect")?;
    let collection = ss.get_default_collection().await?;
    let items = collection
        .search_items(vec![("service", "Claude Code-credentials")].into_iter().collect())
        .await
        .context("secret-service search")?;
    let item = items.first().context("no Claude Code-credentials in secret service")?;
    let secret = item.get_secret().await?;
    let blob = String::from_utf8_lossy(&secret).to_string();
    parse_creds_blob(&blob, &PathBuf::from("(secret-service)"))
}

fn decode_id_token(token: &str) -> Result<OAuthMeta> {
    let mid = token.split('.').nth(1).context("malformed jwt")?;
    let bytes = URL_SAFE_NO_PAD.decode(mid).context("base64 decode")?;
    let claims: IdTokenClaims =
        serde_json::from_slice(&bytes).context("parse id_token claims")?;
    Ok(OAuthMeta {
        email: claims.email,
        plan: claims.plan_type,
        account_id: claims.account_id,
    })
}
