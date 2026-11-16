//! PostgREST (v12, as Supabase serves it) at /rest/v1: Nuvio's tables and
//! functions in the `public` schema, run as the token's role with its claims,
//! so Nuvio's row security and grants decide what each caller sees.
//!
//! What's here is the part Supabase's client libraries send: column lists,
//! horizontal filters with `or`/`and`/`not`, ordering, limits and ranges,
//! inserts and upserts, updates, deletes, `Prefer`, single-object answers,
//! and calls to functions. Embedded resources aren't (Nuvio's clients don't
//! use them).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::Extension;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use deadpool_postgres::GenericClient;
use serde_json::{Map, Value, json};
use tokio_postgres::types::Type;

use super::{Bearer, TokenProblem, caller, ctx_for};
use crate::AppState;
use crate::db::{self, Role};

// --- Errors, as PostgREST sends them ---

#[derive(Debug, Clone)]
pub struct RestError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub details: Option<String>,
    pub hint: Option<String>,
    expired: bool,
}

impl RestError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self { status, code: code.into(), message: message.into(), details: None, hint: None, expired: false }
    }

    fn details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// A request PostgREST can't parse (PGRST100).
    fn parse(details: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "PGRST100", "\"failed to parse filter\"").details(details)
    }

    fn token(problem: TokenProblem) -> Self {
        match problem {
            TokenProblem::Expired => {
                Self { expired: true, ..Self::new(StatusCode::UNAUTHORIZED, "PGRST303", "JWT expired") }
            }
            TokenProblem::Invalid => {
                Self::new(StatusCode::UNAUTHORIZED, "PGRST301", "No suitable key or wrong key type")
            }
            TokenProblem::UnknownRole => Self::new(StatusCode::UNAUTHORIZED, "PGRST301", "JWT role not allowed"),
        }
    }

    /// A database error, with PostgREST's status for its SQLSTATE.
    fn database(error: &tokio_postgres::Error, role: Role) -> Self {
        let Some(db) = error.as_db_error() else {
            tracing::error!("PostgREST: {error:#}");
            return Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "PGRST000",
                "Database client error. Retrying the connection.",
            );
        };
        let code = db.code().code();
        Self {
            status: status_of(code, role),
            code: code.to_owned(),
            message: db.message().to_owned(),
            details: db.detail().map(str::to_owned),
            hint: db.hint().map(str::to_owned),
            expired: false,
        }
    }

    pub fn response(&self) -> Response {
        let body = json!({ "code": self.code, "details": self.details, "hint": self.hint, "message": self.message });
        let mut response = (self.status, body.to_string()).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
        if self.expired {
            headers.insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer error=\"invalid_token\", error_description=\"JWT expired\""),
            );
        } else if self.code == "PGRST301" {
            headers.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer error=\"invalid_token\""));
        }
        response
    }
}

impl From<anyhow::Error> for RestError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!("PostgREST: {error:#}");
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "PGRST000", "Database client error. Retrying the connection.")
    }
}

/// PostgREST's HTTP status for a SQLSTATE (docs: errors.html).
pub fn status_of(code: &str, role: Role) -> StatusCode {
    let status = |n: u16| StatusCode::from_u16(n).unwrap_or(StatusCode::BAD_REQUEST);
    if let Some(custom) = code.strip_prefix("PT")
        && let Ok(n) = custom.parse::<u16>()
    {
        return status(n);
    }
    match code {
        "23503" | "23505" => return status(409),
        "25006" => return status(405),
        "P0001" => return status(400),
        "42883" | "42P01" => return status(404),
        "42P17" => return status(500),
        "42501" => return status(if role == Role::Anon { 401 } else { 403 }),
        _ => {}
    }
    let class = code.get(..2).unwrap_or_default();
    match class {
        "08" | "53" => status(503),
        "09" | "25" | "2D" | "38" | "39" | "3B" | "40" | "54" | "55" | "57" | "58" | "F0" | "HV" | "P0" | "XX" => {
            status(500)
        }
        "0L" | "0P" | "28" => status(403),
        _ => status(400),
    }
}

// --- Reading the request ---

/// SQL text of an identifier.
fn ident(name: &str) -> Result<String, RestError> {
    if name.is_empty() || name.contains('\0') {
        return Err(RestError::parse(format!("unexpected identifier {name:?}")));
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

/// SQL text of a value, of unknown type, so Postgres reads it as its column's type.
fn literal(value: &str) -> Result<String, RestError> {
    if value.contains('\0') {
        return Err(RestError::parse("a value can't hold a NUL character"));
    }
    // standard_conforming_strings is on for every request (db::begin).
    Ok(format!("'{}'", value.replace('\'', "''")))
}

/// Splits at commas not inside parentheses, braces or double quotes.
fn split_top(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quoted = false;
    let mut escaped = false;
    let mut current = String::new();
    for c in text.chars() {
        if quoted {
            current.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                current.push(c);
            }
            '(' | '{' | '[' => {
                depth += 1;
                current.push(c);
            }
            ')' | '}' | ']' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts.into_iter().map(|part| part.trim().to_owned()).filter(|part| !part.is_empty()).collect()
}

/// A value as written in a filter: `"quoted, with \"escapes\""` or bare.
fn unquote(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) else {
        return value.to_owned();
    };
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A column, or a JSON path into one (`data->a->>b`), as SQL, with the name
/// PostgREST gives it in an answer.
fn field(text: &str, qualifier: &str) -> Result<(String, String), RestError> {
    let text = text.trim();
    let Some(first) = text.find("->") else {
        return Ok((format!("{qualifier}{}", ident(text)?), text.to_owned()));
    };
    let mut sql = format!("{qualifier}{}", ident(&text[..first])?);
    let mut rest = &text[first..];
    let mut name = text[..first].to_owned();
    while !rest.is_empty() {
        let (arrow, after) = if let Some(after) = rest.strip_prefix("->>") {
            ("->>", after)
        } else if let Some(after) = rest.strip_prefix("->") {
            ("->", after)
        } else {
            return Err(RestError::parse(format!("unexpected {rest:?} in {text:?}")));
        };
        let end = after.find("->").unwrap_or(after.len());
        let key = &after[..end];
        if key.is_empty() {
            return Err(RestError::parse(format!("an empty key in {text:?}")));
        }
        let key_sql = if key.bytes().all(|b| b.is_ascii_digit())
            || key.starts_with('-') && key[1..].bytes().all(|b| b.is_ascii_digit())
        {
            key.to_owned()
        } else {
            literal(key)?
        };
        sql = format!("{sql}{arrow}{key_sql}");
        name = key.to_owned();
        rest = &after[end..];
    }
    Ok((sql, name))
}

fn cast_type(cast: &str) -> Result<&str, RestError> {
    let ok = !cast.is_empty() && cast.chars().all(|c| c.is_ascii_alphanumeric() || " _[]".contains(c));
    if ok { Ok(cast) } else { Err(RestError::parse(format!("unexpected cast {cast:?}"))) }
}

/// The `select` list as SQL.
fn columns(select: Option<&str>, qualifier: &str) -> Result<String, RestError> {
    let select = select.map(str::trim).filter(|select| !select.is_empty()).unwrap_or("*");
    let mut out = Vec::new();
    for item in split_top(select) {
        if item == "*" {
            out.push(format!("{qualifier}*"));
            continue;
        }
        if item.contains('(') || item.starts_with("...") {
            return Err(RestError::new(
                StatusCode::BAD_REQUEST,
                "PGRST200",
                format!("Could not find a relationship for '{item}' in the schema cache"),
            ));
        }
        let (alias, rest) = match item.find(':') {
            Some(at) if !item[at..].starts_with("::") => (Some(&item[..at]), &item[at + 1..]),
            _ => (None, item.as_str()),
        };
        let (expression, cast) = match rest.rfind("::") {
            Some(at) => (&rest[..at], Some(cast_type(&rest[at + 2..])?)),
            None => (rest, None),
        };
        let (mut sql, name) = field(expression, qualifier)?;
        if let Some(cast) = cast {
            sql = format!("({sql})::{cast}");
        }
        out.push(format!("{sql} AS {}", ident(alias.unwrap_or(&name))?));
    }
    Ok(out.join(", "))
}

/// One filter, `<operator>.<value>`, on `column` (already SQL).
fn operation(column: &str, spec: &str) -> Result<String, RestError> {
    let (negate, spec) = match spec.strip_prefix("not.") {
        Some(rest) => (true, rest),
        None => (false, spec),
    };
    let (operator, value) = spec.split_once('.').ok_or_else(|| RestError::parse(format!("no operator in {spec:?}")))?;
    let (operator, modifier) = match operator.split_once('(') {
        Some((name, rest)) => (name, rest.strip_suffix(')')),
        None => (operator, None),
    };
    let binary =
        |sql_op: &str| -> Result<String, RestError> { Ok(format!("{column} {sql_op} {}", literal(&unquote(value))?)) };
    let pattern = |sql_op: &str| -> Result<String, RestError> {
        Ok(format!("{column} {sql_op} {}", literal(&unquote(value).replace('*', "%"))?))
    };
    let text_search = |function: &str| -> Result<String, RestError> {
        let query = literal(&unquote(value))?;
        Ok(match modifier {
            Some(config) => format!("{column} @@ {function}({}, {query})", literal(config)?),
            None => format!("{column} @@ {function}({query})"),
        })
    };
    let sql = match operator {
        "eq" => binary("=")?,
        "neq" => binary("<>")?,
        "gt" => binary(">")?,
        "gte" => binary(">=")?,
        "lt" => binary("<")?,
        "lte" => binary("<=")?,
        "like" => pattern("LIKE")?,
        "ilike" => pattern("ILIKE")?,
        "match" => binary("~")?,
        "imatch" => binary("~*")?,
        "cs" => binary("@>")?,
        "cd" => binary("<@")?,
        "ov" => binary("&&")?,
        "sl" => binary("<<")?,
        "sr" => binary(">>")?,
        "nxr" => binary("&<")?,
        "nxl" => binary("&>")?,
        "adj" => binary("-|-")?,
        "isdistinct" => binary("IS DISTINCT FROM")?,
        "fts" => text_search("to_tsquery")?,
        "plfts" => text_search("plainto_tsquery")?,
        "phfts" => text_search("phraseto_tsquery")?,
        "wfts" => text_search("websearch_to_tsquery")?,
        "is" => match value.to_ascii_lowercase().as_str() {
            "null" => format!("{column} IS NULL"),
            "not_null" => format!("{column} IS NOT NULL"),
            "true" => format!("{column} IS TRUE"),
            "false" => format!("{column} IS FALSE"),
            "unknown" => format!("{column} IS UNKNOWN"),
            _ => return Err(RestError::parse(format!("unexpected is.{value}"))),
        },
        "in" => {
            let list = value
                .strip_prefix('(')
                .and_then(|rest| rest.strip_suffix(')'))
                .ok_or_else(|| RestError::parse(format!("in. needs a list in parentheses, not {value:?}")))?;
            let items = split_top(list).iter().map(|item| literal(&unquote(item))).collect::<Result<Vec<_>, _>>()?;
            if items.is_empty() { "false".to_owned() } else { format!("{column} IN ({})", items.join(", ")) }
        }
        _ => return Err(RestError::parse(format!("unknown operator {operator:?}"))),
    };
    Ok(if negate { format!("NOT ({sql})") } else { sql })
}

/// `or=(a.eq.1,and(b.gt.2,c.is.null))` and the like.
fn logic(joiner: &str, list: &str, negate: bool, qualifier: &str) -> Result<String, RestError> {
    let inner = list
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(|| RestError::parse(format!("{joiner} needs a list in parentheses")))?;
    let mut parts = Vec::new();
    for item in split_top(inner) {
        let (negated, item_text) = match item.strip_prefix("not.") {
            Some(rest) if rest.starts_with("and(") || rest.starts_with("or(") => (true, rest),
            _ => (false, item.as_str()),
        };
        if let Some(rest) = item_text.strip_prefix("and")
            && rest.starts_with('(')
        {
            parts.push(logic("AND", rest, negated, qualifier)?);
        } else if let Some(rest) = item_text.strip_prefix("or")
            && rest.starts_with('(')
        {
            parts.push(logic("OR", rest, negated, qualifier)?);
        } else {
            let (name, spec) = item.split_once('.').ok_or_else(|| RestError::parse(format!("unexpected {item:?}")))?;
            parts.push(operation(&field(name, qualifier)?.0, spec)?);
        }
    }
    if parts.is_empty() {
        return Ok("true".to_owned());
    }
    let sql = format!("({})", parts.join(&format!(" {joiner} ")));
    Ok(if negate { format!("NOT {sql}") } else { sql })
}

/// The query's parameters that aren't filters.
const RESERVED: [&str; 6] = ["select", "order", "limit", "offset", "columns", "on_conflict"];

/// The WHERE clause from a query's filters.
fn conditions(query: &[(String, String)], skip: &[String], qualifier: &str) -> Result<String, RestError> {
    let mut parts = Vec::new();
    for (name, value) in query {
        if RESERVED.contains(&name.as_str()) || skip.contains(name) || name == "apikey" {
            continue;
        }
        let part = match name.as_str() {
            "or" => logic("OR", value, false, qualifier)?,
            "and" => logic("AND", value, false, qualifier)?,
            "not.or" => logic("OR", value, true, qualifier)?,
            "not.and" => logic("AND", value, true, qualifier)?,
            _ if name.contains('.') && !name.contains("->") => {
                return Err(RestError::new(
                    StatusCode::BAD_REQUEST,
                    "PGRST108",
                    format!("'{name}' is not an embedded resource in this request"),
                ));
            }
            _ => operation(&field(name, qualifier)?.0, value)?,
        };
        parts.push(part);
    }
    Ok(if parts.is_empty() { "true".to_owned() } else { parts.join(" AND ") })
}

fn ordering(order: Option<&str>, qualifier: &str) -> Result<String, RestError> {
    let Some(order) = order.filter(|order| !order.trim().is_empty()) else { return Ok(String::new()) };
    let mut terms = Vec::new();
    for item in split_top(order) {
        let mut pieces = item.split('.');
        let column = field(pieces.next().unwrap_or_default(), qualifier)?.0;
        let mut term = column;
        for piece in pieces {
            term.push_str(match piece {
                "asc" => " ASC",
                "desc" => " DESC",
                "nullsfirst" => " NULLS FIRST",
                "nullslast" => " NULLS LAST",
                _ => return Err(RestError::parse(format!("unexpected {piece:?} in order"))),
            });
        }
        terms.push(term);
    }
    Ok(format!(" ORDER BY {}", terms.join(", ")))
}

#[derive(Debug, Default, Clone, Copy)]
struct Window {
    offset: i64,
    limit: Option<i64>,
}

impl Window {
    fn sql(self) -> String {
        let mut sql = String::new();
        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if self.offset > 0 {
            sql.push_str(&format!(" OFFSET {}", self.offset));
        }
        sql
    }
}

fn window(query: &[(String, String)], headers: &HeaderMap) -> Result<Window, RestError> {
    let number = |name: &str| -> Result<Option<i64>, RestError> {
        match query.iter().find(|(key, _)| key == name) {
            Some((_, value)) => value
                .parse::<i64>()
                .ok()
                .filter(|n| *n >= 0)
                .map(Some)
                .ok_or_else(|| RestError::parse(format!("{name} must be a whole number"))),
            None => Ok(None),
        }
    };
    let mut window = Window { offset: number("offset")?.unwrap_or(0), limit: number("limit")? };
    if let Some(range) = headers.get(header::RANGE).and_then(|value| value.to_str().ok()) {
        let range = range.trim().strip_prefix("items=").unwrap_or(range.trim());
        if let Some((from, to)) = range.split_once('-')
            && let Ok(from) = from.trim().parse::<i64>()
        {
            let to = to.trim().parse::<i64>().ok();
            if to.is_some_and(|to| to < from) {
                return Err(RestError::new(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "PGRST103",
                    "Requested range not satisfiable",
                ));
            }
            window.offset = window.offset.max(from);
            if let Some(to) = to {
                let span = to - from + 1;
                window.limit = Some(window.limit.map_or(span, |limit| limit.min(span)));
            }
        }
    }
    Ok(window)
}

/// What `Prefer` asks for.
#[derive(Debug, Default)]
struct Prefer {
    representation: bool,
    count: bool,
    merge: bool,
    ignore: bool,
    single_object: bool,
    rollback: bool,
    applied: Vec<String>,
}

fn prefer(headers: &HeaderMap) -> Prefer {
    let mut prefer = Prefer::default();
    for value in headers.get_all("prefer") {
        let Ok(value) = value.to_str() else { continue };
        for token in value.split(',').map(str::trim) {
            let known = match token {
                "return=representation" => {
                    prefer.representation = true;
                    true
                }
                "return=minimal" | "return=headers-only" => true,
                "count=exact" | "count=planned" | "count=estimated" => {
                    prefer.count = true;
                    true
                }
                "resolution=merge-duplicates" => {
                    prefer.merge = true;
                    true
                }
                "resolution=ignore-duplicates" => {
                    prefer.ignore = true;
                    true
                }
                "params=single-object" => {
                    prefer.single_object = true;
                    true
                }
                "tx=rollback" => {
                    prefer.rollback = true;
                    true
                }
                "tx=commit" => true,
                _ => false,
            };
            if known {
                prefer.applied.push(token.to_owned());
            }
        }
    }
    prefer
}

/// Whether the client wants one object rather than an array.
fn wants_object(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/vnd.pgrst.object"))
}

fn query_pairs(uri: &Uri) -> Vec<(String, String)> {
    form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()).into_owned().collect()
}

fn param<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

/// Only `public` is exposed, as Nuvio's PGRST_DB_SCHEMAS.
fn check_profile(headers: &HeaderMap) -> Result<(), RestError> {
    for name in ["accept-profile", "content-profile"] {
        if let Some(schema) = headers.get(name).and_then(|value| value.to_str().ok())
            && schema != "public"
        {
            return Err(RestError::new(StatusCode::NOT_ACCEPTABLE, "PGRST106", "Invalid schema: ".to_owned() + schema)
                .hint("Only the following schemas are exposed: public"));
        }
    }
    Ok(())
}

fn json_body(body: &Bytes) -> Result<Value, RestError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|error| {
        RestError::new(StatusCode::BAD_REQUEST, "PGRST102", "Empty or invalid json").details(error.to_string())
    })
}

// --- Answering ---

struct Rows {
    json: String,
    count: i64,
}

struct Answer {
    status: StatusCode,
    body: Option<String>,
    object: bool,
    content_range: Option<String>,
    applied: Vec<String>,
}

impl Answer {
    fn response(self, head: bool) -> Response {
        let mut response = match self.body {
            Some(body) if !head => (self.status, body).into_response(),
            _ => self.status.into_response(),
        };
        let headers = response.headers_mut();
        if self.status != StatusCode::NO_CONTENT {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(if self.object {
                    "application/vnd.pgrst.object+json; charset=utf-8"
                } else {
                    "application/json; charset=utf-8"
                }),
            );
        }
        if let Some(range) = self.content_range.and_then(|range| HeaderValue::from_str(&range).ok()) {
            headers.insert(header::CONTENT_RANGE, range);
        }
        if !self.applied.is_empty()
            && let Ok(value) = HeaderValue::from_str(&self.applied.join(", "))
        {
            headers.insert("preference-applied", value);
        }
        response
    }
}

fn content_range(offset: i64, rows: i64, total: Option<i64>) -> String {
    let total = total.map_or_else(|| "*".to_owned(), |total| total.to_string());
    if rows == 0 { format!("*/{total}") } else { format!("{offset}-{}/{total}", offset + rows - 1) }
}

/// The array, or its one object for `Accept: application/vnd.pgrst.object+json`.
fn shape(rows: Rows, object: bool) -> Result<String, RestError> {
    if !object {
        return Ok(rows.json);
    }
    if rows.count != 1 {
        return Err(RestError::new(
            StatusCode::NOT_ACCEPTABLE,
            "PGRST116",
            "JSON object requested, multiple (or no) rows returned",
        )
        .details(format!("The result contains {} rows", rows.count)));
    }
    let array: Value = serde_json::from_str(&rows.json).unwrap_or_default();
    Ok(array.get(0).cloned().unwrap_or(Value::Null).to_string())
}

/// Runs SQL made for this request, with its JSON as `$1` if any. It's sent
/// with its parameter's type, so Postgres parses, binds and runs it in one
/// round trip rather than describing it first.
async fn typed(
    tx: &tokio_postgres::Transaction<'_>,
    sql: &str,
    json: Option<&Value>,
) -> Result<tokio_postgres::Row, tokio_postgres::Error> {
    match json {
        Some(json) => tx.query_typed_one(sql, &[(json, Type::JSON)]).await,
        None => tx.query_typed_one(sql, &[]).await,
    }
}

async fn rows(
    tx: &tokio_postgres::Transaction<'_>,
    sql: &str,
    json: Option<&Value>,
) -> Result<Rows, tokio_postgres::Error> {
    let row = typed(tx, sql, json).await?;
    Ok(Rows { json: row.get(0), count: row.get(1) })
}

// --- The routes ---

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/rest/v1/", get(root))
        .route("/rest/v1/rpc/{function}", get(rpc_route).post(rpc_route))
        .route("/rest/v1/{table}", get(table_route).post(table_route).patch(table_route).delete(table_route))
}

/// PostgREST's root serves its OpenAPI description; Nuvio's clients don't read it.
async fn root() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/openapi+json; charset=utf-8")],
        json!({ "swagger": "2.0", "info": { "title": "Nuvio", "version": "12" }, "paths": {} }).to_string(),
    )
        .into_response()
}

struct Request {
    state: AppState,
    role: Role,
    ctx: db::Ctx,
    headers: HeaderMap,
    method: Method,
    query: Vec<(String, String)>,
    prefer: Prefer,
}

fn request(
    state: AppState,
    bearer: &Bearer,
    headers: HeaderMap,
    method: Method,
    uri: &Uri,
) -> Result<Request, RestError> {
    check_profile(&headers)?;
    let (role, claims) = caller(&state, bearer.0.as_deref()).map_err(RestError::token)?;
    let ctx = ctx_for(role, claims, &headers, &method, uri.path());
    Ok(Request { state, role, ctx, query: query_pairs(uri), prefer: prefer(&headers), headers, method })
}

impl Request {
    /// Runs `work` in a transaction as the caller, committing unless it fails
    /// or `Prefer: tx=rollback` asks not to.
    async fn run<T>(
        &self,
        work: impl AsyncFnOnce(&deadpool_postgres::Transaction<'_>) -> Result<T, tokio_postgres::Error>,
    ) -> Result<T, RestError> {
        let mut client = self.state.db.client().await?;
        let tx = db::begin(&mut client, &self.ctx).await.map_err(|error| RestError::database(&error, self.role))?;
        let result = work(&tx).await.map_err(|error| RestError::database(&error, self.role))?;
        if self.prefer.rollback {
            tx.rollback().await.map_err(|error| RestError::database(&error, self.role))?;
        } else {
            tx.commit().await.map_err(|error| RestError::database(&error, self.role))?;
        }
        Ok(result)
    }
}

async fn table_route(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path(table): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let head = method == Method::HEAD;
    let result = async {
        let request = request(state, &bearer, headers, method.clone(), &uri)?;
        let table = format!("public.{}", ident(&table)?);
        match method {
            Method::GET | Method::HEAD => read(&request, &table).await,
            Method::POST => insert(&request, &table, json_body(&body)?).await,
            Method::PATCH => update(&request, &table, json_body(&body)?).await,
            Method::DELETE => remove(&request, &table).await,
            _ => Err(RestError::new(StatusCode::METHOD_NOT_ALLOWED, "PGRST117", "Unsupported HTTP method")),
        }
    }
    .await;
    match result {
        Ok(answer) => answer.response(head),
        Err(error) => error.response(),
    }
}

async fn read(request: &Request, table: &str) -> Result<Answer, RestError> {
    let query = &request.query;
    let select = columns(param(query, "select"), "")?;
    let filter = conditions(query, &[], "")?;
    let order = ordering(param(query, "order"), "")?;
    let window = window(query, &request.headers)?;
    let object = wants_object(&request.headers);
    let sql = format!(
        "SELECT coalesce(json_agg(_r), '[]')::text, count(*) FROM
           (SELECT {select} FROM {table} WHERE {filter}{order}{}) _r",
        window.sql()
    );
    let count_sql = request.prefer.count.then(|| format!("SELECT count(*) FROM {table} WHERE {filter}"));
    let (found, total) = request
        .run(async |tx| {
            let found = rows(tx, &sql, None).await?;
            let total: Option<i64> = match &count_sql {
                Some(sql) => Some(typed(tx, sql, None).await?.get(0)),
                None => None,
            };
            Ok((found, total))
        })
        .await?;
    let partial = total.is_some_and(|total| total > found.count);
    let range = content_range(window.offset, found.count, total);
    Ok(Answer {
        status: if partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK },
        body: Some(shape(found, object)?),
        object,
        content_range: Some(range),
        applied: request.prefer.applied.clone(),
    })
}

/// The keys of the rows in a body, in the order first seen, or `columns=`.
fn body_columns(rows: &[Value], columns: Option<&str>) -> Result<Vec<String>, RestError> {
    if let Some(columns) = columns {
        return Ok(columns.split(',').map(|column| column.trim().to_owned()).filter(|c| !c.is_empty()).collect());
    }
    let mut keys: Vec<String> = Vec::new();
    for row in rows {
        let Some(object) = row.as_object() else {
            return Err(RestError::new(StatusCode::BAD_REQUEST, "PGRST102", "All object keys must match"));
        };
        for key in object.keys() {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    Ok(keys)
}

async fn primary_key(tx: &impl GenericClient, table: &str) -> Result<Vec<String>, tokio_postgres::Error> {
    let rows = tx
        .query(
            "SELECT a.attname::text FROM pg_index i
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
             WHERE i.indrelid = $1::text::regclass AND i.indisprimary ORDER BY array_position(i.indkey, a.attnum)",
            &[&table],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

/// The answer to a write: the rows if `return=representation`, else none.
fn written(request: &Request, found: Rows, status: StatusCode) -> Result<Answer, RestError> {
    let object = wants_object(&request.headers);
    let range = content_range(0, found.count, None);
    if request.prefer.representation {
        return Ok(Answer {
            status,
            body: Some(shape(found, object)?),
            object,
            content_range: Some(range),
            applied: request.prefer.applied.clone(),
        });
    }
    if object && found.count != 1 {
        shape(found, true)?;
    }
    Ok(Answer {
        status: if status == StatusCode::CREATED { status } else { StatusCode::NO_CONTENT },
        body: None,
        object: false,
        content_range: Some(range),
        applied: request.prefer.applied.clone(),
    })
}

async fn insert(request: &Request, table: &str, body: Value) -> Result<Answer, RestError> {
    let rows_in = match body {
        Value::Array(rows) => rows,
        Value::Object(_) => vec![body],
        _ => return Err(RestError::new(StatusCode::BAD_REQUEST, "PGRST102", "Empty or invalid json")),
    };
    let keys = body_columns(&rows_in, param(&request.query, "columns"))?;
    let select = columns(param(&request.query, "select"), "")?;
    let target = keys.iter().map(|key| ident(key)).collect::<Result<Vec<_>, _>>()?.join(", ");
    let upsert = request.prefer.merge || request.prefer.ignore;
    let on_conflict = param(&request.query, "on_conflict").map(str::to_owned);
    let ignore = request.prefer.ignore;
    let table_name = table.to_owned();
    let count = rows_in.len() as i64;
    let payload = Value::Array(rows_in);
    let found = request
        .run(async |tx| {
            let mut sql = if keys.is_empty() {
                format!("INSERT INTO {table_name} SELECT FROM generate_series(1, {count})")
            } else {
                format!(
                    "INSERT INTO {table_name} ({target}) SELECT {target} FROM json_populate_recordset(null::{table_name}, $1::json)"
                )
            };
            if upsert {
                let conflict = match &on_conflict {
                    Some(list) => list.split(',').map(|c| c.trim().to_owned()).collect(),
                    None => request.state.schema.primary_key(tx, &table_name).await?.to_vec(),
                };
                let conflict = conflict.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect::<Vec<_>>();
                if ignore || keys.is_empty() {
                    sql.push_str(&format!(" ON CONFLICT ({}) DO NOTHING", conflict.join(", ")));
                } else {
                    let set = keys
                        .iter()
                        .map(|key| {
                            let key = format!("\"{}\"", key.replace('"', "\"\""));
                            format!("{key} = EXCLUDED.{key}")
                        })
                        .collect::<Vec<_>>();
                    sql.push_str(&format!(" ON CONFLICT ({}) DO UPDATE SET {}", conflict.join(", "), set.join(", ")));
                }
            }
            let sql = format!(
                "WITH _w AS ({sql} RETURNING *) SELECT coalesce(json_agg(_r), '[]')::text, count(*) FROM (SELECT {select} FROM _w) _r"
            );
            rows(tx, &sql, (!keys.is_empty()).then_some(&payload)).await
        })
        .await?;
    written(request, found, StatusCode::CREATED)
}

async fn update(request: &Request, table: &str, body: Value) -> Result<Answer, RestError> {
    let Value::Object(fields) = &body else {
        return Err(RestError::new(StatusCode::BAD_REQUEST, "PGRST102", "Empty or invalid json"));
    };
    let select = columns(param(&request.query, "select"), "")?;
    let filter = conditions(&request.query, &[], "")?;
    if fields.is_empty() {
        return written(request, Rows { json: "[]".into(), count: 0 }, StatusCode::OK);
    }
    let keys = fields.keys().map(|key| ident(key)).collect::<Result<Vec<_>, _>>()?.join(", ");
    let sql = format!(
        "WITH _w AS (UPDATE {table} SET ({keys}) = (SELECT {keys} FROM json_populate_record(null::{table}, $1::json))
           WHERE {filter} RETURNING *)
         SELECT coalesce(json_agg(_r), '[]')::text, count(*) FROM (SELECT {select} FROM _w) _r"
    );
    let found = request.run(async |tx| rows(tx, &sql, Some(&body)).await).await?;
    written(request, found, StatusCode::OK)
}

async fn remove(request: &Request, table: &str) -> Result<Answer, RestError> {
    let select = columns(param(&request.query, "select"), "")?;
    let filter = conditions(&request.query, &[], "")?;
    let sql = format!(
        "WITH _w AS (DELETE FROM {table} WHERE {filter} RETURNING *)
         SELECT coalesce(json_agg(_r), '[]')::text, count(*) FROM (SELECT {select} FROM _w) _r"
    );
    let found = request.run(async |tx| rows(tx, &sql, None).await).await?;
    written(request, found, StatusCode::OK)
}

// --- The schema cache ---

/// How long what the cache read of the schema is used: a function or table
/// changed by hand is seen this long after, without a restart.
const SCHEMA_TTL: Duration = Duration::from_secs(60);

/// What PostgREST keeps in its schema cache: functions' arguments and
/// tables' primary keys, read from the catalog once rather than on each
/// request. Only what exists is kept, so made-up names can't fill it.
#[derive(Default)]
pub struct Schema {
    functions: Cached<[Function]>,
    keys: Cached<[String]>,
}

/// What was read for each name, and when.
type Cached<T> = RwLock<HashMap<String, (Instant, Arc<T>)>>;

fn fresh<T: ?Sized>(map: &Cached<T>, name: &str) -> Option<Arc<T>> {
    let map = map.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    map.get(name).filter(|(at, _)| at.elapsed() < SCHEMA_TTL).map(|(_, value)| value.clone())
}

fn keep<T: ?Sized>(map: &Cached<T>, name: &str, value: Arc<T>) {
    let mut map = map.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    map.insert(name.to_owned(), (Instant::now(), value));
}

impl Schema {
    async fn functions(&self, db: &impl GenericClient, name: &str) -> Result<Arc<[Function]>, tokio_postgres::Error> {
        if let Some(found) = fresh(&self.functions, name) {
            return Ok(found);
        }
        let found: Arc<[Function]> = functions(db, name).await?.into();
        if !found.is_empty() {
            keep(&self.functions, name, found.clone());
        }
        Ok(found)
    }

    async fn primary_key(&self, db: &impl GenericClient, table: &str) -> Result<Arc<[String]>, tokio_postgres::Error> {
        if let Some(found) = fresh(&self.keys, table) {
            return Ok(found);
        }
        // An unknown table fails here (regclass), so isn't kept.
        let found: Arc<[String]> = primary_key(db, table).await?.into();
        keep(&self.keys, table, found.clone());
        Ok(found)
    }
}

// --- Functions ---

/// A function in `public`, as PostgREST's schema cache knows it.
#[derive(Debug, Clone)]
struct Function {
    /// Its input arguments: name ("" if unnamed) and type.
    args: Vec<(String, String)>,
    /// How many of the last arguments have defaults.
    defaults: usize,
    returns_set: bool,
    /// Returns rows (a table, a composite or a record), not a single value.
    returns_rows: bool,
    returns_void: bool,
}

impl Function {
    fn required(&self) -> &[(String, String)] {
        &self.args[..self.args.len() - self.defaults.min(self.args.len())]
    }

    fn single_json(&self) -> bool {
        self.args.len() == 1 && matches!(self.args[0].1.as_str(), "json" | "jsonb")
    }
}

async fn functions(db: &impl GenericClient, name: &str) -> Result<Vec<Function>, tokio_postgres::Error> {
    let rows = db
        .query(
            "SELECT coalesce(p.proargnames, '{}')::text[], coalesce(p.proargmodes::text[], '{}'),
                    array(SELECT format_type(t, NULL) FROM unnest(p.proallargtypes) t)::text[],
                    array(SELECT format_type(t, NULL) FROM unnest(p.proargtypes::oid[]) t)::text[],
                    p.pronargdefaults::int4, p.proretset, r.typtype::text, format_type(p.prorettype, NULL)
             FROM pg_proc p
             JOIN pg_namespace n ON n.oid = p.pronamespace
             JOIN pg_type r ON r.oid = p.prorettype
             WHERE n.nspname = 'public' AND p.proname = $1 AND p.prokind = 'f'",
            &[&name],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let names: Vec<String> = row.get(0);
            let modes: Vec<String> = row.get(1);
            let all_types: Vec<String> = row.get(2);
            let in_types: Vec<String> = row.get(3);
            let defaults: i32 = row.get(4);
            let returns_set: bool = row.get(5);
            let kind: String = row.get(6);
            let returns: String = row.get(7);
            let args = if modes.is_empty() {
                in_types
                    .iter()
                    .enumerate()
                    .map(|(at, kind)| (names.get(at).cloned().unwrap_or_default(), kind.clone()))
                    .collect()
            } else {
                modes
                    .iter()
                    .enumerate()
                    .filter(|(_, mode)| matches!(mode.as_str(), "i" | "b" | "v"))
                    .map(|(at, _)| {
                        (names.get(at).cloned().unwrap_or_default(), all_types.get(at).cloned().unwrap_or_default())
                    })
                    .collect()
            };
            Function {
                args,
                defaults: usize::try_from(defaults).unwrap_or(0),
                returns_set,
                returns_rows: kind == "c" || returns == "record",
                returns_void: returns == "void",
            }
        })
        .collect())
}

/// The overload `given` arguments call, as PostgREST picks one.
fn choose<'f>(candidates: &'f [Function], given: &[String], single: bool) -> Option<&'f Function> {
    let mut fitting: Vec<&Function> = candidates
        .iter()
        .filter(|function| {
            if function.single_json() && (single || function.args[0].0.is_empty()) {
                return true;
            }
            given.iter().all(|key| function.args.iter().any(|(name, _)| name == key))
                && function.required().iter().all(|(name, _)| given.contains(name))
        })
        .collect();
    fitting.sort_by_key(|function| function.args.len());
    fitting.first().copied()
}

async fn rpc_route(
    State(state): State<AppState>,
    Extension(bearer): Extension<Bearer>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let head = method == Method::HEAD;
    let result = async {
        let request = request(state, &bearer, headers, method.clone(), &uri)?;
        rpc(&request, &name, &body).await
    }
    .await;
    match result {
        Ok(answer) => answer.response(head),
        Err(error) => error.response(),
    }
}

async fn rpc(request: &Request, name: &str, body: &Bytes) -> Result<Answer, RestError> {
    let reading = request.method == Method::GET || request.method == Method::HEAD;
    let candidates = match fresh(&request.state.schema.functions, name) {
        Some(found) => found,
        None => {
            let client = request.state.db.client().await?;
            request
                .state
                .schema
                .functions(&client, name)
                .await
                .map_err(|error| RestError::database(&error, request.role))?
        }
    };
    // A GET's arguments are its query parameters that name an argument; the rest filter the answer.
    let (arguments, skip): (Value, Vec<String>) = if reading {
        let names: Vec<&str> =
            candidates.iter().flat_map(|function| function.args.iter().map(|(name, _)| name.as_str())).collect();
        let mut map = Map::new();
        for (key, value) in &request.query {
            if names.contains(&key.as_str()) && !RESERVED.contains(&key.as_str()) {
                map.insert(key.clone(), Value::String(value.clone()));
            }
        }
        let skip = map.keys().cloned().collect();
        (Value::Object(map), skip)
    } else {
        (json_body(body)?, Vec::new())
    };
    let given: Vec<String> = match &arguments {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let single = request.prefer.single_object || !arguments.is_object();
    let Some(function) = choose(&candidates, &given, single) else {
        let mut sorted = given.clone();
        sorted.sort();
        let listed = sorted.join(", ");
        return Err(RestError::new(
            StatusCode::NOT_FOUND,
            "PGRST202",
            format!("Could not find the function public.{name}({listed}) in the schema cache"),
        )
        .details(format!(
            "Searched for the function public.{name} with parameter{} {listed} or with a single unnamed json/jsonb parameter, but no matches were found in the schema cache.",
            if sorted.len() == 1 { "" } else { "s" }
        )));
    };
    let function_sql = format!("public.{}", ident(name)?);
    let pass_whole = function.single_json()
        && (single || function.args[0].0.is_empty() || given.len() != 1 || given[0] != function.args[0].0);
    let (from, call) = if pass_whole {
        (String::new(), format!("{function_sql}($1::{})", function.args[0].1))
    } else if given.is_empty() {
        (String::new(), format!("{function_sql}()"))
    } else {
        let mut defs = Vec::new();
        let mut call_args = Vec::new();
        for (arg, kind) in &function.args {
            if given.contains(arg) {
                defs.push(format!("{} {kind}", ident(arg)?));
                call_args.push(format!("{} := _a.{}", ident(arg)?, ident(arg)?));
            }
        }
        (
            format!("json_to_record($1::json) AS _a({}), LATERAL ", defs.join(", ")),
            format!("{function_sql}({})", call_args.join(", ")),
        )
    };
    let uses_param = pass_whole || !given.is_empty();
    let object = wants_object(&request.headers);
    let query = &request.query;

    if function.returns_void {
        let sql = if from.is_empty() {
            format!("SELECT {call}")
        } else {
            format!("SELECT {call} FROM {}", from.trim_end_matches(", LATERAL "))
        };
        request.run(async |tx| typed(tx, &sql, uses_param.then_some(&arguments)).await).await?;
        return Ok(Answer {
            status: StatusCode::NO_CONTENT,
            body: None,
            object: false,
            content_range: None,
            applied: request.prefer.applied.clone(),
        });
    }

    if !function.returns_set && !function.returns_rows {
        let sql = if from.is_empty() {
            format!("SELECT to_json({call})::text")
        } else {
            format!("SELECT to_json({call})::text FROM {}", from.trim_end_matches(", LATERAL "))
        };
        let value: Option<String> =
            request.run(async |tx| Ok(typed(tx, &sql, uses_param.then_some(&arguments)).await?.get(0))).await?;
        return Ok(Answer {
            status: StatusCode::OK,
            body: Some(value.unwrap_or_else(|| "null".into())),
            object: false,
            content_range: Some("0-0/*".into()),
            applied: request.prefer.applied.clone(),
        });
    }

    let source = if function.returns_rows {
        format!("{from}{call} _f")
    } else {
        format!("{from}{call} AS _f({})", ident(name)?)
    };
    let select =
        if function.returns_rows { columns(param(query, "select"), "_f.")? } else { format!("_f.{}", ident(name)?) };
    let filter = conditions(query, &skip, "_f.")?;
    let order = ordering(param(query, "order"), "_f.")?;
    let window = window(query, &request.headers)?;
    let aggregate = if function.returns_rows { "json_agg(_r)" } else { "json_agg(_r.v)" };
    let select = if function.returns_rows { select } else { format!("{select} AS v") };
    let sql = format!(
        "SELECT coalesce({aggregate}, '[]')::text, count(*) FROM (SELECT {select} FROM {source} WHERE {filter}{order}{}) _r",
        window.sql()
    );
    let found = request.run(async |tx| rows(tx, &sql, uses_param.then_some(&arguments)).await).await?;
    if !function.returns_set {
        // One composite value: PostgREST answers with the object itself.
        let array: Value = serde_json::from_str(&found.json).unwrap_or_default();
        return Ok(Answer {
            status: StatusCode::OK,
            body: Some(array.get(0).cloned().unwrap_or(Value::Null).to_string()),
            object: false,
            content_range: Some("0-0/*".into()),
            applied: request.prefer.applied.clone(),
        });
    }
    let range = content_range(window.offset, found.count, None);
    Ok(Answer {
        status: StatusCode::OK,
        body: Some(shape(found, object)?),
        object,
        content_range: Some(range),
        applied: request.prefer.applied.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_follow_postgrest() {
        assert_eq!(status_of("23505", Role::Authenticated), StatusCode::CONFLICT);
        assert_eq!(status_of("42501", Role::Anon), StatusCode::UNAUTHORIZED);
        assert_eq!(status_of("42501", Role::Authenticated), StatusCode::FORBIDDEN);
        assert_eq!(status_of("PT409", Role::Authenticated), StatusCode::CONFLICT);
        assert_eq!(status_of("22023", Role::Authenticated), StatusCode::BAD_REQUEST);
        assert_eq!(status_of("40001", Role::Authenticated), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(status_of("28000", Role::Authenticated), StatusCode::FORBIDDEN);
        assert_eq!(status_of("P0002", Role::Authenticated), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(status_of("42883", Role::Anon), StatusCode::NOT_FOUND);
    }

    #[test]
    fn filters_become_sql() {
        assert_eq!(operation("\"a\"", "eq.1").unwrap(), "\"a\" = '1'");
        assert_eq!(operation("\"a\"", "not.is.null").unwrap(), "NOT (\"a\" IS NULL)");
        assert_eq!(operation("\"a\"", "in.(1,\"x,y\",it's)").unwrap(), "\"a\" IN ('1', 'x,y', 'it''s')");
        assert_eq!(operation("\"a\"", "ilike.*smoke*").unwrap(), "\"a\" ILIKE '%smoke%'");
        assert_eq!(operation("\"a\"", "fts(english).cat").unwrap(), "\"a\" @@ to_tsquery('english', 'cat')");
        assert!(operation("\"a\"", "nope.1").is_err());
        assert_eq!(
            logic("OR", "(a.eq.1,and(b.gt.2,c.is.null))", false, "").unwrap(),
            "(\"a\" = '1' OR (\"b\" > '2' AND \"c\" IS NULL))"
        );
        assert_eq!(field("data->a->>b", "").unwrap(), ("\"data\"->'a'->>'b'".to_owned(), "b".to_owned()));
        assert_eq!(field("x\"y", "_f.").unwrap().0, "_f.\"x\"\"y\"");
    }

    #[test]
    fn select_and_order() {
        assert_eq!(columns(None, "").unwrap(), "*");
        assert_eq!(
            columns(Some("id,label:name,n::text"), "").unwrap(),
            "\"id\" AS \"id\", \"name\" AS \"label\", (\"n\")::text AS \"n\""
        );
        assert!(columns(Some("profiles(*)"), "").is_err());
        assert!(columns(Some("a::text;drop"), "").is_err());
        assert_eq!(ordering(Some("a.desc.nullslast,b"), "").unwrap(), " ORDER BY \"a\" DESC NULLS LAST, \"b\"");
        assert_eq!(content_range(0, 0, None), "*/*");
        assert_eq!(content_range(5, 3, Some(20)), "5-7/20");
    }

    #[test]
    fn overloads() {
        let one = Function {
            args: vec![("p_code".into(), "text".into()), ("p_nonce".into(), "text".into())],
            defaults: 1,
            returns_set: false,
            returns_rows: false,
            returns_void: false,
        };
        let candidates = [one];
        assert!(choose(&candidates, &["p_code".into()], false).is_some());
        assert!(choose(&candidates, &["p_nonce".into()], false).is_none());
        assert!(choose(&candidates, &["p_code".into(), "other".into()], false).is_none());
    }
}
