//! Browser cookie extraction for Chromium-family browsers on Linux.
//!
//! Reads `Cookies` SQLite under each browser's profile dir, decrypts the
//! `sessionKey` entry for `.claude.ai`, returns the plaintext cookie value.

use aes::Aes128;
use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use anyhow::{Result, anyhow};
use hmac::Hmac;
use rusqlite::Connection;
use secret_service::{EncryptionType, SecretService};
use sha1::Sha1;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const SALT: &[u8] = b"saltysalt";
const IV: [u8; 16] = [0x20; 16];
const KEY_LEN: usize = 16;
const PBKDF2_ITERS: u32 = 1;
/// Fallback "v10" key used when libsecret is unavailable.
const FALLBACK_PASSWORD: &[u8] = b"peanuts";

#[derive(Debug, Clone)]
pub struct SessionKey {
    pub value: String,
    pub source: String,
}

/// Try every supported browser + profile, return the first valid sessionKey.
pub async fn find_session_key() -> Result<SessionKey> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let mut last_err: Option<anyhow::Error> = None;

    for browser in browsers() {
        let base = home.join(&browser.config_subpath);
        if !base.exists() {
            continue;
        }
        // Resolve the libsecret key for this browser (or fall back to v10/"peanuts").
        let key = match derive_key_via_secret_service(&browser.libsecret_app_names).await {
            Ok(k) => k,
            Err(e) => {
                tracing::debug!(browser=%browser.name, error=%e, "no libsecret key, will use v10 fallback only");
                derive_key_from_password(FALLBACK_PASSWORD)
            }
        };

        for profile in enumerate_profiles(&base) {
            let cookies_db = locate_cookies_file(&profile);
            let Some(cookies_db) = cookies_db else {
                continue;
            };
            match read_session_key(&cookies_db, &key) {
                Ok(Some(plaintext)) => {
                    return Ok(SessionKey {
                        value: plaintext,
                        source: format!(
                            "{} ({})",
                            browser.name,
                            profile.file_name().and_then(|s| s.to_str()).unwrap_or("?")
                        ),
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(db=%cookies_db.display(), error=%e, "skip cookie db");
                    last_err = Some(e);
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("no Claude sessionKey found in any browser")))
}

struct Browser {
    name: &'static str,
    config_subpath: PathBuf,
    libsecret_app_names: Vec<&'static str>,
}

fn browsers() -> Vec<Browser> {
    vec![
        Browser {
            name: "Chrome",
            config_subpath: PathBuf::from(".config/google-chrome"),
            libsecret_app_names: vec!["chrome", "google-chrome"],
        },
        Browser {
            name: "Brave",
            config_subpath: PathBuf::from(".config/BraveSoftware/Brave-Browser"),
            libsecret_app_names: vec!["brave", "brave-browser"],
        },
        Browser {
            name: "Chromium",
            config_subpath: PathBuf::from(".config/chromium"),
            libsecret_app_names: vec!["chromium"],
        },
        Browser {
            name: "Edge",
            config_subpath: PathBuf::from(".config/microsoft-edge"),
            libsecret_app_names: vec!["microsoft-edge", "msedge"],
        },
        Browser {
            name: "Vivaldi",
            config_subpath: PathBuf::from(".config/vivaldi"),
            libsecret_app_names: vec!["vivaldi"],
        },
    ]
}

fn enumerate_profiles(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if base.join("Default").is_dir() {
        out.push(base.join("Default"));
    }
    if let Ok(entries) = std::fs::read_dir(base) {
        for e in entries.flatten() {
            let name = match e.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if name.starts_with("Profile ") {
                out.push(e.path());
            }
        }
    }
    out
}

fn locate_cookies_file(profile: &Path) -> Option<PathBuf> {
    // Newer Chromium puts it under Network/Cookies; older versions used Cookies directly.
    [
        profile.join("Network").join("Cookies"),
        profile.join("Cookies"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

async fn derive_key_via_secret_service(app_names: &[&str]) -> Result<[u8; KEY_LEN]> {
    let ss = SecretService::connect(EncryptionType::Dh).await?;
    let collection = ss.get_default_collection().await?;
    for app in app_names {
        let mut attrs = HashMap::new();
        attrs.insert("application", *app);
        let items = collection.search_items(attrs).await.unwrap_or_default();
        if let Some(item) = items.first() {
            let secret = item.get_secret().await?;
            return Ok(derive_key_from_password(&secret));
        }
    }
    Err(anyhow!(
        "libsecret entry not found for any of: {:?}",
        app_names
    ))
}

fn derive_key_from_password(password: &[u8]) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    pbkdf2::pbkdf2::<Hmac<Sha1>>(password, SALT, PBKDF2_ITERS, &mut out)
        .expect("pbkdf2 fixed sizes");
    out
}

fn read_session_key(db_path: &Path, key: &[u8; KEY_LEN]) -> Result<Option<String>> {
    // Browser may hold an exclusive lock. Copy to a tmpfile so we can read.
    let tmp = std::env::temp_dir().join(format!("ai-usage-bar-cookies-{}.db", std::process::id()));
    std::fs::copy(db_path, &tmp)?;
    let _cleanup = TmpFile(tmp.clone());

    let uri = format!("file:{}?mode=ro&immutable=1", tmp.display());
    let conn = Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    // The Chromium schema names the encrypted column `encrypted_value`.
    // The host column has varied: `host_key` (recent) vs `host` (very old).
    let mut stmt = conn.prepare(
        "SELECT encrypted_value FROM cookies WHERE host_key LIKE '%claude.ai' AND name = 'sessionKey' ORDER BY expires_utc DESC",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let blob: Vec<u8> = row.get(0)?;
        if let Some(pt) = decrypt_cookie_value(&blob, key)?
            && pt.starts_with("sk-ant-")
        {
            return Ok(Some(pt));
        }
    }
    Ok(None)
}

struct TmpFile(PathBuf);
impl Drop for TmpFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn decrypt_cookie_value(blob: &[u8], key: &[u8; KEY_LEN]) -> Result<Option<String>> {
    if blob.len() < 3 {
        return Ok(None);
    }
    let prefix = &blob[..3];
    let ciphertext = match prefix {
        b"v10" | b"v11" => &blob[3..],
        _ => return Ok(None), // unencrypted on older versions is rare; ignore
    };
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Ok(None);
    }
    let mut buf = ciphertext.to_vec();
    let plaintext = Aes128CbcDec::new(key.into(), &IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| anyhow!("aes decrypt: {e}"))?;

    // Chromium may prepend a 32-byte SHA-256 of the cookie host (since ~v130 on Linux).
    // Heuristic: try the raw decode; if not valid UTF-8 or doesn't look like the key,
    // strip 32 bytes and try again.
    if let Ok(s) = std::str::from_utf8(plaintext)
        && s.starts_with("sk-ant-")
    {
        return Ok(Some(s.to_string()));
    }
    if plaintext.len() > 32
        && let Ok(s) = std::str::from_utf8(&plaintext[32..])
        && s.starts_with("sk-ant-")
    {
        return Ok(Some(s.to_string()));
    }
    Ok(None)
}
