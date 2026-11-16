//! Supabase Storage (references/storage-api) at /storage/v1, on the local
//! disk: objects are rows of `storage.objects`, written and read as the
//! caller so Nuvio's storage policies apply, with their bytes in
//! VELIO_STORAGE_DIR/<bucket>/<name>. Nuvio keeps its avatar images here.

use std::path::{Path as FsPath, PathBuf};

use anyhow::{Context, Result};
use axum::Extension;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Bearer, TokenProblem, caller, ctx_for, json_response};
use crate::AppState;
use crate::db;
use crate::tokens::sha256_hex;

// --- Errors, as storage-api renders them ---

#[derive(Debug, Clone)]
pub struct StorageError {
    /// The status in the body; the answer itself is 400 unless `sent` says otherwise, as storage-api.
    status: u16,
    sent: StatusCode,
    code: &'static str,
    error: &'static str,
    message: String,
}

impl StorageError {
    fn new(status: u16, code: &'static str, error: &'static str, message: impl Into<String>) -> Self {
        let sent = if status >= 500 { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::BAD_REQUEST };
        Self { status, sent, code, error, message: message.into() }
    }

    fn not_found() -> Self {
        Self::new(404, "NoSuchKey", "not_found", "Object not found")
    }

    fn no_bucket() -> Self {
        Self::new(404, "NoSuchBucket", "Bucket not found", "Bucket not found")
    }

    fn invalid_key(key: &str) -> Self {
        Self::new(400, "InvalidKey", "InvalidKey", format!("Invalid key: {key}"))
    }

    fn token(problem: TokenProblem) -> Self {
        let message = match problem {
            TokenProblem::Expired => "\"exp\" claim timestamp check failed",
            _ => "invalid signature",
        };
        Self::new(400, "InvalidJWT", "InvalidJWT", message)
    }

    fn database(error: &tokio_postgres::Error) -> Self {
        match error.as_db_error().map(|db| (db.code().code(), db.message())) {
            Some(("42501", message)) => Self::new(
                403,
                "AccessDenied",
                "Unauthorized",
                if message.contains("row-level security") {
                    "new row violates row-level security policy".to_owned()
                } else {
                    message.to_owned()
                },
            ),
            Some(("23505", _)) => Self::new(409, "ResourceAlreadyExists", "Duplicate", "The resource already exists"),
            _ => Self::internal(error),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!("Storage: {error:#}");
        Self::new(500, "InternalError", "internal", "Internal Server Error")
    }

    fn response(&self) -> Response {
        json_response(
            self.sent,
            json!({ "statusCode": self.status.to_string(), "code": self.code, "error": self.error, "message": self.message }),
        )
    }
}

impl From<anyhow::Error> for StorageError {
    fn from(error: anyhow::Error) -> Self {
        Self::internal(error)
    }
}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error)
    }
}

fn reply(result: Result<Response, StorageError>) -> Response {
    result.unwrap_or_else(|error| error.response())
}

// --- Names on disk ---

/// storage-api's isValidKey, and nothing that leaves the bucket's folder.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 1024
        && key.chars().all(|c| c.is_alphanumeric() || "_/!-.*'() &$@=;:+,?".contains(c))
        && key.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

fn valid_bucket(bucket: &str) -> bool {
    !bucket.is_empty()
        && bucket.len() <= 100
        && bucket.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
        && bucket != "."
        && bucket != ".."
}

fn file_of(root: &FsPath, bucket: &str, key: &str) -> PathBuf {
    let mut path = root.join(bucket);
    for part in key.split('/') {
        path.push(part);
    }
    path
}

/// Writes the bytes beside their place, then moves them in, so a reader sees all of a file or none.
async fn write_file(path: &FsPath, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let temporary = path.with_extension(format!("upload-{}", Uuid::new_v4().simple()));
    tokio::fs::write(&temporary, bytes).await?;
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(())
}

// --- Buckets and objects ---

struct Bucket {
    public: bool,
    size_limit: Option<i64>,
    mime_types: Option<Vec<String>>,
}

async fn bucket(db: &impl deadpool_postgres::GenericClient, id: &str) -> Result<Option<Bucket>, tokio_postgres::Error> {
    Ok(db
        .query_opt("SELECT public, file_size_limit, allowed_mime_types FROM storage.buckets WHERE id = $1", &[&id])
        .await?
        .map(|row| Bucket { public: row.get(0), size_limit: row.get(1), mime_types: row.get(2) }))
}

fn mime_allowed(allowed: &[String], mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or_default().trim();
    allowed.iter().any(|pattern| match pattern.strip_suffix("/*") {
        Some(kind) => base.split('/').next() == Some(kind),
        None => pattern == base,
    })
}

/// What storage-api keeps in `metadata`.
fn metadata(bytes: &[u8], mime: &str, cache_control: &str) -> Value {
    let now = crate::time::iso(crate::time::now_ms());
    json!({
        "eTag": format!("\"{}\"", &sha256_hex(bytes)[..32]),
        "size": bytes.len(),
        "mimetype": mime,
        "cacheControl": cache_control,
        "lastModified": now,
        "contentLength": bytes.len(),
        "httpStatusCode": 200,
    })
}

/// The caller, as storage-api reads the token Kong passed on.
fn who(
    state: &AppState,
    bearer: &Bearer,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
) -> Result<db::Ctx, StorageError> {
    let (role, claims) = caller(state, bearer.0.as_deref()).map_err(StorageError::token)?;
    Ok(ctx_for(role, claims, headers, method, path))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/storage/v1/bucket", get(list_buckets))
        .route("/storage/v1/bucket/{id}", get(get_bucket))
        .route("/storage/v1/object/list/{bucket}", post(list_objects))
        .route("/storage/v1/object/public/{bucket}/{*key}", get(public_object))
        .route("/storage/v1/object/authenticated/{bucket}/{*key}", get(private_object))
        .route("/storage/v1/object/{bucket}/{*key}", get(private_object).post(upload).put(upload).delete(delete_object))
        .route("/storage/v1/object/{bucket}", axum::routing::delete(delete_objects))
}

async fn upload(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path((bucket_id, key)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    reply(
        async {
            if !valid_bucket(&bucket_id) || !valid_key(&key) {
                return Err(StorageError::invalid_key(&key));
            }
            let ctx = who(&state, &bearer, &headers, &method, &format!("/object/{bucket_id}/{key}"))?;
            let mime = headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_owned();
            if mime.starts_with("multipart/form-data") {
                return Err(StorageError::new(
                    400,
                    "InvalidRequest",
                    "InvalidRequest",
                    "Send the file as the request body",
                ));
            }
            let cache_control = headers
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("no-cache")
                .to_owned();
            // PUT updates; POST creates, or replaces with x-upsert.
            let upsert =
                method == Method::PUT || headers.get("x-upsert").and_then(|value| value.to_str().ok()) == Some("true");
            let mut client = state.db.client().await?;
            let found = bucket(&client, &bucket_id).await.map_err(|e| StorageError::database(&e))?;
            let found = found.ok_or_else(StorageError::no_bucket)?;
            if let Some(limit) = found.size_limit
                && body.len() as i64 > limit
            {
                return Err(StorageError::new(
                    413,
                    "EntityTooLarge",
                    "Payload too large",
                    "The object exceeded the maximum allowed size",
                ));
            }
            if let Some(allowed) = found.mime_types.as_deref().filter(|allowed| !allowed.is_empty())
                && !mime_allowed(allowed, &mime)
            {
                return Err(StorageError::new(
                    415,
                    "InvalidMimeType",
                    "invalid_mime_type",
                    format!("mime type {mime} is not supported"),
                ));
            }
            let meta = metadata(&body, &mime, &cache_control);
            let owner = ctx.claims.get("sub").and_then(Value::as_str).map(str::to_owned);
            let owner_uuid = owner.as_deref().and_then(|sub| sub.parse::<Uuid>().ok());
            let version = Uuid::new_v4().to_string();
            let tx = db::begin(&mut client, &ctx).await.map_err(|e| StorageError::database(&e))?;
            let sql = if method == Method::PUT {
                "UPDATE storage.objects SET metadata = $3, owner = $4, owner_id = $5, version = $6, updated_at = now()
                 WHERE bucket_id = $1 AND name = $2 RETURNING id"
            } else if upsert {
                "INSERT INTO storage.objects (bucket_id, name, metadata, owner, owner_id, version)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (bucket_id, name) DO UPDATE SET metadata = excluded.metadata, owner = excluded.owner,
                     owner_id = excluded.owner_id, version = excluded.version, updated_at = now()
                 RETURNING id"
            } else {
                "INSERT INTO storage.objects (bucket_id, name, metadata, owner, owner_id, version)
                 VALUES ($1, $2, $3, $4, $5, $6) RETURNING id"
            };
            let row = tx
                .query_opt(sql, &[&bucket_id, &key, &meta, &owner_uuid, &owner, &version])
                .await
                .map_err(|e| StorageError::database(&e))?
                .ok_or_else(StorageError::not_found)?;
            let id: Uuid = row.get(0);
            write_file(&file_of(&state.cfg.storage_dir, &bucket_id, &key), &body).await?;
            tx.commit().await.map_err(|e| StorageError::database(&e))?;
            Ok(json_response(StatusCode::OK, json!({ "Id": id.to_string(), "Key": format!("{bucket_id}/{key}") })))
        }
        .await,
    )
}

/// The file, with storage-api's headers.
async fn serve(
    state: &AppState,
    bucket_id: &str,
    key: &str,
    meta: Value,
    headers: &HeaderMap,
) -> Result<Response, StorageError> {
    let etag = meta["eTag"].as_str().unwrap_or_default().to_owned();
    if !etag.is_empty()
        && headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok()) == Some(etag.as_str())
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        return Ok(response);
    }
    let bytes = match tokio::fs::read(file_of(&state.cfg.storage_dir, bucket_id, key)).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(StorageError::not_found()),
        Err(error) => return Err(error.into()),
    };
    let mut response = Response::new(Body::from(bytes));
    let out = response.headers_mut();
    let text = |value: &Value, fallback: &str| value.as_str().unwrap_or(fallback).to_owned();
    for (name, value) in [
        (header::CONTENT_TYPE, text(&meta["mimetype"], "application/octet-stream")),
        (header::CACHE_CONTROL, text(&meta["cacheControl"], "no-cache")),
        (header::ETAG, etag),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value)
            && !value.is_empty()
        {
            out.insert(name, value);
        }
    }
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    Ok(response)
}

async fn public_object(
    State(state): State<AppState>,
    Path((bucket_id, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    reply(
        async {
            if !valid_bucket(&bucket_id) || !valid_key(&key) {
                return Err(StorageError::not_found());
            }
            let client = state.db.client().await?;
            let public = bucket(&client, &bucket_id).await.map_err(|e| StorageError::database(&e))?;
            if !public.is_some_and(|bucket| bucket.public) {
                return Err(StorageError::not_found());
            }
            let meta: Value = client
                .query_opt(
                    "SELECT metadata FROM storage.objects WHERE bucket_id = $1 AND name = $2",
                    &[&bucket_id, &key],
                )
                .await
                .map_err(|e| StorageError::database(&e))?
                .ok_or_else(StorageError::not_found)?
                .get(0);
            serve(&state, &bucket_id, &key, meta, &headers).await
        }
        .await,
    )
}

async fn private_object(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path((bucket_id, key)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    reply(
        async {
            if !valid_bucket(&bucket_id) || !valid_key(&key) {
                return Err(StorageError::not_found());
            }
            let ctx = who(&state, &bearer, &headers, &method, &format!("/object/{bucket_id}/{key}"))?;
            let mut client = state.db.client().await?;
            let tx = db::begin(&mut client, &ctx).await.map_err(|e| StorageError::database(&e))?;
            let meta: Value = tx
                .query_opt(
                    "SELECT metadata FROM storage.objects WHERE bucket_id = $1 AND name = $2",
                    &[&bucket_id, &key],
                )
                .await
                .map_err(|e| StorageError::database(&e))?
                .ok_or_else(StorageError::not_found)?
                .get(0);
            tx.commit().await.map_err(|e| StorageError::database(&e))?;
            serve(&state, &bucket_id, &key, meta, &headers).await
        }
        .await,
    )
}

/// Deletes rows the caller may delete, then their files. The names deleted.
async fn delete_keys(
    state: &AppState,
    ctx: &db::Ctx,
    bucket_id: &str,
    keys: &[String],
) -> Result<Vec<Value>, StorageError> {
    let mut client = state.db.client().await?;
    let tx = db::begin(&mut client, ctx).await.map_err(|e| StorageError::database(&e))?;
    let rows = tx
        .query(
            "DELETE FROM storage.objects WHERE bucket_id = $1 AND name = ANY($2)
             RETURNING jsonb_build_object('bucket_id', bucket_id, 'name', name, 'id', id, 'owner', owner,
                 'updated_at', updated_at, 'created_at', created_at, 'last_accessed_at', last_accessed_at,
                 'metadata', metadata)",
            &[&bucket_id, &keys],
        )
        .await
        .map_err(|e| StorageError::database(&e))?;
    tx.commit().await.map_err(|e| StorageError::database(&e))?;
    let deleted: Vec<Value> = rows.iter().map(|row| row.get(0)).collect();
    for object in &deleted {
        if let Some(name) = object["name"].as_str() {
            let _ = tokio::fs::remove_file(file_of(&state.cfg.storage_dir, bucket_id, name)).await;
        }
    }
    Ok(deleted)
}

async fn delete_object(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path((bucket_id, key)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    reply(
        async {
            if !valid_bucket(&bucket_id) || !valid_key(&key) {
                return Err(StorageError::invalid_key(&key));
            }
            let ctx = who(&state, &bearer, &headers, &method, &format!("/object/{bucket_id}/{key}"))?;
            if delete_keys(&state, &ctx, &bucket_id, &[key]).await?.is_empty() {
                return Err(StorageError::not_found());
            }
            Ok(json_response(StatusCode::OK, json!({ "message": "Successfully deleted" })))
        }
        .await,
    )
}

async fn delete_objects(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path(bucket_id): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    reply(
        async {
            let body: Value = serde_json::from_slice(&body).unwrap_or_default();
            let keys: Vec<String> = body["prefixes"]
                .as_array()
                .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            if keys.is_empty() || keys.len() > 1000 {
                return Err(StorageError::new(
                    400,
                    "InvalidRequest",
                    "InvalidRequest",
                    "body/prefixes must NOT have fewer than 1 items",
                ));
            }
            let ctx = who(&state, &bearer, &headers, &method, &format!("/object/{bucket_id}"))?;
            let deleted = delete_keys(&state, &ctx, &bucket_id, &keys).await?;
            Ok(json_response(StatusCode::OK, Value::Array(deleted)))
        }
        .await,
    )
}

/// POST /object/list/{bucket}: the files and folders under a prefix, as storage.search.
async fn list_objects(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path(bucket_id): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    reply(
        async {
            let body: Value = serde_json::from_slice(&body).unwrap_or_default();
            let prefix = body["prefix"].as_str().unwrap_or_default().trim_matches('/').to_owned();
            let prefix = if prefix.is_empty() { String::new() } else { format!("{prefix}/") };
            let limit = body["limit"].as_i64().unwrap_or(100).clamp(1, 1000);
            let offset = body["offset"].as_i64().unwrap_or(0).max(0);
            let search = body["search"].as_str().unwrap_or_default().to_owned();
            let column = match body.pointer("/sortBy/column").and_then(Value::as_str) {
                Some("updated_at") => "updated_at",
                Some("created_at") => "created_at",
                Some("last_accessed_at") => "last_accessed_at",
                _ => "name",
            };
            let order =
                if body.pointer("/sortBy/order").and_then(Value::as_str) == Some("desc") { "DESC" } else { "ASC" };
            let ctx = who(&state, &bearer, &headers, &method, &format!("/object/list/{bucket_id}"))?;
            let mut client = state.db.client().await?;
            let tx = db::begin(&mut client, &ctx).await.map_err(|e| StorageError::database(&e))?;
            let row = tx
                .query_one(
                    &format!(
                        "WITH under AS (
                             SELECT split_part(substr(name, length($2) + 1), '/', 1) AS entry,
                                    position('/' IN substr(name, length($2) + 1)) > 0 AS folder,
                                    id, updated_at, created_at, last_accessed_at, metadata
                             FROM storage.objects
                             WHERE bucket_id = $1 AND name LIKE replace(replace($2, '%', '\\%'), '_', '\\_') || '%'),
                         entries AS (
                             SELECT DISTINCT ON (entry) entry AS name,
                                    CASE WHEN folder THEN NULL ELSE id END AS id,
                                    CASE WHEN folder THEN NULL ELSE updated_at END AS updated_at,
                                    CASE WHEN folder THEN NULL ELSE created_at END AS created_at,
                                    CASE WHEN folder THEN NULL ELSE last_accessed_at END AS last_accessed_at,
                                    CASE WHEN folder THEN NULL ELSE metadata END AS metadata
                             FROM under WHERE entry ILIKE '%' || $3 || '%' ORDER BY entry, folder DESC)
                         SELECT coalesce(jsonb_agg(e ORDER BY e.{column} {order} NULLS FIRST, e.name), '[]')
                         FROM (SELECT * FROM entries ORDER BY {column} {order} NULLS FIRST, name LIMIT $4 OFFSET $5) e"
                    ),
                    &[&bucket_id, &prefix, &search, &limit, &offset],
                )
                .await
                .map_err(|e| StorageError::database(&e))?;
            tx.commit().await.map_err(|e| StorageError::database(&e))?;
            Ok(json_response(StatusCode::OK, row.get(0)))
        }
        .await,
    )
}

const BUCKET_JSON: &str = "jsonb_build_object('id', id, 'name', name, 'owner', coalesce(owner::text, ''),
    'public', public, 'file_size_limit', file_size_limit, 'allowed_mime_types', allowed_mime_types,
    'created_at', created_at, 'updated_at', updated_at)";

async fn buckets_as(state: &AppState, ctx: &db::Ctx, id: Option<&str>) -> Result<Vec<Value>, StorageError> {
    let mut client = state.db.client().await?;
    let tx = db::begin(&mut client, ctx).await.map_err(|e| StorageError::database(&e))?;
    let rows = tx
        .query(
            &format!("SELECT {BUCKET_JSON} FROM storage.buckets WHERE $1::text IS NULL OR id = $1 ORDER BY name"),
            &[&id],
        )
        .await
        .map_err(|e| StorageError::database(&e))?;
    tx.commit().await.map_err(|e| StorageError::database(&e))?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

async fn list_buckets(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    reply(
        async {
            let ctx = who(&state, &bearer, &headers, &method, "/bucket")?;
            Ok(json_response(StatusCode::OK, Value::Array(buckets_as(&state, &ctx, None).await?)))
        }
        .await,
    )
}

async fn get_bucket(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path(id): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    reply(
        async {
            let ctx = who(&state, &bearer, &headers, &method, &format!("/bucket/{id}"))?;
            let found = buckets_as(&state, &ctx, Some(&id)).await?;
            let found = found.into_iter().next().ok_or_else(StorageError::no_bucket)?;
            Ok(json_response(StatusCode::OK, found))
        }
        .await,
    )
}

// --- Nuvio's avatars ---

/// Puts each image of `dir` in the avatars bucket, as Nuvio's
/// seed-avatars.sh does, unless it's there already. The images are
/// Nuvio's (references/self-host/assets/avatars), not shipped with Velio.
pub async fn seed_avatars(state: &AppState, dir: &FsPath) -> Result<usize> {
    let mut entries = tokio::fs::read_dir(dir).await.with_context(|| format!("Reading {}", dir.display()))?;
    let client = state.db.client().await?;
    let mut added = 0;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let mime = match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            _ => continue,
        };
        if !valid_key(&name) {
            continue;
        }
        let path = file_of(&state.cfg.storage_dir, "avatars", &name);
        let present = client
            .query_opt("SELECT 1 FROM storage.objects WHERE bucket_id = 'avatars' AND name = $1", &[&name])
            .await?
            .is_some();
        if present && tokio::fs::try_exists(&path).await.unwrap_or(false) {
            continue;
        }
        let bytes = tokio::fs::read(entry.path()).await?;
        write_file(&path, &bytes).await?;
        let meta = metadata(&bytes, mime, "public, max-age=31536000, immutable");
        client
            .execute(
                "INSERT INTO storage.objects (bucket_id, name, metadata, version) VALUES ('avatars', $1, $2, $3)
                 ON CONFLICT (bucket_id, name) DO UPDATE SET metadata = excluded.metadata, updated_at = now()",
                &[&name, &meta, &Uuid::new_v4().to_string()],
            )
            .await?;
        added += 1;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_stay_in_their_bucket() {
        assert!(valid_key("avatar_aang_1772809370453.png"));
        assert!(valid_key("user/abc/cover (1).jpg"));
        assert!(!valid_key("../etc/passwd"));
        assert!(!valid_key("a//b"));
        assert!(!valid_key("a/./b"));
        assert!(!valid_key("a\\b"));
        assert!(!valid_bucket(".."));
        assert!(mime_allowed(&["image/png".into()], "image/png"));
        assert!(mime_allowed(&["image/*".into()], "image/webp; q=1"));
        assert!(!mime_allowed(&["image/png".into()], "text/html"));
    }
}
