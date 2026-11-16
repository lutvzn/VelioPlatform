//! The Postgres database, with Nuvio's schema (references/self-host).
//!
//! The server stands in for Supabase's services (Auth, PostgREST, Storage),
//! so it reaches the database as they do: each request runs in one
//! transaction that first switches to the caller's role (anon, authenticated
//! or service_role) and sets `request.jwt.claims`, `request.headers`,
//! `request.method` and `request.path`, as PostgREST sets them. Nuvio's
//! row-level policies, grants and functions then decide what the request
//! may do, exactly as on Nuvio's own server.
//!
//! Migrations, in order, each once:
//!
//! 1. `migrations/0000…0002`: what Supabase's image and services make before
//!    Nuvio's migrations (roles, Auth's schema, Storage's tables). Skipped on
//!    a database where Auth or Storage already made them.
//! 2. `migrations/nuvio/*.sql`: Nuvio's own, unchanged, recorded in
//!    `nuvio_migrations.schema_migrations` as Nuvio's `migrate` records them.
//! 3. `migrations/0003…`: Velio's additions.

use anyhow::{Context, Result, bail};
use deadpool_postgres::{GenericClient, Manager, ManagerConfig, Pool, RecyclingMethod, Transaction};
use serde_json::{Value, json};
use tokio_postgres::types::ToSql;
use tokio_postgres::{NoTls, Row};

/// What Supabase makes before Nuvio's migrations.
const BEFORE_NUVIO: &[(&str, &str)] = &[
    ("0000_supabase_roles", include_str!("../migrations/0000_supabase_roles.sql")),
    ("0001_gotrue", include_str!("../migrations/0001_gotrue.sql")),
    ("0002_storage", include_str!("../migrations/0002_storage.sql")),
];

/// Nuvio's, as copied by scripts/supabase-migrations.mjs. Run in name order;
/// the version is the name up to the first `_`.
#[derive(rust_embed::RustEmbed)]
#[folder = "migrations/nuvio/"]
#[include = "*.sql"]
struct Nuvio;

/// Velio's, after Nuvio's.
const AFTER_NUVIO: &[(&str, &str)] = &[
    ("0003_velio", include_str!("../migrations/0003_velio.sql")),
    ("0004_faster_pulls", include_str!("../migrations/0004_faster_pulls.sql")),
];

#[derive(Clone)]
pub struct Db {
    pool: Pool,
}

pub type Client = deadpool_postgres::Object;

/// The role a request runs as, as Supabase's keys and tokens give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Anon,
    Authenticated,
    ServiceRole,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Anon => "anon",
            Role::Authenticated => "authenticated",
            Role::ServiceRole => "service_role",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "anon" => Some(Role::Anon),
            "authenticated" => Some(Role::Authenticated),
            "service_role" => Some(Role::ServiceRole),
            _ => None,
        }
    }
}

/// Who a request runs as, and what of it Nuvio's SQL reads.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub role: Role,
    /// The token's claims: `auth.uid()` and `auth.jwt()` read them.
    pub claims: Value,
    /// The request's headers, names in lower case, as PostgREST sets them.
    pub headers: Value,
    pub method: String,
    pub path: String,
    /// As PostgREST runs GET and HEAD: a write fails with 25006.
    pub read_only: bool,
}

impl Ctx {
    /// The server itself, for its own work (the website's handlers act as the user instead).
    pub fn service() -> Self {
        Self::new(Role::ServiceRole, json!({ "role": "service_role" }))
    }

    pub fn new(role: Role, claims: Value) -> Self {
        Self { role, claims, headers: json!({}), method: "POST".into(), path: "/".into(), read_only: false }
    }

    /// The signed-in user, as their access token's claims say.
    pub fn user(claims: Value) -> Self {
        Self::new(Role::Authenticated, claims)
    }
}

impl Db {
    /// A pool of at most `connections` to the database at `url`.
    pub fn connect(url: &str, connections: usize) -> Result<Self> {
        let config: tokio_postgres::Config = url.parse().context("DATABASE_URL must be a Postgres URL")?;
        let manager = Manager::from_config(config, NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast });
        let pool = Pool::builder(manager).max_size(connections).build().context("Making the database pool")?;
        Ok(Self { pool })
    }

    pub async fn client(&self) -> Result<Client> {
        self.pool.get().await.context("Connecting to the database")
    }

    /// Runs every migration not yet run. One server at a time, by an advisory lock.
    pub async fn migrate(&self) -> Result<()> {
        let mut client = self.client().await?;
        client.batch_execute("SELECT pg_advisory_lock(hashtext('velio_migrations'))").await?;
        let result = migrate(&mut client).await;
        client.batch_execute("SELECT pg_advisory_unlock(hashtext('velio_migrations'))").await?;
        // Nuvio's baseline is a pg_dump, which sets row_security, search_path and
        // more for the whole session: this connection never serves a request.
        drop(deadpool_postgres::Object::take(client));
        result
    }
}

/// A transaction as `ctx`: Nuvio's policies and functions see its role and claims.
pub async fn begin<'a>(client: &'a mut Client, ctx: &Ctx) -> Result<Transaction<'a>, tokio_postgres::Error> {
    let tx = client.build_transaction().read_only(ctx.read_only).start().await?;
    tx.exec(
        "SELECT set_config('role', $1, true), set_config('request.jwt.claims', $2, true),
                set_config('request.headers', $3, true), set_config('request.method', $4, true),
                set_config('request.path', $5, true), set_config('search_path', 'public, extensions', true),
                set_config('standard_conforming_strings', 'on', true)",
        &[&ctx.role.name(), &ctx.claims.to_string(), &ctx.headers.to_string(), &ctx.method, &ctx.path],
    )
    .await?;
    Ok(tx)
}

type Params<'a> = &'a [&'a (dyn ToSql + Sync)];

/// Fixed SQL, as a statement each connection prepares once and keeps
/// (deadpool's statement cache). Postgres then neither parses nor, mostly,
/// plans it again, and a call is one round trip instead of two. Only for SQL
/// written in the server: SQL that varies with a request (PostgREST's) would
/// fill the cache.
pub trait Prepared: GenericClient {
    fn exec(&self, sql: &str, params: Params<'_>) -> impl Future<Output = Result<u64, tokio_postgres::Error>> + Send;
    fn one(&self, sql: &str, params: Params<'_>) -> impl Future<Output = Result<Row, tokio_postgres::Error>> + Send;
    fn opt(
        &self,
        sql: &str,
        params: Params<'_>,
    ) -> impl Future<Output = Result<Option<Row>, tokio_postgres::Error>> + Send;
    fn all(
        &self,
        sql: &str,
        params: Params<'_>,
    ) -> impl Future<Output = Result<Vec<Row>, tokio_postgres::Error>> + Send;
}

impl<C: GenericClient> Prepared for C {
    async fn exec(&self, sql: &str, params: Params<'_>) -> Result<u64, tokio_postgres::Error> {
        self.execute(&self.prepare_cached(sql).await?, params).await
    }

    async fn one(&self, sql: &str, params: Params<'_>) -> Result<Row, tokio_postgres::Error> {
        self.query_one(&self.prepare_cached(sql).await?, params).await
    }

    async fn opt(&self, sql: &str, params: Params<'_>) -> Result<Option<Row>, tokio_postgres::Error> {
        self.query_opt(&self.prepare_cached(sql).await?, params).await
    }

    async fn all(&self, sql: &str, params: Params<'_>) -> Result<Vec<Row>, tokio_postgres::Error> {
        self.query(&self.prepare_cached(sql).await?, params).await
    }
}

async fn migrate(client: &mut Client) -> Result<()> {
    client
        .batch_execute(
            "CREATE SCHEMA IF NOT EXISTS velio_private;
             CREATE TABLE IF NOT EXISTS velio_private.schema_migrations (
                 version text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())",
        )
        .await?;
    async fn done(client: &Client, version: &str) -> Result<bool> {
        let row =
            client.query_opt("SELECT 1 FROM velio_private.schema_migrations WHERE version = $1", &[&version]).await?;
        Ok(row.is_some())
    }

    for (version, sql) in BEFORE_NUVIO {
        if done(client, version).await? {
            continue;
        }
        // A database where Supabase's services already ran keeps what they made.
        let made_by = match *version {
            "0001_gotrue" => Some("auth.users"),
            "0002_storage" => Some("storage.objects"),
            _ => None,
        };
        let exists = match made_by {
            Some(table) => client.query_one("SELECT to_regclass($1) IS NOT NULL", &[&table]).await?.get::<_, bool>(0),
            None => false,
        };
        run_velio(client, version, if exists { "" } else { sql }).await?;
    }

    let mut names: Vec<_> = Nuvio::iter().collect();
    names.sort();
    for name in names {
        let version = name.split('_').next().unwrap_or_default();
        let file = Nuvio::get(&name).context("An embedded migration")?;
        let sql = std::str::from_utf8(&file.data).context("A migration is UTF-8")?;
        let applied: bool = client
            .query_one(
                "SELECT to_regclass('nuvio_migrations.schema_migrations') IS NOT NULL
                   AND EXISTS (SELECT 1 FROM nuvio_migrations.schema_migrations WHERE version = $1)",
                &[&version],
            )
            .await
            // The table doesn't exist before the baseline, so the EXISTS fails then.
            .map(|row| row.get(0))
            .unwrap_or(false);
        if applied {
            continue;
        }
        // Each file has its own BEGIN and COMMIT, and records itself.
        if let Err(error) = client.batch_execute(sql).await {
            let _ = client.batch_execute("ROLLBACK").await;
            return Err(error).with_context(|| format!("Nuvio's migration {version}"));
        }
        // What a pg_dump file SETs for its session doesn't carry on to the next file.
        client.batch_execute("RESET ALL").await?;
        tracing::info!("Applied Nuvio's migration {version}");
    }

    for (version, sql) in AFTER_NUVIO {
        if !done(client, version).await? {
            run_velio(client, version, sql).await?;
        }
    }
    Ok(())
}

async fn run_velio(client: &mut Client, version: &str, sql: &str) -> Result<()> {
    let tx = client.transaction().await?;
    tx.batch_execute(sql).await.with_context(|| format!("Migration {version}"))?;
    tx.execute("INSERT INTO velio_private.schema_migrations (version) VALUES ($1)", &[&version]).await?;
    tx.commit().await?;
    if sql.is_empty() {
        tracing::info!("Migration {version} was made already");
    } else {
        tracing::info!("Applied migration {version}");
    }
    Ok(())
}

/// A row's one jsonb column, as most queries here answer.
pub fn json_of(row: &tokio_postgres::Row) -> Value {
    row.get::<_, Option<Value>>(0).unwrap_or(Value::Null)
}

/// Refuses a database that isn't Postgres 15 or newer, which Nuvio's schema needs.
pub async fn check_version(client: &Client) -> Result<()> {
    let version: i32 = client.query_one("SELECT current_setting('server_version_num')::int", &[]).await?.get(0);
    if version < 150_000 {
        bail!("The database is Postgres {}; Nuvio's schema needs 15 or newer", version / 10_000);
    }
    Ok(())
}
