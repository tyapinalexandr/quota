//! Kimi Code (Moonshot AI subscription) provider: quota windows for an API key
//! the user pastes in from the Kimi Code console (kimi.ai/code).
//!
//! One endpoint is used:
//!   GET https://api.kimi.com/coding/v1/usages
//! It reports the weekly/monthly quota, the 5-hour sliding window and the
//! short rate window, each with a personalized `resetTime` — exactly the
//! "how much is left and when does it reset" view the console shows.
//! Keys for the Kimi Open Platform (pay-as-you-go, api.moonshot.ai) are a
//! separate billing system and are rejected here with a hint.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::cursor::now_ts;

const DATA_DIR: &str = ".quota";
const ACCOUNTS_DIR: &str = "kimi_accounts";
const ACCOUNTS_INDEX_FILE: &str = "kimi_accounts.json";

const KIMI_USAGES_URL: &str = "https://api.kimi.com/coding/v1/usages";
const KIMI_ME_URL: &str = "https://api.kimi.com/coding/v1/me";

// ---------------------------------------------------------------------------
// Disk storage types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredKimiAccount {
    id: String,
    label: String,
    /// On Windows: a `wcm:` reference into the Credential Manager, never the
    /// key itself. See `token_store`.
    api_key: String,
    /// Share of the monthly quota still available, 0-100.
    #[serde(skip_serializing_if = "Option::is_none")]
    monthly_remaining_percent: Option<f64>,
    /// Share of the monthly quota used, 0-100.
    #[serde(skip_serializing_if = "Option::is_none")]
    monthly_used_percent: Option<f64>,
    /// Unix seconds when the monthly quota resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    monthly_reset_at: Option<i64>,
    /// Share of the 5-hour window still available, 0-100.
    #[serde(skip_serializing_if = "Option::is_none")]
    five_hour_remaining_percent: Option<f64>,
    /// Unix seconds when the 5-hour window resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    five_hour_reset_at: Option<i64>,
    /// Requests left in the short rate window (e.g. 81/100 per 5 minutes).
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit_remaining: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit_limit: Option<i64>,
    /// Unix seconds when the rate window resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit_reset_at: Option<i64>,
    /// Subscription level name from /me ("Max", "Ultra"...).
    #[serde(skip_serializing_if = "Option::is_none")]
    user_level_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quota_query_last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_updated_at: Option<i64>,
    created_at: i64,
    last_used: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KimiAccountIndex {
    version: String,
    account_ids: Vec<String>,
}

impl KimiAccountIndex {
    fn new() -> Self {
        Self {
            version: "1.0".to_string(),
            account_ids: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Public summary sent to React (no API key)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiAccountSummary {
    pub id: String,
    pub label: String,
    /// Last characters of the key, purely so the user can tell keys apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_remaining_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_used_percent: Option<f64>,
    /// Unix seconds when the monthly quota resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_reset_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_remaining_percent: Option<f64>,
    /// Unix seconds when the 5-hour window resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_reset_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_remaining: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_reset_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_level_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_query_last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_updated_at: Option<i64>,
    pub created_at: i64,
    pub last_used: i64,
}

impl StoredKimiAccount {
    fn to_summary(&self) -> KimiAccountSummary {
        KimiAccountSummary {
            id: self.id.clone(),
            label: self.label.clone(),
            key_hint: key_hint(&self.api_key),
            monthly_remaining_percent: self.monthly_remaining_percent,
            monthly_used_percent: self.monthly_used_percent,
            monthly_reset_at: self.monthly_reset_at,
            five_hour_remaining_percent: self.five_hour_remaining_percent,
            five_hour_reset_at: self.five_hour_reset_at,
            rate_limit_remaining: self.rate_limit_remaining,
            rate_limit_limit: self.rate_limit_limit,
            rate_limit_reset_at: self.rate_limit_reset_at,
            user_level_name: self.user_level_name.clone(),
            quota_query_last_error: self.quota_query_last_error.clone(),
            usage_updated_at: self.usage_updated_at,
            created_at: self.created_at,
            last_used: self.last_used,
        }
    }
}

/// `…abcd` from a stored plaintext key; `None` for keyring references.
fn key_hint(api_key: &str) -> Option<String> {
    if crate::token_store::is_sealed(api_key) {
        return None;
    }
    let tail: String = api_key
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if tail.is_empty() {
        None
    } else {
        Some(format!("…{tail}"))
    }
}

// ---------------------------------------------------------------------------
// Pending key additions
// ---------------------------------------------------------------------------

struct PendingKimiKey {
    api_key: String,
    label: Option<String>,
}

fn pending_keys() -> &'static Mutex<HashMap<String, PendingKimiKey>> {
    static PENDING: OnceLock<Mutex<HashMap<String, PendingKimiKey>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Storage layout
// ---------------------------------------------------------------------------

fn quota_storage_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Could not locate home directory".to_string())?;
    Ok(home.join(DATA_DIR))
}

fn accounts_dir_in(storage_dir: &Path) -> PathBuf {
    storage_dir.join(ACCOUNTS_DIR)
}

fn index_path_in(storage_dir: &Path) -> PathBuf {
    storage_dir.join(ACCOUNTS_INDEX_FILE)
}

fn account_path_in(storage_dir: &Path, id: &str) -> PathBuf {
    accounts_dir_in(storage_dir).join(format!("{}.json", id))
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "No parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("Could not create directory: {}", e))?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("kimi"),
        std::process::id()
    ));
    fs::write(&tmp, content).map_err(|e| format!("Could not write temp file: {}", e))?;
    fs::rename(&tmp, path).map_err(|e| format!("Could not rename temp file: {}", e))?;
    Ok(())
}

fn load_index_in(storage_dir: &Path) -> Result<KimiAccountIndex, String> {
    let path = index_path_in(storage_dir);
    if !path.exists() {
        return Ok(KimiAccountIndex::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Could not read Kimi account index: {}", e))?;
    if content.trim().is_empty() {
        return Ok(KimiAccountIndex::new());
    }
    serde_json::from_str(&content)
        .map_err(|e| format!("Could not parse Kimi account index: {}", e))
}

fn save_index_in(storage_dir: &Path, index: &KimiAccountIndex) -> Result<(), String> {
    let content = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Could not encode Kimi account index: {}", e))?;
    write_atomic(&index_path_in(storage_dir), &content)
}

fn load_account_in(storage_dir: &Path, id: &str) -> Result<StoredKimiAccount, String> {
    let content = fs::read_to_string(account_path_in(storage_dir, id))
        .map_err(|e| format!("Could not read Kimi account: {}", e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Could not parse Kimi account: {}", e))
}

fn save_account_in(storage_dir: &Path, account: &StoredKimiAccount) -> Result<(), String> {
    // The API key goes into the OS credential store (Windows Credential
    // Manager); the JSON on disk only keeps a `wcm:` reference to it.
    let mut stored = account.clone();
    stored.api_key =
        crate::token_store::seal(&format!("kimi-{}-apikey", account.id), &account.api_key)?;
    let content = serde_json::to_string_pretty(&stored)
        .map_err(|e| format!("Could not encode Kimi account: {}", e))?;
    write_atomic(&account_path_in(storage_dir, &account.id), &content)
}

/// Loads an account with its API key resolved from the credential store.
fn load_account_with_key_in(
    storage_dir: &Path,
    id: &str,
) -> Result<(StoredKimiAccount, String), String> {
    let account = load_account_in(storage_dir, id)?;
    let api_key = crate::token_store::unseal(&account.api_key)?;
    Ok((account, api_key))
}

fn upsert_account_in(
    storage_dir: &Path,
    mut account: StoredKimiAccount,
) -> Result<KimiAccountSummary, String> {
    let mut index = load_index_in(storage_dir)?;
    if let Ok(existing) = load_account_in(storage_dir, &account.id) {
        account.created_at = existing.created_at;
    }
    account.last_used = now_ts();
    if !index.account_ids.iter().any(|id| id == &account.id) {
        index.account_ids.insert(0, account.id.clone());
    }
    save_account_in(storage_dir, &account)?;
    save_index_in(storage_dir, &index)?;
    Ok(account.to_summary())
}

// ---------------------------------------------------------------------------
// Response parsing (pure, unit-tested)
// ---------------------------------------------------------------------------

/// Numeric fields arrive as strings ("100") in some responses and as numbers
/// in others; accept both.
fn value_as_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    match value {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn value_as_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    value_as_f64(value).map(|v| v as i64)
}

/// RFC 3339 ("2026-10-19T00:00:00Z", sometimes with fractional seconds) to
/// Unix seconds.
fn parse_reset_time(value: Option<&serde_json::Value>) -> Option<i64> {
    let raw = value?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp())
}

#[derive(Debug)]
struct ParsedUsage {
    monthly_used_percent: Option<f64>,
    monthly_reset_at: Option<i64>,
    five_hour_remaining_percent: Option<f64>,
    five_hour_reset_at: Option<i64>,
    rate_limit_remaining: Option<i64>,
    rate_limit_limit: Option<i64>,
    rate_limit_reset_at: Option<i64>,
}

fn parse_usages(raw: &serde_json::Value) -> Result<ParsedUsage, String> {
    if raw.get("error").is_some() {
        let message = raw
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("Kimi rejected the usage request");
        return Err(message.to_string());
    }

    let usages = raw
        .get("usages")
        .ok_or_else(|| "Kimi usage response has no usages object".to_string())?;

    // Monthly quota: the console shows the total bucket (limit_month_total),
    // matching the "Использование за месяц" card, so prefer it and fall back
    // to the code bucket.
    let monthly_bucket = usages
        .get("limit_month_total")
        .or_else(|| usages.get("limit_month_code"));
    let monthly_used_percent = monthly_bucket
        .and_then(|b| value_as_f64(b.get("used_ratio")))
        .map(|ratio| ratio * 100.0);
    let monthly_reset_at = monthly_bucket.and_then(|b| parse_reset_time(b.get("reset_time")));

    // The "5-hour window" the console displays is the `limits` entry with a
    // 300-minute window, derived from used/limit counts. Fall back to the
    // limit_5h usage bucket when the limits array is absent.
    let five_hour = raw
        .get("limits")
        .and_then(|l| l.as_array())
        .and_then(|arr| {
            arr.iter().find(|entry| {
                entry
                    .pointer("/window/duration")
                    .and_then(|v| v.as_i64())
                    .map(|d| d == 300)
                    .unwrap_or(false)
            })
        })
        .map(|entry| entry.get("detail").unwrap_or(entry));
    let five_hour_from_limits = five_hour.and_then(|detail| {
        let limit = value_as_f64(detail.get("limit"))?;
        if limit <= 0.0 {
            return None;
        }
        let used = value_as_f64(detail.get("used")).unwrap_or_else(|| {
            let remaining = value_as_f64(detail.get("remaining")).unwrap_or(0.0);
            (limit - remaining).max(0.0)
        });
        Some(((1.0 - used / limit) * 100.0).clamp(0.0, 100.0))
    });
    let five_hour_remaining_percent = five_hour_from_limits.or_else(|| {
        usages
            .get("limit_5h")
            .and_then(|b| value_as_f64(b.get("used_ratio")))
            .map(|ratio| (1.0 - ratio) * 100.0)
    });
    let five_hour_reset_at = five_hour
        .and_then(|d| parse_reset_time(d.get("resetTime")))
        .or_else(|| {
            usages
                .get("limit_5h")
                .and_then(|b| parse_reset_time(b.get("reset_time")))
        });

    // Short rate window: any non-5h entry of `limits` (present only for
    // some subscriptions).
    let rate = raw
        .get("limits")
        .and_then(|l| l.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|entry| {
                    entry
                        .pointer("/window/duration")
                        .and_then(|v| v.as_i64())
                        .map(|d| d != 300)
                        .unwrap_or(false)
                })
        })
        .map(|entry| entry.get("detail").unwrap_or(entry));
    let rate_limit_remaining = rate.and_then(|d| value_as_i64(d.get("remaining")));
    let rate_limit_limit = rate.and_then(|d| value_as_i64(d.get("limit")));
    let rate_limit_reset_at = rate.and_then(|d| parse_reset_time(d.get("resetTime")));

    Ok(ParsedUsage {
        monthly_used_percent,
        monthly_reset_at,
        five_hour_remaining_percent,
        five_hour_reset_at,
        rate_limit_remaining,
        rate_limit_limit,
        rate_limit_reset_at,
    })
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

async fn fetch_usages(client: &reqwest::Client, api_key: &str) -> Result<serde_json::Value, String> {
    let response = client
        .get(KIMI_USAGES_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .map_err(|e| format!("Kimi request failed: {}", e))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(
            "Kimi rejected the API key (401 Unauthorized). Make sure it is a Kimi Code key from \
             kimi.ai/code (sk-kimi-...) copied in full — Kimi Open Platform keys \
             (platform.moonshot.ai, pay-as-you-go) are a separate billing system."
                .to_string(),
        );
    }
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("Could not read the Kimi response: {}", e))?;
    if !status.is_success() {
        return Err(format!("Kimi request failed with {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("Could not parse the Kimi response: {}", e))
}

/// Subscription level from GET /me ("user_level_name": "Max"). Best-effort:
/// the quotas already came through, so a /me hiccup must not fail the refresh.
async fn fetch_user_level_name(client: &reqwest::Client, api_key: &str) -> Option<String> {
    let body = client
        .get(KIMI_ME_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    let raw: serde_json::Value = serde_json::from_str(&body).ok()?;
    raw.get("user_level_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Validates a key against the usages endpoint without touching storage.
async fn verify_key(api_key: &str) -> Result<(), String> {
    let client = build_http_client()?;
    let raw = fetch_usages(&client, api_key).await?;
    parse_usages(&raw).map(|_| ())
}

async fn refresh_stored_account(
    mut account: StoredKimiAccount,
    api_key: &str,
) -> StoredKimiAccount {
    let client = match build_http_client() {
        Ok(client) => client,
        Err(e) => {
            account.quota_query_last_error = Some(e);
            return account;
        }
    };

    match fetch_usages(&client, api_key).await.and_then(|raw| parse_usages(&raw)) {
        Ok(parsed) => {
            account.monthly_used_percent = parsed.monthly_used_percent;
            account.monthly_remaining_percent =
                parsed.monthly_used_percent.map(|used| (100.0 - used).max(0.0));
            account.monthly_reset_at = parsed.monthly_reset_at;
            account.five_hour_remaining_percent = parsed.five_hour_remaining_percent;
            account.five_hour_reset_at = parsed.five_hour_reset_at;
            account.rate_limit_remaining = parsed.rate_limit_remaining;
            account.rate_limit_limit = parsed.rate_limit_limit;
            account.rate_limit_reset_at = parsed.rate_limit_reset_at;
            if let Some(level) = fetch_user_level_name(&client, api_key).await {
                account.user_level_name = Some(level);
            }
            account.usage_updated_at = Some(now_ts());
            account.quota_query_last_error = None;
        }
        Err(e) => {
            account.quota_query_last_error = Some(e);
        }
    }

    account
}

fn build_account_id(api_key: &str) -> String {
    format!("kimi_{:x}", md5::compute(api_key.as_bytes()))
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_kimi_accounts() -> Result<Vec<KimiAccountSummary>, String> {
    let dir = quota_storage_dir()?;
    let index = load_index_in(&dir)?;
    Ok(index
        .account_ids
        .iter()
        .filter_map(|id| load_account_in(&dir, id).ok())
        .map(|a| a.to_summary())
        .collect())
}

#[tauri::command]
pub async fn kimi_add_key_start(api_key: String, label: Option<String>) -> Result<String, String> {
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("Paste an API key first".to_string());
    }
    verify_key(&api_key).await?;
    let login_id = simple_uuid();
    pending_keys()
        .lock()
        .map_err(|_| "Internal lock poisoned".to_string())?
        .insert(login_id.clone(), PendingKimiKey { api_key, label });
    Ok(login_id)
}

#[tauri::command]
pub fn kimi_add_key_complete(login_id: String) -> Result<KimiAccountSummary, String> {
    let pending = pending_keys()
        .lock()
        .map_err(|_| "Internal lock poisoned".to_string())?
        .remove(&login_id)
        .ok_or_else(|| "This Kimi key addition expired; start over".to_string())?;

    let id = build_account_id(&pending.api_key);
    let now = now_ts();
    let label = pending
        .label
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| format!("Kimi {}", key_hint(&pending.api_key).unwrap_or_default()));

    let account = StoredKimiAccount {
        id,
        label,
        api_key: pending.api_key,
        monthly_remaining_percent: None,
        monthly_used_percent: None,
        monthly_reset_at: None,
        five_hour_remaining_percent: None,
        five_hour_reset_at: None,
        rate_limit_remaining: None,
        rate_limit_limit: None,
        rate_limit_reset_at: None,
        user_level_name: None,
        quota_query_last_error: None,
        usage_updated_at: None,
        created_at: now,
        last_used: now,
    };

    let dir = quota_storage_dir()?;
    upsert_account_in(&dir, account)
}

#[tauri::command]
pub fn kimi_add_key_cancel(login_id: Option<String>) -> Result<(), String> {
    if let Some(id) = login_id {
        pending_keys()
            .lock()
            .map_err(|_| "Internal lock poisoned".to_string())?
            .remove(&id);
    }
    Ok(())
}

#[tauri::command]
pub async fn refresh_kimi_account(account_id: String) -> Result<KimiAccountSummary, String> {
    let dir = quota_storage_dir()?;
    let (account, api_key) = load_account_with_key_in(&dir, &account_id)?;
    let updated = refresh_stored_account(account, &api_key).await;
    upsert_account_in(&dir, updated)
}

#[tauri::command]
pub async fn refresh_all_kimi_accounts() -> Result<Vec<KimiAccountSummary>, String> {
    let dir = quota_storage_dir()?;
    let index = load_index_in(&dir)?;
    let mut summaries = Vec::new();
    for id in &index.account_ids {
        if let Ok((account, api_key)) = load_account_with_key_in(&dir, id) {
            let updated = refresh_stored_account(account, &api_key).await;
            if let Ok(summary) = upsert_account_in(&dir, updated) {
                summaries.push(summary);
            }
        }
    }
    Ok(summaries)
}

#[tauri::command]
pub fn delete_kimi_account(account_id: String) -> Result<(), String> {
    let dir = quota_storage_dir()?;
    // Best effort: drop the key from the OS credential store.
    if let Ok(account) = load_account_in(&dir, &account_id) {
        crate::token_store::remove(&account.api_key);
    }
    let mut index = load_index_in(&dir)?;
    index.account_ids.retain(|id| id != &account_id);
    save_index_in(&dir, &index)?;
    let path = account_path_in(&dir, &account_id);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("Could not delete Kimi account: {}", e))?;
    }
    Ok(())
}

/// Random 128-bit id rendered as hex; the same shape Cursor logins use.
fn simple_uuid() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen::<u8>()).collect();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape api.kimi.com/coding/v1/usages returned for a real
    /// subscription key (percentages as ratios, limits as strings).
    #[test]
    fn parses_a_real_usages_response() {
        let raw = serde_json::json!({
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": {
                        "limit": "100",
                        "used": "19",
                        "remaining": "81",
                        "resetTime": "2026-09-22T14:09:42.054749Z"
                    }
                }
            ],
            "usages": {
                "limit_5h": { "used_ratio": 0, "reset_time": "2026-09-22T14:09:41Z" },
                "limit_month_total": { "used_ratio": 0.0774, "reset_time": "2026-10-19T00:00:00Z" },
                "limit_month_code": { "used_ratio": 0, "reset_time": "2026-10-19T00:00:00Z" }
            }
        });
        let parsed = parse_usages(&raw).unwrap();
        // The console's monthly card shows the total bucket.
        assert!((parsed.monthly_used_percent.unwrap() - 7.74).abs() < 1e-6);
        assert_eq!(
            parsed.monthly_reset_at,
            Some(chrono::DateTime::parse_from_rfc3339("2026-10-19T00:00:00Z").unwrap().timestamp())
        );
        // The 5-hour row comes from the 300-minute limits entry: 19/100 used
        // => 81% left, matching the console's rate-limit card.
        assert!((parsed.five_hour_remaining_percent.unwrap() - 81.0).abs() < 1e-6);
        assert!(parsed.five_hour_reset_at.is_some());
        // The 300-minute entry is the 5h window, so no separate rate row.
        assert_eq!(parsed.rate_limit_limit, None);
    }

    #[test]
    fn falls_back_to_limit_5h_and_keeps_non_300m_rate_windows() {
        let raw = serde_json::json!({
            "limits": [
                {
                    "window": { "duration": 60, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": {
                        "limit": "20",
                        "used": "5",
                        "remaining": "15",
                        "resetTime": "2026-09-22T13:00:00Z"
                    }
                }
            ],
            "usages": {
                "limit_5h": { "used_ratio": 0.4, "reset_time": "2026-09-22T16:00:00Z" },
                "limit_month_total": { "used_ratio": 0.1, "reset_time": "2026-10-19T00:00:00Z" }
            }
        });
        let parsed = parse_usages(&raw).unwrap();
        // No 300-minute window: the 5h row uses the limit_5h bucket, the 60m
        // window stays as the rate row.
        assert!((parsed.five_hour_remaining_percent.unwrap() - 60.0).abs() < 1e-6);
        assert_eq!(
            parsed.five_hour_reset_at,
            Some(chrono::DateTime::parse_from_rfc3339("2026-09-22T16:00:00Z").unwrap().timestamp())
        );
        assert_eq!(parsed.rate_limit_remaining, Some(15));
        assert_eq!(parsed.rate_limit_limit, Some(20));
    }

    #[test]
    fn rejects_an_error_response_with_its_message() {
        let raw = serde_json::json!({
            "error": { "message": "Invalid Authentication", "type": "invalid_authentication_error" }
        });
        let err = parse_usages(&raw).unwrap_err();
        assert!(err.contains("Invalid Authentication"), "got: {err}");
    }

    #[test]
    fn key_hints_hide_full_keys() {
        assert_eq!(key_hint("sk-kimi-abcdefgh"), Some("…efgh".to_string()));
        assert_eq!(key_hint("wcm:kimi-1-apikey"), None);
    }

    #[test]
    fn accepts_string_and_numeric_fields() {
        assert_eq!(value_as_f64(Some(&serde_json::json!("81"))), Some(81.0));
        assert_eq!(value_as_f64(Some(&serde_json::json!(0.0774))), Some(0.0774));
        assert_eq!(value_as_f64(Some(&serde_json::json!(null))), None);
    }
}
