//! Read-only file layer for run results + user input uploads (issue #82
//! P0-1 / P0-2).
//!
//! - `GET /api/runs/{id}/files?path=` — download (Range/ETag), text preview,
//!   or a streaming STORE-mode zip of a directory. Paths are strictly
//!   sandboxed to the run workdir; sensitive filenames (.env, keys, …) are
//!   never served.
//! - `POST /api/files` — multipart upload into the user's inputs workspace,
//!   chunked to disk with a per-file size cap.
//! - `GET /api/files` — list the user's uploaded inputs.

use axum::body::Body;
use axum::extract::{Multipart, Path, Query};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use std::io::Read as _;
use std::path::{Path as FsPath, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::workflow::handlers::ApiError;

fn err(status: StatusCode, code: &str, msg: String) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: msg,
            detail: None,
            suggestion: None,
        }),
    )
}

type ApiErrorRes = (StatusCode, Json<ApiError>);

/// Per-file upload cap. 8 GiB covers the largest realistic single upload
/// (paired-end fastqs, archives); total-request abuse is bounded by the
/// global rate limiter.
const MAX_UPLOAD_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Zip archive bounds: entry count and total size.
const MAX_ZIP_ENTRIES: usize = 4096;
const MAX_ZIP_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Text preview truncation.
const PREVIEW_MAX_BYTES: usize = 100 * 1024;

/// Filenames that must never be downloadable (secrets often end up in
/// workdirs next to results).
fn is_blocked_name(name: &str) -> bool {
    const BLOCKED: &[&str] = &[
        ".env",
        ".key",
        ".pem",
        ".p12",
        ".pfx",
        ".keystore",
        ".secret",
        ".token",
        ".credentials",
        "id_rsa",
        "id_ed25519",
        "authorized_keys",
        "kubeconfig",
        "web.config",
    ];
    BLOCKED.contains(&name.to_lowercase().as_str())
}

/// Extension → MIME. Unknown extensions serve as octet-stream; common
/// bioinformatics text formats get text/* so previews and browsers handle
/// them sensibly.
fn mime_for(path: &FsPath) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "txt" | "log" | "out" | "err" | "sam" | "vcf" | "bed" | "gtf" | "gff" | "fasta" | "fa"
        | "fastq" | "fq" | "bai" | "dict" => "text/plain",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        "gz" => "application/gzip",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

fn is_previewable(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/toml" | "application/yaml"
        )
}

/// Join a relative path onto `base`, rejecting traversal (`..`, absolute
/// paths). A canonicalized prefix check is the second line of defense
/// against symlink tricks.
fn resolve_rel(base: &FsPath, rel: &str) -> Result<PathBuf, ApiErrorRes> {
    if rel.is_empty() || rel.starts_with('/') {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_PATH",
            "path must be a non-empty relative path".into(),
        ));
    }
    let mut out = base.to_path_buf();
    for comp in rel.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "INVALID_PATH",
                "path must not escape the run directory".into(),
            ));
        }
        out.push(comp);
    }
    let canonical_base = base.canonicalize().map_err(|_| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Run workdir missing".into(),
        )
    })?;
    let target = out
        .canonicalize()
        .map_err(|_| err(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found".into()))?;
    if !target.starts_with(&canonical_base) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_PATH",
            "path must not escape the run directory".into(),
        ));
    }
    Ok(target)
}

/// Parse a single `Range: bytes=a-b` header. `Err(true)` = unsatisfiable
/// (416), `Err(false)` = malformed/multi-range (serve the full body, which
/// RFC 9110 §14.2 explicitly permits).
fn parse_single_range(spec: &str, size: u64) -> Result<(u64, u64), bool> {
    let Some(rest) = spec.strip_prefix("bytes=") else {
        return Err(false);
    };
    if rest.contains(',') {
        return Err(false);
    }
    let (a, b) = rest.split_once('-').ok_or(false)?;
    if a.is_empty() {
        let n: u64 = b.parse().map_err(|_| false)?;
        if n == 0 {
            return Err(true);
        }
        let start = size.saturating_sub(n);
        return Ok((start, size - 1));
    }
    let start: u64 = a.parse().map_err(|_| false)?;
    let end: u64 = if b.is_empty() {
        size - 1
    } else {
        b.parse::<u64>().map_err(|_| false)?.min(size - 1)
    };
    if start > end || start >= size {
        return Err(true);
    }
    Ok((start, end))
}

fn etag(path: &FsPath, size: u64) -> String {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("\"{mtime:x}-{size:x}\"")
}

/// Serve one file: preview JSON, or bytes with Range/ETag/disposition.
async fn serve_file(path: &FsPath, preview: bool, range: Option<String>) -> Response {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => {
            return err(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found".into())
                .into_response();
        }
    };
    let size = meta.len();
    let mime = mime_for(path);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();

    // Preview: JSON for text, inline bytes for images, 415 for the rest.
    if preview {
        if is_previewable(mime) {
            let bytes = match tokio::fs::read(path).await {
                Ok(b) => b,
                Err(e) => {
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "READ_ERROR",
                        format!("Failed to read file: {e}"),
                    )
                    .into_response();
                }
            };
            let truncated = bytes.len() > PREVIEW_MAX_BYTES;
            let content = String::from_utf8_lossy(&bytes[..bytes.len().min(PREVIEW_MAX_BYTES)]);
            return Json(serde_json::json!({
                "name": name,
                "size_bytes": size,
                "mime": mime,
                "truncated": truncated,
                "content": content,
            }))
            .into_response();
        }
        if mime.starts_with("image/") {
            let data = match tokio::fs::read(path).await {
                Ok(d) => d,
                Err(e) => {
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "READ_ERROR",
                        format!("Failed to read image: {e}"),
                    )
                    .into_response();
                }
            };
            let mut headers = HeaderMap::new();
            headers.insert("content-type", HeaderValue::from_static(mime));
            headers.insert(
                "cache-control",
                HeaderValue::from_static("private, max-age=300"),
            );
            return (StatusCode::OK, headers, data).into_response();
        }
        return err(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "NO_PREVIEW",
            format!("No preview for {mime} — download the file instead"),
        )
        .into_response();
    }

    let tag = etag(path, size);
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static(mime));
    headers.insert("accept-ranges", HeaderValue::from_static("bytes"));
    headers.insert(
        "etag",
        HeaderValue::from_str(&tag).unwrap_or(HeaderValue::from_static("\"\"")),
    );
    headers.insert(
        "content-disposition",
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .unwrap_or(HeaderValue::from_static("attachment")),
    );
    headers.insert(
        "cache-control",
        HeaderValue::from_static("private, max-age=300"),
    );

    // Range support: exactly one range is served; multi-range requests
    // degrade to the full body (RFC 9110 §14.2).
    if size > 0
        && let Some(spec) = range.as_deref()
        && let Ok((start, end)) = parse_single_range(spec, size)
    {
        let len = end - start + 1;
        let Ok(mut f) = tokio::fs::File::open(path).await else {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "READ_ERROR",
                "Failed to open file".into(),
            )
            .into_response();
        };
        if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "READ_ERROR",
                "Failed to seek file".into(),
            )
            .into_response();
        }
        headers.insert(
            "content-range",
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).unwrap(),
        );
        headers.insert("content-length", HeaderValue::from(len));
        let stream = tokio_util::io::ReaderStream::with_capacity(f.take(len), 64 * 1024);
        return (
            StatusCode::PARTIAL_CONTENT,
            headers,
            Body::from_stream(stream),
        )
            .into_response();
    }
    if size > 0
        && let Some(spec) = range.as_deref()
        && parse_single_range(spec, size) == Err(true)
    {
        return unsat_range(size);
    }

    let Ok(f) = tokio::fs::File::open(path).await else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "READ_ERROR",
            "Failed to open file".into(),
        )
        .into_response();
    };
    headers.insert("content-length", HeaderValue::from(size));
    let stream = tokio_util::io::ReaderStream::with_capacity(f, 64 * 1024);
    (StatusCode::OK, headers, Body::from_stream(stream)).into_response()
}

/// Range-unsatisfiable responses need the spec's `Content-Range: bytes */N`.
fn unsat_range(size: u64) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-range",
        HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
    );
    headers.insert("accept-ranges", HeaderValue::from_static("bytes"));
    (StatusCode::RANGE_NOT_SATISFIABLE, headers, "").into_response()
}

/// One entry in the zip archive.
struct ZipEntry {
    rel: String,
    size: u64,
    crc: u32,
    data: PathBuf,
}

/// Collect directory entries for the zip, computing CRCs upfront so headers
/// can precede data in the STORE stream.
fn collect_zip_entries(dir: &FsPath) -> Result<Vec<ZipEntry>, String> {
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let rd = std::fs::read_dir(&current).map_err(|e| e.to_string())?;
        for item in rd {
            let item = item.map_err(|e| e.to_string())?;
            let path = item.path();
            let meta = item.metadata().map_err(|e| e.to_string())?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if is_blocked_name(path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
                continue;
            }
            total += meta.len();
            if total > MAX_ZIP_TOTAL_BYTES {
                return Err(format!("Directory exceeds {MAX_ZIP_TOTAL_BYTES} bytes"));
            }
            entries.push(ZipEntry {
                rel: path
                    .strip_prefix(dir)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/"),
                size: meta.len(),
                crc: crc32(&path, meta.len()).ok_or("Failed to compute CRC")?,
                data: path,
            });
            if entries.len() > MAX_ZIP_ENTRIES {
                return Err(format!("Directory has more than {MAX_ZIP_ENTRIES} files"));
            }
        }
    }
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(entries)
}

fn crc32(path: &FsPath, size: u64) -> Option<u32> {
    // crc32fast updates from chunks; large files get read once here and
    // once for the zip body — bounded by MAX_ZIP_TOTAL_BYTES.
    let mut reader = std::fs::File::open(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    let mut remaining = size;
    let mut buf = vec![0u8; 256 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..want]).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Some(hasher.finalize())
}

/// Streaming STORE-mode zip: local headers + file bytes + central
/// directory + EOCD, written into a channel that feeds the HTTP body
/// directly — no temporary archive on disk.
fn zip_body_stream(entries: Vec<ZipEntry>) -> Body {
    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<Vec<u8>>>(8);
    tokio::spawn(async move {
        let _ = zip_into_channel(entries, &tx).await;
    });
    Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
}

async fn zip_into_channel(
    entries: Vec<ZipEntry>,
    tx: &tokio::sync::mpsc::Sender<std::io::Result<Vec<u8>>>,
) -> std::io::Result<()> {
    let mut offset: u64 = 0;
    let mut central: Vec<u8> = Vec::new();
    for entry in &entries {
        let name = entry.rel.as_bytes();
        // Local file header (STORE, UTF-8 names).
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(&0x04034b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes()); // version needed
        header.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
        header.extend_from_slice(&0u16.to_le_bytes()); // method: store
        header.extend_from_slice(&0u32.to_le_bytes()); // mod time/date
        header.extend_from_slice(&entry.crc.to_le_bytes());
        header.extend_from_slice(&(entry.size as u32).to_le_bytes());
        header.extend_from_slice(&(entry.size as u32).to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra len
        header.extend_from_slice(name);

        // Central directory record.
        let mut cd = Vec::with_capacity(46 + name.len());
        cd.extend_from_slice(&0x02014b50u32.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes()); // version made by
        cd.extend_from_slice(&20u16.to_le_bytes()); // version needed
        cd.extend_from_slice(&0x0800u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&entry.crc.to_le_bytes());
        cd.extend_from_slice(&(entry.size as u32).to_le_bytes());
        cd.extend_from_slice(&(entry.size as u32).to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        cd.extend_from_slice(&(offset as u32).to_le_bytes());
        cd.extend_from_slice(name);
        central.extend_from_slice(&cd);

        offset += header.len() as u64 + entry.size;
        tx.send(Ok(header))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client gone"))?;

        // Stream the file data in 256 KiB chunks.
        let mut file = tokio::fs::File::open(&entry.data).await?;
        let mut remaining = entry.size;
        let mut buf = vec![0u8; 256 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = file.read(&mut buf[..want]).await?;
            if n == 0 {
                break;
            }
            tx.send(Ok(buf[..n].to_vec()))
                .await
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client gone"))?;
            remaining -= n as u64;
        }
    }

    // End of central directory record.
    let cd_offset = offset;
    let mut eocd = Vec::with_capacity(22);
    eocd.extend_from_slice(&0x06054b50u32.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&(central.len() as u32).to_le_bytes());
    eocd.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    tx.send(Ok(central)).await.ok();
    tx.send(Ok(eocd)).await.ok();
    Ok(())
}

#[derive(Deserialize)]
pub struct FileQuery {
    pub path: String,
    #[serde(default)]
    pub preview: Option<bool>,
}

/// GET /api/runs/{id}/files — download / preview / zip a run's products.
pub async fn get_run_file(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let user = resolve(authenticated.as_ref());
    let pool = match crate::infra::db::sqlite::try_pool() {
        Ok(p) => p,
        Err(_) => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "DB_ERROR",
                "Database not available".into(),
            )
            .into_response();
        }
    };
    let run = match super::handlers::load_owned_run(pool, &user, &id).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let workdir = match run.workdir.as_deref() {
        Some(wd) if !wd.is_empty() => PathBuf::from(wd),
        _ => {
            return err(
                StatusCode::NOT_FOUND,
                "NO_WORKDIR",
                "Run has no workdir".into(),
            )
            .into_response();
        }
    };
    let target = match resolve_rel(&workdir, &q.path) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    if is_blocked_name(&name) {
        return err(
            StatusCode::FORBIDDEN,
            "BLOCKED",
            "This file type is never served".into(),
        )
        .into_response();
    }
    if target.is_dir() {
        match collect_zip_entries(&target) {
            Ok(entries) => {
                let dir_name = if q.path == "." {
                    workdir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("run")
                        .to_string()
                } else {
                    name
                };
                let mut headers = HeaderMap::new();
                headers.insert("content-type", HeaderValue::from_static("application/zip"));
                headers.insert(
                    "content-disposition",
                    HeaderValue::from_str(&format!("attachment; filename=\"{dir_name}.zip\""))
                        .unwrap(),
                );
                headers.insert(
                    "cache-control",
                    HeaderValue::from_static("private, max-age=60"),
                );
                return (StatusCode::OK, headers, zip_body_stream(entries)).into_response();
            }
            Err(e) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "ZIP_ERROR",
                    format!("Cannot archive directory: {e}"),
                )
                .into_response();
            }
        }
    }
    let range = headers
        .get("range")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    serve_file(&target, q.preview.unwrap_or(false), range).await
}

/// POST /api/files — multipart upload into the acting user's inputs
/// workspace (`workspace/users/{user}/inputs/...`).
pub async fn upload_files(
    authenticated: Option<Extension<CurrentUser>>,
    mut multipart: Multipart,
) -> Response {
    let user = resolve(authenticated.as_ref());
    let mut subdir = String::new();
    let mut saved: Vec<serde_json::Value> = Vec::new();

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        if field_name == "path" {
            if let Ok(value) = field.text().await {
                subdir = value.trim().trim_matches('/').to_string();
                if subdir
                    .split('/')
                    .any(|c| c.is_empty() || c == ".." || c.contains('\\'))
                {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "INVALID_PATH",
                        "path must be a clean relative directory".into(),
                    )
                    .into_response();
                }
            }
            continue;
        }
        // File field: basename only, no traversal.
        let Some(raw_name) = field.file_name().map(String::from) else {
            continue;
        };
        let name = FsPath::new(&raw_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() || name.contains('/') || name.contains('\\') || name == ".." {
            return err(
                StatusCode::BAD_REQUEST,
                "INVALID_NAME",
                format!("Rejected filename '{raw_name}'"),
            )
            .into_response();
        }

        let mut dest = crate::workspace::inputs_directory(&user.id);
        if !subdir.is_empty() {
            dest = dest.join(&subdir);
        }
        dest = dest.join(&name);
        if let Some(parent) = dest.parent()
            && tokio::fs::create_dir_all(parent).await.is_err()
        {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "IO_ERROR",
                "Failed to create upload directory".into(),
            )
            .into_response();
        }

        // Chunked write: large fastqs must never be buffered in memory.
        let mut written: u64 = 0;
        let mut file = match tokio::fs::File::create(&dest).await {
            Ok(f) => f,
            Err(e) => {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "IO_ERROR",
                    format!("Failed to create file: {e}"),
                )
                .into_response();
            }
        };
        let mut over = false;
        while let Ok(Some(chunk)) = field.chunk().await {
            written += chunk.len() as u64;
            if written > MAX_UPLOAD_FILE_BYTES {
                over = true;
                break;
            }
            if file.write_all(&chunk).await.is_err() {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "IO_ERROR",
                    "Failed to write upload".into(),
                )
                .into_response();
            }
        }
        let _ = file.flush().await;
        drop(file);
        if over {
            let _ = tokio::fs::remove_file(&dest).await;
            return err(
                StatusCode::PAYLOAD_TOO_LARGE,
                "FILE_TOO_LARGE",
                format!("File exceeds the {MAX_UPLOAD_FILE_BYTES}-byte upload cap"),
            )
            .into_response();
        }
        let rel = if subdir.is_empty() {
            name.clone()
        } else {
            format!("{subdir}/{name}")
        };
        saved.push(serde_json::json!({
            "name": name,
            "path": rel,
            "size_bytes": written,
        }));
    }

    Json(serde_json::json!({"files": saved})).into_response()
}

/// GET /api/files — list the acting user's uploaded inputs (recursive,
/// capped at 1000 entries).
pub async fn list_uploaded_files(authenticated: Option<Extension<CurrentUser>>) -> Response {
    let user = resolve(authenticated.as_ref());
    let root = crate::workspace::inputs_directory(&user.id);
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in rd.flatten() {
            let path = item.path();
            let Ok(meta) = item.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            out.push(serde_json::json!({
                "name": path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "path": path.strip_prefix(&root).unwrap_or(&path).to_string_lossy(),
                "size_bytes": meta.len(),
            }));
            if out.len() >= 1000 {
                break;
            }
        }
    }
    out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Json(out).into_response()
}
