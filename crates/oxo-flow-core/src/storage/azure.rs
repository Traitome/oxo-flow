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
/// Azure requires all `x-ms-*` headers to be sorted alphabetically,
/// lowercased, and joined as `name:value\n`. We always send `x-ms-date` and
/// `x-ms-version`; `x_ms_headers` carries any additional `x-ms-*` headers
/// (e.g. `x-ms-blob-content-md5` on Put Block List).
fn canonicalized_headers(date: &str, extra: &[(&str, &str)]) -> String {
    let mut headers = vec![
        ("x-ms-date".to_string(), date.to_string()),
        ("x-ms-version".to_string(), AZURE_API_VERSION.to_string()),
    ];
    for (name, value) in extra {
        headers.push((name.to_ascii_lowercase(), (*value).to_string()));
    }
    headers.sort();
    let mut result = String::new();
    for (name, value) in &headers {
        result.push_str(name);
        result.push(':');
        result.push_str(value);
        result.push('\n');
    }
    result
}

/// Build the canonicalized resource string.
///
/// Format: `/account/container/key` (no trailing slash for objects).
fn canonicalized_resource(account: &str, container: &str, key: &str) -> String {
    format!("/{account}/{container}/{key}")
}

/// Build the canonicalized resource string including query parameters.
///
/// SharedKey canonicalization appends every query parameter as
/// `name:value`, each preceded by `\n`, with names lowercased, all pairs
/// sorted lexicographically by name, and names/values URL-decoded
/// (`?comp=block&blockid=AQ%3D%3D` yields `\nblockid:AQ==\ncomp:block`).
fn canonicalized_resource_with_query(
    account: &str,
    container: &str,
    key: &str,
    query: &[(&str, &str)],
) -> String {
    let mut resource = canonicalized_resource(account, container, key);
    let mut params: Vec<(String, String)> = query
        .iter()
        .map(|(name, value)| (urldecode(&name.to_ascii_lowercase()), urldecode(value)))
        .collect();
    params.sort();
    for (name, value) in &params {
        resource.push('\n');
        resource.push_str(name);
        resource.push(':');
        resource.push_str(value);
    }
    resource
}

/// Percent-decode a query-parameter component; malformed escapes pass
/// through untouched rather than erroring (values are ours or Azure's).
fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Append query parameters to a blob URL.
///
/// SAS tokens ride in the query string, so `url` may already carry a query —
/// additional parameters are joined with `&`. Values are percent-encoded
/// (`+` and `=` in base64 block ids become `%2B`/`%3D`).
fn url_with_query(url: &str, query: &[(&str, &str)]) -> String {
    let mut result = url.to_string();
    for (name, value) in query {
        result.push_str(if result.contains('?') { "&" } else { "?" });
        result.push_str(&urlencode_key(name));
        result.push('=');
        result.push_str(&urlencode_key(value));
    }
    result
}

/// Fixed-width base64 block id for a 0-based block index.
///
/// Azure requires every block id of a blob to share one length (≤64 bytes
/// pre-encoding). An 8-byte big-endian index always encodes to 12 ASCII
/// characters, so ids compare uniformly and the commit order is explicit.
fn block_id(index: u64) -> String {
    base64::engine::general_purpose::STANDARD.encode(index.to_be_bytes())
}

/// Build the Put Block List XML body (`<Latest>` = use the block's content).
fn block_list_xml(block_ids: &[String]) -> String {
    // Base64 block ids carry no XML-special characters — no escaping needed.
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList>");
    for id in block_ids {
        xml.push_str("<Latest>");
        xml.push_str(id);
        xml.push_str("</Latest>");
    }
    xml.push_str("</BlockList>");
    xml
}

/// The Azure Blob service API version we target.
const AZURE_API_VERSION: &str = "2020-10-02";

/// Bytes per Put Block when streaming uploads. Well under the 4,000 MiB
/// per-block service limit (API 2019-12-12+); peak upload memory is one
/// buffer of this size regardless of file size.
const AZURE_BLOCK_SIZE: usize = 8 * 1024 * 1024;

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
    /// `Range` header value (empty string when the request sends none) —
    /// Azure signs the Range component only when the header is present.
    range: &'a str,
    /// Canonicalized x-ms headers beyond `x-ms-date`/`x-ms-version`
    /// (e.g. `x-ms-blob-content-type` on Put Block List).
    extra_headers: &'a [(&'a str, &'a str)],
    /// Query parameters (already percent-encoded values) that ride on the
    /// request URL and must be canonicalized into the resource string.
    query: &'a [(&'a str, &'a str)],
    date: &'a str,
    account: &'a str,
    container: &'a str,
    key: &'a str,
}

/// Build the `Authorization` header for a SharedKey request.
fn azure_authorization(creds: &AzureCredentials, p: &AuthParams<'_>) -> Result<String> {
    let canonical_headers = canonicalized_headers(p.date, p.extra_headers);
    let canonical_resource =
        canonicalized_resource_with_query(p.account, p.container, p.key, p.query);

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
         {}\n\
         {canonical_headers}\
         {canonical_resource}",
        p.method, p.content_length, p.content_md5, p.content_type, p.range
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
                range: "",
                extra_headers: &[],
                query: &[],
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
                range: &range_header,
                extra_headers: &[],
                query: &[],
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
                range: "",
                extra_headers: &[],
                query: &[],
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
    // SharedKey signs the Content-Length component as the empty string when
    // no content is sent (2015-02-21+ contract); only a non-empty body sends
    // (and signs) an explicit Content-Length header.
    let content_length = if data.is_empty() {
        ""
    } else {
        &*data.len().to_string()
    };

    let mut req = http_client().put(&url);
    req = req
        .header("x-ms-date", &date)
        .header("x-ms-version", AZURE_API_VERSION)
        .header("Content-Type", content_type);
    if !data.is_empty() {
        req = req.header("Content-Length", data.len()).body(data.to_vec());
    }

    if let AzureCredentials::SharedKey { .. } = creds {
        let auth = azure_authorization(
            creds,
            &AuthParams {
                method: "PUT",
                content_length,
                content_md5: "",
                content_type,
                range: "",
                extra_headers: &[],
                query: &[],
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

/// PUT a blob from a local file, streaming in fixed-size blocks.
///
/// The file is read in [`AZURE_BLOCK_SIZE`] chunks uploaded as Put Block
/// requests, then committed atomically with Put Block List — peak memory is
/// one buffer regardless of file size, and the 256 MiB single-PUT service
/// ceiling never applies. Empty files commit an empty block list.
async fn azure_put_file(
    creds: &AzureCredentials,
    container: &str,
    key: &str,
    path: &Path,
    content_type: &str,
) -> Result<()> {
    let account = account_name(creds);
    let sas = sas_token(creds);
    let url = azure_url(account, container, key, sas);

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| azure_io_error("PUT file open", container, key, &e.to_string()))?;

    // One reused buffer: reads may return short counts, so a block is only
    // complete once `filled == AZURE_BLOCK_SIZE` (or EOF leaves a tail).
    let mut block_buf = vec![0u8; AZURE_BLOCK_SIZE];
    let mut block_ids: Vec<String> = Vec::new();
    let mut filled = 0usize;
    let mut index: u64 = 0;
    loop {
        if filled == block_buf.len() {
            let id = block_id(index);
            put_block(creds, &url, container, key, &id, &block_buf).await?;
            block_ids.push(id);
            index += 1;
            filled = 0;
        }
        let n = tokio::io::AsyncReadExt::read(&mut file, &mut block_buf[filled..])
            .await
            .map_err(|e| azure_io_error("PUT file read", container, key, &e.to_string()))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    if filled > 0 {
        let id = block_id(index);
        put_block(creds, &url, container, key, &id, &block_buf[..filled]).await?;
        block_ids.push(id);
    }

    put_block_list(creds, &url, container, key, &block_ids, content_type).await
}

/// Upload one block of a streaming upload (Put Block: `?comp=block&blockid=…`).
async fn put_block(
    creds: &AzureCredentials,
    url: &str,
    container: &str,
    key: &str,
    id: &str,
    data: &[u8],
) -> Result<()> {
    let block_url = url_with_query(url, &[("blockid", id), ("comp", "block")]);
    let date = x_ms_date();
    let content_length = data.len().to_string();

    let mut req = http_client()
        .put(&block_url)
        .header("x-ms-date", &date)
        .header("x-ms-version", AZURE_API_VERSION)
        .header("Content-Length", data.len())
        .body(data.to_vec());

    if let AzureCredentials::SharedKey { .. } = creds {
        let auth = azure_authorization(
            creds,
            &AuthParams {
                method: "PUT",
                content_length: &content_length,
                content_md5: "",
                content_type: "",
                range: "",
                extra_headers: &[],
                query: &[("blockid", id), ("comp", "block")],
                date: &date,
                account: account_name(creds),
                container,
                key,
            },
        )?;
        req = req.header("Authorization", &auth);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| azure_io_error("PUT block send", container, key, &e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable>"));
        return Err(azure_io_error(
            "PUT block",
            container,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }
    Ok(())
}

/// Commit staged blocks as the blob content (Put Block List: `?comp=blocklist`).
///
/// `content_type` rides on `x-ms-blob-content-type`, which sets the committed
/// blob's Content-Type and participates in the canonicalized-headers signature.
async fn put_block_list(
    creds: &AzureCredentials,
    url: &str,
    container: &str,
    key: &str,
    block_ids: &[String],
    content_type: &str,
) -> Result<()> {
    let list_url = url_with_query(url, &[("comp", "blocklist")]);
    let date = x_ms_date();
    let body = block_list_xml(block_ids);
    let content_length = body.len().to_string();

    let mut req = http_client()
        .put(&list_url)
        .header("x-ms-date", &date)
        .header("x-ms-version", AZURE_API_VERSION)
        .header("Content-Type", "application/xml")
        .header("x-ms-blob-content-type", content_type)
        .header("Content-Length", body.len())
        .body(body);

    if let AzureCredentials::SharedKey { .. } = creds {
        let auth = azure_authorization(
            creds,
            &AuthParams {
                method: "PUT",
                content_length: &content_length,
                content_md5: "",
                content_type: "application/xml",
                range: "",
                extra_headers: &[("x-ms-blob-content-type", content_type)],
                query: &[("comp", "blocklist")],
                date: &date,
                account: account_name(creds),
                container,
                key,
            },
        )?;
        req = req.header("Authorization", &auth);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| azure_io_error("PUT block list send", container, key, &e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable>"));
        return Err(azure_io_error(
            "PUT block list",
            container,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }
    Ok(())
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

    // ---- Golden-vector signature tests -------------------------------------
    //
    // These pin the exact SharedKey string-to-sign each request builder
    // produces: fixed key (base64 of 32 zero bytes), fixed date, account
    // `acct`, container `container`, key `k`. Any drift in canonicalization
    // (header ordering, query decoding, empty-body Content-Length handling,
    // extra-header inclusion) flips the signature and fails here.

    fn golden_creds() -> AzureCredentials {
        AzureCredentials::SharedKey {
            account: "acct".to_string(),
            key: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
        }
    }

    #[test]
    fn sharedkey_signature_empty_put() {
        // azure_put with an empty body signs no Content-Length component at
        // all (2015-02-21+ contract) while Content-Type stays signed.
        let sig = azure_authorization(
            &golden_creds(),
            &AuthParams {
                method: "PUT",
                content_length: "",
                content_md5: "",
                content_type: "application/octet-stream",
                range: "",
                extra_headers: &[],
                query: &[],
                date: "Mon, 02 Nov 2026 00:00:00 GMT",
                account: "acct",
                container: "container",
                key: "k",
            },
        )
        .unwrap();
        assert_eq!(
            sig,
            "SharedKey acct:JjSoiI3O64sBWVJt8jKMnJz8rGlDwg/4W8mhb0wOLoA="
        );
    }

    #[test]
    fn sharedkey_signature_range_get() {
        // azure_get_range signs the Range header as the 12th component.
        let sig = azure_authorization(
            &golden_creds(),
            &AuthParams {
                method: "GET",
                content_length: "",
                content_md5: "",
                content_type: "",
                range: "bytes=0-99",
                extra_headers: &[],
                query: &[],
                date: "Mon, 02 Nov 2026 00:00:00 GMT",
                account: "acct",
                container: "container",
                key: "k",
            },
        )
        .unwrap();
        assert_eq!(
            sig,
            "SharedKey acct:2QgDGHCSPM/fCJ25Eso728oUxL5DoBS8eOKz1DS88V8="
        );
    }

    #[test]
    fn sharedkey_signature_put_block() {
        // Put Block signs the body length, an empty Content-Type, and both
        // query params URL-decoded into the canonical resource.
        let sig = azure_authorization(
            &golden_creds(),
            &AuthParams {
                method: "PUT",
                content_length: "3",
                content_md5: "",
                content_type: "",
                range: "",
                extra_headers: &[],
                query: &[("blockid", "AAAAAAAAAAE="), ("comp", "block")],
                date: "Mon, 02 Nov 2026 00:00:00 GMT",
                account: "acct",
                container: "container",
                key: "k",
            },
        )
        .unwrap();
        assert_eq!(
            sig,
            "SharedKey acct:q4uK0wChjfS6SROYYMw8wrBDoIZY9VphcenZBlTwbms="
        );
    }

    #[test]
    fn sharedkey_signature_put_block_list() {
        // Put Block List signs the XML body length, the XML content type,
        // and the extra x-ms-blob-content-type header (sorted before
        // x-ms-date), with `comp=blocklist` in the canonical resource.
        let sig = azure_authorization(
            &golden_creds(),
            &AuthParams {
                method: "PUT",
                content_length: "90",
                content_md5: "",
                content_type: "application/xml",
                range: "",
                extra_headers: &[("x-ms-blob-content-type", "text/plain")],
                query: &[("comp", "blocklist")],
                date: "Mon, 02 Nov 2026 00:00:00 GMT",
                account: "acct",
                container: "container",
                key: "k",
            },
        )
        .unwrap();
        assert_eq!(
            sig,
            "SharedKey acct:PmuSXq9+BeqBmZvSPD07ePhjq6Y5dqrkLO0xP6fvSJU="
        );
    }

    #[test]
    fn block_id_is_fixed_width_base64() {
        assert_eq!(block_id(0), "AAAAAAAAAAA=");
        assert_eq!(block_id(1), "AAAAAAAAAAE=");
        assert_eq!(block_id(u64::MAX).len(), 12);
    }

    #[test]
    fn block_list_xml_single_block_shape() {
        let xml = block_list_xml(&[block_id(1)]);
        assert_eq!(
            xml,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <BlockList><Latest>AAAAAAAAAAE=</Latest></BlockList>"
        );
        assert_eq!(xml.len(), 90);
    }

    #[test]
    fn canonicalized_resource_with_query_decodes_and_sorts() {
        let resource = canonicalized_resource_with_query(
            "acct",
            "container",
            "k",
            &[("comp", "block"), ("blockid", "AQ%3D%3D")],
        );
        assert_eq!(resource, "/acct/container/k\nblockid:AQ==\ncomp:block");
    }

    #[test]
    fn url_with_query_encodes_values() {
        assert_eq!(
            url_with_query(
                "https://acct.blob.core.windows.net/c/k",
                &[("comp", "block")]
            ),
            "https://acct.blob.core.windows.net/c/k?comp=block"
        );
        assert_eq!(
            url_with_query(
                "https://acct.blob.core.windows.net/c/k?comp=block",
                &[("blockid", "AAAAAAAAAAE=")]
            ),
            "https://acct.blob.core.windows.net/c/k?comp=block&blockid=AAAAAAAAAAE%3D"
        );
    }
}
