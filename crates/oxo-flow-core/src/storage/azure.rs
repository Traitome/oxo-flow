//! Azure Blob Storage backend using HMAC-SHA256 SharedKey authentication and
//! the Azure Blob REST API.
//!
//! The backend reads credentials from the following environment variables:
//!
//! 1. `AZURE_STORAGE_ACCOUNT_NAME` + `AZURE_STORAGE_ACCOUNT_KEY` (SharedKey)
//! 2. `AZURE_STORAGE_SAS_TOKEN` (SAS token — scoped, time-limited)
//! 3. `AZURE_STORAGE_CONNECTION_STRING` (fallback; parsed into account+key)
//!
//! # Authentication
//!
//! The Azure Blob REST API uses an HMAC-SHA256 signature scheme. The
//! `Authorization` header is formatted as:
//!
//! ```text
//! SharedKey <account-name>:<base64(hmac-sha256(account-key, string-to-sign))>
//! ```
//!
//! The `string-to-sign` is the newline-joined concatenation of the HTTP
//! verb, standard headers, and canonicalized Azure headers + resource.
//!
//! # Feature flag
//!
//! This module is only compiled when the `azure-storage` feature is enabled.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{OxoFlowError, Result};
use crate::storage::{RemoteStat, StorageBackend, StoragePath};

use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Credential resolution
// ---------------------------------------------------------------------------

/// Azure Storage credential variants.
#[derive(Debug, Clone)]
enum AzureCredentials {
    /// SharedKey: account name + base64-encoded account key.
    SharedKey { account: String, key: String },
    /// SAS token: pre-signed query string (starts with `?` or `&`).
    SasToken { account: String, token: String },
}

/// Resolve Azure credentials from the environment (cached).
fn load_credentials() -> Result<&'static AzureCredentials> {
    static CREDS: OnceLock<Result<AzureCredentials>> = OnceLock::new();
    CREDS
        .get_or_init(resolve_credentials)
        .as_ref()
        .map_err(|e| OxoFlowError::Config {
            message: match e {
                OxoFlowError::Config { message } => message.clone(),
                _ => e.to_string(),
            },
        })
}

fn resolve_credentials() -> Result<AzureCredentials> {
    // Priority 1: explicit SharedKey pair.
    let account = std::env::var("AZURE_STORAGE_ACCOUNT_NAME").ok();
    let key = std::env::var("AZURE_STORAGE_ACCOUNT_KEY").ok();
    if let (Some(account), Some(key)) = (account, key) {
        return Ok(AzureCredentials::SharedKey { account, key });
    }

    // Priority 2: SAS token.
    let account = std::env::var("AZURE_STORAGE_ACCOUNT_NAME").ok();
    let sas = std::env::var("AZURE_STORAGE_SAS_TOKEN").ok();
    if let (Some(account), Some(token)) = (account, sas) {
        // Normalize: strip leading `?` — we add our own query separator.
        let token = token.trim_start_matches('?').to_string();
        return Ok(AzureCredentials::SasToken { account, token });
    }

    // Priority 3: connection string (only if no explicit pair/SAS above).
    if let Ok(conn) = std::env::var("AZURE_STORAGE_CONNECTION_STRING") {
        return parse_connection_string(&conn);
    }

    Err(OxoFlowError::Config {
        message: "Azure Storage credentials not found. Set \
                  AZURE_STORAGE_ACCOUNT_NAME + AZURE_STORAGE_ACCOUNT_KEY (SharedKey), \
                  AZURE_STORAGE_ACCOUNT_NAME + AZURE_STORAGE_SAS_TOKEN (SAS), or \
                  AZURE_STORAGE_CONNECTION_STRING."
            .to_string(),
    })
}

/// Parse an Azure connection string into credentials.
///
/// Supported keys: `AccountName`, `AccountKey`, `SharedAccessSignature`.
fn parse_connection_string(conn: &str) -> Result<AzureCredentials> {
    let mut account: Option<String> = None;
    let mut key: Option<String> = None;
    let mut sas: Option<String> = None;

    for part in conn.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, v)) = part.split_once('=') {
            match k.trim() {
                "AccountName" => account = Some(v.trim().to_string()),
                "AccountKey" => key = Some(v.trim().to_string()),
                "SharedAccessSignature" => sas = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }

    let account = account.ok_or_else(|| OxoFlowError::Config {
        message: "AZURE_STORAGE_CONNECTION_STRING is missing AccountName".to_string(),
    })?;

    if let Some(key) = key {
        return Ok(AzureCredentials::SharedKey { account, key });
    }
    if let Some(token) = sas {
        let token = token.trim_start_matches('?').to_string();
        return Ok(AzureCredentials::SasToken { account, token });
    }

    Err(OxoFlowError::Config {
        message: "AZURE_STORAGE_CONNECTION_STRING must contain AccountKey or \
                  SharedAccessSignature."
            .to_string(),
    })
}

// ---------------------------------------------------------------------------
// Azure Blob REST API helpers
// ---------------------------------------------------------------------------

/// Base URL for the Azure Blob REST API.
fn azure_url(account: &str, container: &str, key: &str, sas: Option<&str>) -> String {
    let encoded_key = urlencode_key(key);
    let mut url = format!("https://{account}.blob.core.windows.net/{container}/{encoded_key}");
    if let Some(token) = sas {
        url.push('?');
        url.push_str(token);
    }
    url
}

/// Minimal URL encoding for object keys (only encode special characters).
fn urlencode_key(key: &str) -> String {
    let mut result = String::with_capacity(key.len());
    for byte in key.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    result
}

/// Compute the Azure SharedKey HMAC-SHA256 signature.
fn azure_signature(account_key: &str, string_to_sign: &str) -> Result<String> {
    // Azure account keys are base64-encoded. Decode before HMAC.
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(account_key)
        .map_err(|e| OxoFlowError::Config {
            message: format!("AZURE_STORAGE_ACCOUNT_KEY is not valid base64: {e}"),
        })?;
    let mut mac = HmacSha256::new_from_slice(&key_bytes).expect("HMAC can accept any key length");
    mac.update(string_to_sign.as_bytes());
    let result = mac.finalize();
    Ok(base64::engine::general_purpose::STANDARD.encode(result.into_bytes()))
}

/// Build the Azure `x-ms-date` header value (RFC 1123, GMT).
fn x_ms_date() -> String {
    chrono::Utc::now()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

/// Build the canonicalized Azure headers string.
///
/// Azure requires all `x-ms-*` headers to be sorted alphabetically, lowercased,
/// and joined as `name:value\n`. We only send `x-ms-date` and `x-ms-version`,
/// so the canonicalized string is fixed.
fn canonicalized_headers(date: &str) -> String {
    format!("x-ms-date:{}\nx-ms-version:{}\n", date, AZURE_API_VERSION)
}

/// Build the canonicalized resource string.
///
/// Format: `/account/container/key` (no trailing slash for objects).
fn canonicalized_resource(account: &str, container: &str, key: &str) -> String {
    format!("/{account}/{container}/{key}")
}

/// The Azure Blob service API version we target.
const AZURE_API_VERSION: &str = "2020-10-02";

/// Azure Blob HTTP client (shared request-level client).
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .expect("failed to create Azure Blob HTTP client")
    })
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

/// Parameters for building an Azure SharedKey authorization header.
struct AuthParams<'a> {
    method: &'a str,
    content_length: &'a str,
    content_md5: &'a str,
    content_type: &'a str,
    date: &'a str,
    account: &'a str,
    container: &'a str,
    key: &'a str,
}

/// Build the `Authorization` header for a SharedKey request.
fn azure_authorization(creds: &AzureCredentials, p: &AuthParams<'_>) -> Result<String> {
    let canonical_headers = canonicalized_headers(p.date);
    let canonical_resource = canonicalized_resource(p.account, p.container, p.key);

    let string_to_sign = format!(
        "{}\n\
         \n\
         \n\
         {}\n\
         {}\n\
         {}\n\
         \n\
         \n\
         \n\
         \n\
         \n\
         \n\
         {canonical_headers}\
         {canonical_resource}",
        p.method, p.content_length, p.content_md5, p.content_type
    );

    match creds {
        AzureCredentials::SharedKey {
            account: cred_account,
            key: account_key,
        } => {
            let signature = azure_signature(account_key, &string_to_sign)?;
            Ok(format!("SharedKey {cred_account}:{signature}"))
        }
        AzureCredentials::SasToken { .. } => {
            // SAS requests do not use the Authorization header — the token
            // rides in the query string instead.
            unreachable!("SAS requests must not call azure_authorization")
        }
    }
}

/// Return the account name for the active credentials.
fn account_name(creds: &AzureCredentials) -> &str {
    match creds {
        AzureCredentials::SharedKey { account, .. } => account,
        AzureCredentials::SasToken { account, .. } => account,
    }
}

/// Return the SAS token if the active credentials use SAS.
fn sas_token(creds: &AzureCredentials) -> Option<&str> {
    match creds {
        AzureCredentials::SharedKey { .. } => None,
        AzureCredentials::SasToken { token, .. } => Some(token),
    }
}

/// Send a GET request and return the live response after the status check.
async fn azure_get_response(
    creds: &AzureCredentials,
    container: &str,
    key: &str,
) -> Result<reqwest::Response> {
    let account = account_name(creds);
    let sas = sas_token(creds);
    let url = azure_url(account, container, key, sas);
    let date = x_ms_date();

    let mut req = http_client().get(&url);
    req = req
        .header("x-ms-date", &date)
        .header("x-ms-version", AZURE_API_VERSION);

    if let AzureCredentials::SharedKey { .. } = creds {
        let auth = azure_authorization(
            creds,
            &AuthParams {
                method: "GET",
                content_length: "",
                content_md5: "",
                content_type: "",
                date: &date,
                account,
                container,
                key,
            },
        )?;
        req = req.header("Authorization", &auth);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| azure_io_error("GET send", container, key, &e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable>"));
        return Err(azure_io_error(
            "GET",
            container,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }
    Ok(resp)
}

/// GET a blob and return the full body as bytes.
async fn azure_get(creds: &AzureCredentials, container: &str, key: &str) -> Result<Vec<u8>> {
    let resp = azure_get_response(creds, container, key).await?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| azure_io_error("GET body", container, key, &e.to_string()))
}

/// GET a byte range of a blob.
async fn azure_get_range(
    creds: &AzureCredentials,
    container: &str,
    key: &str,
    offset: u64,
    end: u64,
) -> Result<Vec<u8>> {
    let account = account_name(creds);
    let sas = sas_token(creds);
    let url = azure_url(account, container, key, sas);
    let date = x_ms_date();
    let range_header = format!("bytes={offset}-{end}");

    let mut req = http_client().get(&url);
    req = req
        .header("x-ms-date", &date)
        .header("x-ms-version", AZURE_API_VERSION)
        .header("Range", &range_header);

    if let AzureCredentials::SharedKey { .. } = creds {
        let auth = azure_authorization(
            creds,
            &AuthParams {
                method: "GET",
                content_length: "",
                content_md5: "",
                content_type: "",
                date: &date,
                account,
                container,
                key,
            },
        )?;
        req = req.header("Authorization", &auth);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| azure_io_error("GET range send", container, key, &e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable>"));
        return Err(azure_io_error(
            "GET range",
            container,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| azure_io_error("GET range body", container, key, &e.to_string()))
}

/// HEAD a blob: return `Ok(true)` when it exists, `Ok(false)` on 404.
async fn azure_head(creds: &AzureCredentials, container: &str, key: &str) -> Result<bool> {
    let stat = azure_head_stat(creds, container, key).await?;
    Ok(stat.is_some())
}

/// HEAD a blob and return its metadata.
async fn azure_head_stat(
    creds: &AzureCredentials,
    container: &str,
    key: &str,
) -> Result<Option<RemoteStat>> {
    let account = account_name(creds);
    let sas = sas_token(creds);
    let url = azure_url(account, container, key, sas);
    let date = x_ms_date();

    let mut req = http_client().head(&url);
    req = req
        .header("x-ms-date", &date)
        .header("x-ms-version", AZURE_API_VERSION);

    if let AzureCredentials::SharedKey { .. } = creds {
        let auth = azure_authorization(
            creds,
            &AuthParams {
                method: "HEAD",
                content_length: "",
                content_md5: "",
                content_type: "",
                date: &date,
                account,
                container,
                key,
            },
        )?;
        req = req.header("Authorization", &auth);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| azure_io_error("HEAD send", container, key, &e.to_string()))?;

    match resp.status() {
        s if s.is_success() => {
            let size: u64 = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            // Azure returns Content-MD5 as raw bytes; we surface the base64
            // form as the etag-equivalent identity (same as GCS md5Hash).
            let etag = resp
                .headers()
                .get("content-md5")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            Ok(Some(RemoteStat { size, etag }))
        }
        s if s == reqwest::StatusCode::NOT_FOUND => Ok(None),
        s => {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable>"));
            Err(azure_io_error(
                "HEAD",
                container,
                key,
                &format!("HTTP {s}: {body}"),
            ))
        }
    }
}

/// PUT a blob with the given body and content type.
async fn azure_put(
    creds: &AzureCredentials,
    container: &str,
    key: &str,
    data: &[u8],
    content_type: &str,
) -> Result<()> {
    let account = account_name(creds);
    let sas = sas_token(creds);
    let url = azure_url(account, container, key, sas);
    let date = x_ms_date();
    let content_length = data.len().to_string();

    let mut req = http_client().put(&url);
    req = req
        .header("x-ms-date", &date)
        .header("x-ms-version", AZURE_API_VERSION)
        .header("Content-Type", content_type)
        .header("Content-Length", &content_length)
        .body(data.to_vec());

    if let AzureCredentials::SharedKey { .. } = creds {
        let auth = azure_authorization(
            creds,
            &AuthParams {
                method: "PUT",
                content_length: &content_length,
                content_md5: "",
                content_type,
                date: &date,
                account,
                container,
                key,
            },
        )?;
        req = req.header("Authorization", &auth);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| azure_io_error("PUT send", container, key, &e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable>"));
        return Err(azure_io_error(
            "PUT",
            container,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }
    Ok(())
}

/// PUT a blob from a local file (streaming).
async fn azure_put_file(
    creds: &AzureCredentials,
    container: &str,
    key: &str,
    path: &Path,
    content_type: &str,
) -> Result<()> {
    let data = tokio::fs::read(path)
        .await
        .map_err(|e| azure_io_error("PUT file read", container, key, &e.to_string()))?;
    azure_put(creds, container, key, &data, content_type).await
}

fn azure_io_error(op: &str, container: &str, key: &str, detail: &str) -> OxoFlowError {
    OxoFlowError::Config {
        message: format!("Azure Blob {op} failed for '{container}/{key}': {detail}"),
    }
}

/// Extract the container name from a StoragePath, erroring when absent.
fn require_azure_container(sp: &StoragePath) -> Result<&str> {
    sp.bucket.as_deref().ok_or_else(|| OxoFlowError::Config {
        message: format!(
            "Azure Blob path '{}' must include a container name (az://container/key)",
            sp.raw
        ),
    })
}

// ---------------------------------------------------------------------------
// AzureBlobStorage struct & StorageBackend impl
// ---------------------------------------------------------------------------

/// Azure Blob Storage backend using SharedKey or SAS token authentication.
///
/// Reads credentials from `AZURE_STORAGE_ACCOUNT_NAME` +
/// `AZURE_STORAGE_ACCOUNT_KEY` (SharedKey), `AZURE_STORAGE_ACCOUNT_NAME` +
/// `AZURE_STORAGE_SAS_TOKEN` (SAS), or `AZURE_STORAGE_CONNECTION_STRING`.
///
/// Uses the Azure Blob REST API directly via `reqwest` — no Azure SDK
/// dependency (issue #1020: dependency-budget posture).
pub struct AzureBlobStorage;

#[async_trait::async_trait]
impl StorageBackend for AzureBlobStorage {
    async fn exists(&self, path: &StoragePath) -> Result<bool> {
        let creds = load_credentials()?;
        let container = require_azure_container(path)?;
        azure_head(creds, container, &path.key).await
    }

    async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>> {
        let creds = load_credentials()?;
        let container = require_azure_container(path)?;
        azure_head_stat(creds, container, &path.key).await
    }

    async fn read_to_string(&self, path: &StoragePath) -> Result<String> {
        let creds = load_credentials()?;
        let container = require_azure_container(path)?;
        let bytes = azure_get(creds, container, &path.key).await?;
        String::from_utf8(bytes).map_err(|e| OxoFlowError::Config {
            message: format!("Azure Blob content is not valid UTF-8: {e}"),
        })
    }

    async fn read_range(&self, path: &StoragePath, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let creds = load_credentials()?;
        let container = require_azure_container(path)?;
        let stat = self.head(path).await?.ok_or_else(|| {
            azure_io_error(
                "read_range head",
                container,
                &path.key,
                "object does not exist",
            )
        })?;
        if offset >= stat.size {
            return Ok(Vec::new());
        }
        let end = (offset + len as u64 - 1).min(stat.size.saturating_sub(1));
        azure_get_range(creds, container, &path.key, offset, end).await
    }

    async fn write(&self, path: &StoragePath, data: &[u8]) -> Result<()> {
        let creds = load_credentials()?;
        let container = require_azure_container(path)?;
        azure_put(
            creds,
            container,
            &path.key,
            data,
            "application/octet-stream",
        )
        .await
    }

    async fn stage(&self, path: &StoragePath, workdir: &Path) -> Result<PathBuf> {
        let creds = load_credentials()?;
        let container = require_azure_container(path)?;
        let dest = crate::storage::staged_path(workdir, path)?;
        let stat = self.head(path).await?.ok_or_else(|| {
            azure_io_error("stage head", container, &path.key, "object does not exist")
        })?;
        // Clone the credential variant into the closure — the variants are
        // cheap to clone (two short Strings or one String + one String).
        let creds_clone: AzureCredentials = creds.clone();
        let container = container.to_string();
        let key = path.key.clone();
        crate::storage::stage_with_cache(stat, &dest, move |mut file| {
            let container = container.clone();
            let key = key.clone();
            async move {
                let mut resp = azure_get_response(&creds_clone, &container, &key).await?;
                while let Some(chunk) = resp
                    .chunk()
                    .await
                    .map_err(|e| azure_io_error("stage read", &container, &key, &e.to_string()))?
                {
                    tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                        .await
                        .map_err(|e| {
                            azure_io_error("stage write", &container, &key, &e.to_string())
                        })?;
                }
                Ok(())
            }
        })
        .await?;
        Ok(dest)
    }

    async fn upload(&self, local: &Path, remote: &StoragePath) -> Result<()> {
        let creds = load_credentials()?;
        let container = require_azure_container(remote)?;
        azure_put_file(
            creds,
            container,
            &remote.key,
            local,
            "application/octet-stream",
        )
        .await
    }

    fn name(&self) -> &'static str {
        "azure"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_key_preserves_safe_chars() {
        assert_eq!(urlencode_key("data/file.txt"), "data/file.txt");
        assert_eq!(urlencode_key("a b.txt"), "a%20b.txt");
        assert_eq!(urlencode_key("x+y"), "x%2By");
    }

    #[test]
    fn azure_url_formats_correctly() {
        let url = azure_url("myacct", "mycontainer", "data/file.txt", None);
        assert_eq!(
            url,
            "https://myacct.blob.core.windows.net/mycontainer/data/file.txt"
        );
    }

    #[test]
    fn azure_url_appends_sas_token() {
        let url = azure_url("myacct", "mycontainer", "file.txt", Some("sig=abc&se=123"));
        assert!(
            url.contains("?sig=abc&se=123"),
            "SAS token in query string: {url}"
        );
    }

    #[test]
    fn parse_connection_string_shared_key() {
        let creds = parse_connection_string(
            "DefaultEndpointsProtocol=https;AccountName=myacct;AccountKey=aGVsbG8=;EndpointSuffix=core.windows.net",
        )
        .unwrap();
        match creds {
            AzureCredentials::SharedKey { account, key } => {
                assert_eq!(account, "myacct");
                assert_eq!(key, "aGVsbG8=");
            }
            _ => panic!("expected SharedKey"),
        }
    }

    #[test]
    fn parse_connection_string_sas() {
        let creds = parse_connection_string(
            "AccountName=myacct;SharedAccessSignature=sig%3Dabc%26se%3D123",
        )
        .unwrap();
        match creds {
            AzureCredentials::SasToken { account, token } => {
                assert_eq!(account, "myacct");
                assert_eq!(token, "sig%3Dabc%26se%3D123");
            }
            _ => panic!("expected SasToken"),
        }
    }

    #[test]
    fn parse_connection_string_missing_account_errors() {
        let result = parse_connection_string("AccountKey=aGVsbG8=");
        assert!(result.is_err());
    }

    #[test]
    fn canonicalized_resource_format() {
        assert_eq!(
            canonicalized_resource("myacct", "mycontainer", "file.txt"),
            "/myacct/mycontainer/file.txt"
        );
    }

    #[test]
    fn azure_signature_produces_base64() {
        // A known key + string-to-sign pair should produce a deterministic
        // base64 signature. We verify the format is base64, not the exact
        // value (that would test the HMAC library, not our code).
        let key = base64::engine::general_purpose::STANDARD.encode(b"test-key");
        let sig = azure_signature(&key, "GET\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n").unwrap();
        // Base64 decode should succeed and produce 32 bytes (SHA-256).
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&sig)
            .unwrap();
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn require_azure_container_errors_on_missing_bucket() {
        let sp = StoragePath::parse("az://");
        assert!(require_azure_container(&sp).is_err());
    }

    #[test]
    fn azure_storage_backend_name() {
        let backend = AzureBlobStorage;
        assert_eq!(backend.name(), "azure");
    }
}
