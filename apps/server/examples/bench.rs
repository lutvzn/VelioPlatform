//! The server's benchmark: how fast it answers, and how much memory and
//! space it takes (README, "Benchmark").
//!
//!   cargo build --release && cargo run --release --example bench -- [options]
//!
//! It makes a throwaway Postgres cluster (initdb and postgres on PATH, or in
//! PG_BIN), starts the release binary on it, signs up accounts and fills
//! them as Nuvio's apps and VelioTV do (profiles, library, watch progress,
//! IPTV lists, a TV), then runs each scenario at a fixed concurrency. It
//! prints a table, writes the numbers as JSON, and compares them with a
//! baseline when given one.
//!
//!   --accounts N        accounts to make (100)
//!   --seconds S         seconds each scenario runs, after a second's warm-up (8)
//!   --concurrency C     requests in flight (32)
//!   --only a,b          only these scenarios
//!   --out FILE          where the JSON goes (target/bench/latest.json)
//!   --baseline FILE     a JSON file from an earlier run, to compare with
//!   --binary FILE       the server (target/release/velio-server)
//!   --pool N            VELIO_DB_POOL for the server, if set
//!   --statements        load pg_stat_statements: each scenario also gives the
//!                       statements a request runs, and the costliest of them
//!   --postgres SETTING  a setting for Postgres, as name=value (repeatable);
//!                       its log then goes to target/bench/postgres.log, e.g.
//!                       --postgres session_preload_libraries=auto_explain
//!                       --postgres auto_explain.log_min_duration=0
//!
//! Postgres runs with its defaults, but synchronous_commit=off, so a write's
//! time is the server's and Postgres's work rather than this disk's flushes.
//! The load comes from this process, on the same machine: compare runs on
//! one machine, not across machines.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const SITE: &str = "https://velio.tv";
const PASSWORD: &str = "bench-password-1";

#[derive(Debug, Clone)]
struct Options {
    accounts: usize,
    seconds: u64,
    concurrency: usize,
    only: Option<Vec<String>>,
    out: PathBuf,
    baseline: Option<PathBuf>,
    binary: PathBuf,
    pool: Option<String>,
    statements: bool,
    postgres: Vec<String>,
}

fn options() -> Result<Options> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut options = Options {
        accounts: 100,
        seconds: 8,
        concurrency: 32,
        only: None,
        out: root.join("target/bench/latest.json"),
        baseline: None,
        binary: root.join("target/release/velio-server"),
        pool: None,
        statements: false,
        postgres: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--accounts" => options.accounts = value()?.parse()?,
            "--seconds" => options.seconds = value()?.parse()?,
            "--concurrency" => options.concurrency = value()?.parse()?,
            "--only" => options.only = Some(value()?.split(',').map(str::to_owned).collect()),
            "--out" => options.out = value()?.into(),
            "--baseline" => options.baseline = Some(value()?.into()),
            "--binary" => options.binary = value()?.into(),
            "--pool" => options.pool = Some(value()?),
            "--statements" => options.statements = true,
            "--postgres" => options.postgres.push(value()?),
            _ => bail!("Unknown option {arg}; see the top of examples/bench.rs"),
        }
    }
    if options.accounts < options.concurrency {
        bail!("--accounts must be at least --concurrency, so each request has an account of its own");
    }
    Ok(options)
}

// --- Processes: CPU time and memory, from /proc ---

/// Clock ticks a second, as /proc counts CPU time (USER_HZ, 100 on Linux).
const TICKS: f64 = 100.0;

/// Fields of /proc/<pid>/stat after the command's name: state is 0.
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 2..];
    Some(after.split_whitespace().map(str::to_owned).collect())
}

/// CPU seconds a process used, with its children that have stopped.
fn cpu_seconds(pid: u32) -> f64 {
    let Some(fields) = stat_fields(pid) else { return 0.0 };
    // utime, stime, cutime, cstime: fields 14-17 of stat, 11-14 here.
    let ticks: f64 = fields[11..15].iter().filter_map(|field| field.parse::<f64>().ok()).sum();
    ticks / TICKS
}

/// The processes whose parent is `pid`.
fn children(pid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|child| stat_fields(*child).and_then(|fields| fields.get(1)?.parse::<u32>().ok()) == Some(pid))
        .collect()
}

/// A line of /proc/<pid>/<file>, in kB, such as VmRSS of status.
fn kb(pid: u32, file: &str, name: &str) -> u64 {
    let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/{file}")) else { return 0 };
    text.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

/// Postgres's processes: the postmaster and its children.
fn postgres_pids(postmaster: u32) -> Vec<u32> {
    let mut pids = children(postmaster);
    pids.push(postmaster);
    pids
}

/// Memory shared between Postgres's processes counts once: the sum of their PSS.
fn postgres_pss_kb(postmaster: u32) -> u64 {
    postgres_pids(postmaster).into_iter().map(|pid| kb(pid, "smaps_rollup", "Pss")).sum()
}

fn postgres_cpu(postmaster: u32) -> f64 {
    postgres_pids(postmaster).into_iter().map(cpu_seconds).sum()
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => dir_bytes(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

// --- Postgres and the server ---

fn pg_bin(name: &str) -> PathBuf {
    std::env::var_os("PG_BIN").map_or_else(|| name.into(), |dir| Path::new(&dir).join(name))
}

fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

struct Cluster {
    process: Child,
    url: String,
}

impl Cluster {
    fn start(dir: &Path, options: &Options) -> Result<Self> {
        let data = dir.join("pg");
        let made = Command::new(pg_bin("initdb"))
            .args(["-D"])
            .arg(&data)
            .args(["-U", "postgres", "--auth=trust", "-E", "UTF8", "--locale=C", "--no-sync"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .context("Running initdb (on PATH, or in PG_BIN)")?;
        if !made.success() {
            bail!("initdb failed");
        }
        let port = free_port()?;
        let mut command = Command::new(pg_bin("postgres"));
        command.arg("-D").arg(&data).args(["-p", &port.to_string(), "-k"]).arg(dir).args([
            "-c",
            "listen_addresses=127.0.0.1",
            "-c",
            "synchronous_commit=off",
        ]);
        if options.statements {
            command.args(["-c", "shared_preload_libraries=pg_stat_statements", "-c", "pg_stat_statements.track=all"]);
            command.args(["-c", "pg_stat_statements.track_planning=on"]);
        }
        let log = if options.postgres.is_empty() {
            Stdio::null()
        } else {
            // To stderr, unless a setting says otherwise.
            command.args(["-c", "logging_collector=off"]);
            for setting in &options.postgres {
                command.args(["-c", setting]);
            }
            let path = options.out.with_file_name("postgres.log");
            std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
            std::fs::File::create(&path).with_context(|| format!("Making {}", path.display()))?.into()
        };
        let process = command.stdout(Stdio::null()).stderr(log).spawn().context("Starting postgres")?;
        let url = format!("postgres://postgres@127.0.0.1:{port}/postgres");
        let cluster = Self { process, url };
        for _ in 0..100 {
            if cluster.psql("SELECT 1").is_ok() {
                return Ok(cluster);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("Postgres didn't start")
    }

    fn pid(&self) -> u32 {
        self.process.id()
    }

    /// One value, as psql prints it.
    fn psql(&self, sql: &str) -> Result<String> {
        let out = Command::new(pg_bin("psql"))
            .args([&self.url, "-XAtq", "-v", "ON_ERROR_STOP=1", "-c", sql])
            .stderr(Stdio::piped())
            .output()?;
        if !out.status.success() {
            bail!("psql: {}", String::from_utf8_lossy(&out.stderr));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    fn number(&self, sql: &str) -> Result<i64> {
        self.psql(sql)?.parse().with_context(|| format!("A number from {sql}"))
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        // SIGINT is Postgres's fast shutdown.
        let _ = Command::new("kill").args(["-INT", &self.pid().to_string()]).status();
        let _ = self.process.wait();
    }
}

struct Server {
    process: Child,
    url: String,
    started_ms: f64,
}

impl Server {
    fn start(options: &Options, database_url: &str, dir: &Path) -> Result<Self> {
        let started = Instant::now();
        let mut command = Command::new(&options.binary);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .env("DEV_MODE", "1")
            .env("VELIO_SITE_URL", SITE)
            .env("VELIO_LISTEN", "127.0.0.1:0")
            .env("DATABASE_URL", database_url)
            .env("VELIO_STORAGE_DIR", dir.join("storage"))
            .env("VELIO_TRUSTED_PROXY", "127.0.0.1")
            .env("JWT_SECRET", "bench-jwt-secret-of-at-least-32-characters")
            .env("ENABLE_EMAIL_AUTOCONFIRM", "true")
            .env("OPS_TOKEN", "bench-ops-token")
            .env("EMAIL_FROM", "Velio <no-reply@velio.tv>")
            .env("ABUSE_EMAIL", "abuse@velio.tv")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(pool) = &options.pool {
            command.env("VELIO_DB_POOL", pool);
        }
        let mut process = command.spawn().with_context(|| format!("Starting {}", options.binary.display()))?;
        let stdout = process.stdout.take().context("The server's output")?;
        let (found, listening) = std::sync::mpsc::channel();
        // The log is read all along, as a container runtime would, so the server never waits on the pipe.
        std::thread::spawn(move || {
            let mut found = Some(found);
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(url) = line.split("Listening on ").nth(1)
                    && let Some(found) = found.take()
                {
                    let _ = found.send(url.trim().to_owned());
                }
            }
        });
        let url = listening.recv_timeout(Duration::from_secs(120)).context("The server didn't start")?;
        Ok(Self { process, url, started_ms: started.elapsed().as_secs_f64() * 1000.0 })
    }

    fn pid(&self) -> u32 {
        self.process.id()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("kill").args(["-TERM", &self.pid().to_string()]).status();
        let _ = self.process.wait();
    }
}

// --- Requests ---

/// What every request needs: the server, its key, and a client.
#[derive(Clone)]
struct Api {
    http: reqwest::Client,
    base: String,
    key: String,
}

/// An address of its own for each request that's rate limited by address.
static NEXT_IP: AtomicU64 = AtomicU64::new(1);

fn fresh_ip() -> String {
    let n = NEXT_IP.fetch_add(1, Ordering::Relaxed);
    format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255)
}

impl Api {
    fn request(&self, method: reqwest::Method, path: &str, token: Option<&str>) -> reqwest::RequestBuilder {
        let builder = self.http.request(method, format!("{}{path}", self.base)).header("apikey", &self.key);
        match token {
            Some(token) => builder.bearer_auth(token),
            None => builder.bearer_auth(&self.key),
        }
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<(u16, Bytes)> {
        let response = builder.send().await?;
        let status = response.status().as_u16();
        Ok((status, response.bytes().await?.to_vec()))
    }

    async fn rpc(&self, name: &str, token: &str, body: &Value) -> Result<(u16, Bytes)> {
        self.send(self.request(reqwest::Method::POST, &format!("/rest/v1/rpc/{name}"), Some(token)).json(body)).await
    }
}

type Bytes = Vec<u8>;

/// What an account has, as its app keeps it.
#[derive(Debug, Clone)]
struct Account {
    email: String,
    access: String,
    refresh: String,
}

fn tokens_of(body: &[u8]) -> Result<(String, String)> {
    let value: Value = serde_json::from_slice(body)?;
    let access = value["access_token"].as_str().context("An access token")?.to_owned();
    let refresh = value["refresh_token"].as_str().context("A refresh token")?.to_owned();
    Ok((access, refresh))
}

fn ok(what: &str, (status, body): (u16, Bytes)) -> Result<Bytes> {
    if !(200..300).contains(&status) {
        bail!("{what}: {status} {}", String::from_utf8_lossy(&body));
    }
    Ok(body)
}

fn library_items(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| {
                json!({
                    "content_id": format!("tt{:07}", 1_000_000 + i),
                    "content_type": if i % 3 == 0 { "series" } else { "movie" },
                    "name": format!("A made-up title {i}"),
                    "poster": format!("https://images.example.invalid/poster/{i}.jpg"),
                    "background": format!("https://images.example.invalid/background/{i}.jpg"),
                    "description": "A film made up for the benchmark, with a line or two of text as add-ons give.",
                    "release_info": "2024",
                    "imdb_rating": 7.1,
                    "genres": ["Drama", "Comedy"],
                    "addon_base_url": "https://addon.example.invalid",
                    "added_at": 1_760_000_000_000_i64 + i as i64,
                })
            })
            .collect(),
    )
}

fn progress_entries(n: usize, at: i64) -> Value {
    Value::Array(
        (0..n)
            .map(|i| {
                let series = i % 2 == 0;
                json!({
                    "content_id": format!("tt{:07}", 2_000_000 + i),
                    "content_type": if series { "series" } else { "movie" },
                    "video_id": if series { format!("tt{:07}:1:{}", 2_000_000 + i, i % 10 + 1) } else { format!("tt{:07}", 2_000_000 + i) },
                    "season": if series { Some(1) } else { None },
                    "episode": if series { Some(i % 10 + 1) } else { None },
                    "position": 600_000 + i * 1000,
                    "duration": 3_600_000,
                    "last_watched": at - i as i64 * 60_000,
                })
            })
            .collect(),
    )
}

/// Signs an account up and fills it as a Nuvio app and a Velio TV would.
async fn make_account(api: &Api, n: usize) -> Result<Account> {
    let email = format!("bench{n}@example.invalid");
    let body = ok(
        "Sign-up",
        api.send(
            api.request(reqwest::Method::POST, "/auth/v1/signup", None)
                .header("x-forwarded-for", fresh_ip())
                .json(&json!({ "email": email, "password": PASSWORD })),
        )
        .await?,
    )?;
    let (access, refresh) = tokens_of(&body)?;
    let profiles = json!({ "p_profiles": [
        { "profile_index": 1, "name": "Living room", "avatar_color_hex": "#1E88E5" },
        { "profile_index": 2, "name": "Kids", "avatar_color_hex": "#43A047" },
    ] });
    ok("Profiles", api.rpc("sync_push_profiles", &access, &profiles).await?)?;
    for profile in [1, 2] {
        let items = json!({ "p_items": library_items(150), "p_profile_id": profile });
        ok("Library", api.rpc("sync_push_library", &access, &items).await?)?;
        let entries = json!({ "p_entries": progress_entries(80, 1_770_000_000_000), "p_profile_id": profile });
        ok("Watch progress", api.rpc("sync_push_watch_progress", &access, &entries).await?)?;
    }
    let lists = json!([
        { "id": "news", "name": "News", "kind": "m3u", "url": "https://lists.example.invalid/news.m3u",
          "server": null, "username": null, "password": null, "profile_ids": [], "sort_order": 0 },
        { "id": "sport", "name": "Sport", "kind": "m3u", "url": "https://lists.example.invalid/sport.m3u",
          "server": null, "username": null, "password": null, "profile_ids": [1], "sort_order": 1 },
        { "id": "box", "name": "Box", "kind": "xtream", "url": null, "server": "https://xtream.example.invalid",
          "username": "u", "password": "p", "profile_ids": [], "sort_order": 2 },
    ]);
    // As in PostgREST, a key missing from one row is null in it: each row has them all.
    ok(
        "IPTV lists",
        api.send(
            api.request(reqwest::Method::POST, "/rest/v1/velio_iptv_lists", Some(&access))
                .header("prefer", "return=minimal")
                .json(&lists),
        )
        .await?,
    )?;
    let device = json!({
        "p_installation_id": format!("bench-installation-{n:08}"), "p_client_name": "Nuvio TV",
        "p_platform": "tv", "p_device_name": "Living room TV",
    });
    ok("Device", api.rpc("register_current_device", &access, &device).await?)?;
    Ok(Account { email, access, refresh })
}

// --- Scenarios ---

const SCENARIOS: &[(&str, &str)] = &[
    ("discovery", "GET /.well-known/nuvio, no database"),
    ("tv_pull", "VelioTV's pull: sync_pull_profiles, addons, velio_iptv_lists (3 requests)"),
    ("pull_profiles", "rpc/sync_pull_profiles"),
    ("pull_library", "rpc/sync_pull_library, 150 items"),
    ("pull_progress", "rpc/sync_pull_watch_progress, 80 entries"),
    ("push_progress", "rpc/sync_push_watch_progress, one entry"),
    ("rest_addons", "GET /rest/v1/addons, as the TV asks"),
    ("rest_upsert", "POST /rest/v1/velio_iptv_lists, an upsert of one list"),
    ("auth_user", "GET /auth/v1/user"),
    ("refresh", "POST /auth/v1/token?grant_type=refresh_token"),
    ("password", "POST /auth/v1/token?grant_type=password (bcrypt)"),
    ("web_home", "GET /, the site's home page"),
];

const ADDONS: &str = "/rest/v1/addons?select=id,url,name,enabled,sort_order,profile_id&order=sort_order.asc";
const LISTS: &str = "/rest/v1/velio_iptv_lists?select=id,name,kind,url,server,username,password,guide_urls,refresh_hours,disabled,profile_ids&order=sort_order.asc,created_at.asc";

/// One step of a scenario: its requests' statuses.
async fn step(api: &Api, scenario: &str, account: &mut Account, n: u64, latencies: &mut Vec<u32>) -> Result<u64> {
    let mut failed = 0;
    let mut timed = async |builder: reqwest::RequestBuilder| -> Result<()> {
        let started = Instant::now();
        let (status, _) = api.send(builder).await?;
        latencies.push(u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX));
        if !(200..300).contains(&status) {
            failed += 1;
        }
        Ok(())
    };
    let post = reqwest::Method::POST;
    let get = reqwest::Method::GET;
    let token = account.access.clone();
    match scenario {
        "discovery" => timed(api.http.get(format!("{}/.well-known/nuvio", api.base))).await?,
        "tv_pull" => {
            timed(api.request(post, "/rest/v1/rpc/sync_pull_profiles", Some(&token)).json(&json!({}))).await?;
            timed(api.request(get.clone(), ADDONS, Some(&token))).await?;
            timed(api.request(get, LISTS, Some(&token))).await?;
        }
        "pull_profiles" => {
            timed(api.request(post, "/rest/v1/rpc/sync_pull_profiles", Some(&token)).json(&json!({}))).await?
        }
        "pull_library" => {
            timed(api.request(post, "/rest/v1/rpc/sync_pull_library", Some(&token)).json(&json!({ "p_profile_id": 1 })))
                .await?
        }
        "pull_progress" => {
            timed(
                api.request(post, "/rest/v1/rpc/sync_pull_watch_progress", Some(&token))
                    .json(&json!({ "p_profile_id": 1 })),
            )
            .await?
        }
        "push_progress" => {
            // Later each time, by more than Nuvio's minimum gaps, so each push writes.
            let at = 1_780_000_000_000 + i64::try_from(n).unwrap_or(0) * 120_000;
            let entries = progress_entries(1, at);
            timed(
                api.request(post, "/rest/v1/rpc/sync_push_watch_progress", Some(&token))
                    .json(&json!({ "p_entries": entries, "p_profile_id": 1 })),
            )
            .await?
        }
        "rest_addons" => timed(api.request(get, ADDONS, Some(&token))).await?,
        "rest_upsert" => {
            let list = json!({ "id": "upserted", "name": format!("Upserted {n}"), "kind": "m3u",
                               "url": "https://lists.example.invalid/upserted.m3u", "profile_ids": [] });
            timed(
                api.request(post, "/rest/v1/velio_iptv_lists?on_conflict=user_id,id", Some(&token))
                    .header("prefer", "resolution=merge-duplicates,return=minimal")
                    .json(&list),
            )
            .await?
        }
        "auth_user" => timed(api.request(get, "/auth/v1/user", Some(&token))).await?,
        "refresh" => {
            let started = Instant::now();
            let (status, body) = api
                .send(
                    api.request(post, "/auth/v1/token?grant_type=refresh_token", None)
                        .json(&json!({ "refresh_token": account.refresh })),
                )
                .await?;
            latencies.push(u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX));
            match tokens_of(&body) {
                Ok((access, refresh)) if status == 200 => {
                    account.access = access;
                    account.refresh = refresh;
                }
                _ => failed += 1,
            }
        }
        "password" => {
            timed(
                api.request(post, "/auth/v1/token?grant_type=password", None)
                    .header("x-forwarded-for", fresh_ip())
                    .json(&json!({ "email": account.email, "password": PASSWORD })),
            )
            .await?
        }
        "web_home" => timed(api.http.get(format!("{}/", api.base))).await?,
        _ => bail!("No scenario {scenario}"),
    }
    Ok(failed)
}

#[derive(Debug, Default)]
struct Ran {
    requests: u64,
    failed: u64,
    seconds: f64,
    latencies: Vec<u32>,
}

/// Runs a scenario for `seconds` at `concurrency`, each worker with accounts of its own.
async fn run(api: &Api, scenario: &str, accounts: &[Account], concurrency: usize, seconds: f64) -> Result<Ran> {
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    let started = Instant::now();
    let workers = (0..concurrency).map(|worker| {
        let api = api.clone();
        let scenario = scenario.to_owned();
        let mut mine: Vec<Account> = accounts.iter().skip(worker).step_by(concurrency).cloned().collect();
        tokio::spawn(async move {
            let mut ran = Ran::default();
            let mut n = 0u64;
            while Instant::now() < until {
                let at = usize::try_from(n).unwrap_or(0) % mine.len();
                let before = ran.latencies.len();
                ran.failed +=
                    step(&api, &scenario, &mut mine[at], n * 1000 + worker as u64, &mut ran.latencies).await?;
                ran.requests += (ran.latencies.len() - before) as u64;
                n += 1;
            }
            anyhow::Ok(ran)
        })
    });
    let mut total = Ran::default();
    for worker in futures_util::future::join_all(workers).await {
        let ran = worker??;
        total.requests += ran.requests;
        total.failed += ran.failed;
        total.latencies.extend(ran.latencies);
    }
    total.seconds = started.elapsed().as_secs_f64();
    total.latencies.sort_unstable();
    Ok(total)
}

fn percentile(sorted: &[u32], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() - 1) as f64 * p).round() as usize;
    f64::from(sorted[at]) / 1000.0
}

// --- The run ---

fn main() -> Result<()> {
    let options = options()?;
    if !options.binary.exists() {
        bail!("No {}: run `cargo build --release` first", options.binary.display());
    }
    let dir = std::env::temp_dir().join(format!("velio-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let result = bench(&options, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    let result = result?;

    if let Some(parent) = options.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&options.out, serde_json::to_string_pretty(&result)? + "\n")?;
    let baseline = match &options.baseline {
        Some(path) => Some(serde_json::from_str::<Value>(&std::fs::read_to_string(path)?)?),
        None => None,
    };
    print_report(&result, baseline.as_ref());
    eprintln!("\nWritten to {}", options.out.display());
    Ok(())
}

fn bench(options: &Options, dir: &Path) -> Result<Value> {
    let cluster = Cluster::start(dir, options)?;
    eprintln!("Postgres at {}", cluster.url);
    let empty_db = cluster.number("SELECT pg_database_size(current_database())")?;
    let server = Server::start(options, &cluster.url, dir)?;
    eprintln!("Server at {} after {:.0} ms (migrations included)", server.url, server.started_ms);
    let migrated_db = cluster.number("SELECT pg_database_size(current_database())")?;
    if options.statements {
        cluster.psql("CREATE EXTENSION IF NOT EXISTS pg_stat_statements WITH SCHEMA extensions")?;
    }
    std::thread::sleep(Duration::from_millis(500));
    let idle_rss = kb(server.pid(), "status", "VmRSS");
    let idle_pg = postgres_pss_kb(cluster.pid());

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let mut scenarios = BTreeMap::new();
    let (seeding, accounts) = runtime.block_on(async {
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(options.concurrency * 2)
            .timeout(Duration::from_secs(60))
            .build()?;
        let discovery: Value = http.get(format!("{}/.well-known/nuvio", server.url)).send().await?.json().await?;
        let key = discovery["publishable_key"].as_str().context("The publishable key")?.to_owned();
        let api = Api { http, base: server.url.clone(), key };

        use futures_util::StreamExt;
        let started = Instant::now();
        let mut accounts = Vec::new();
        {
            let mut made = futures_util::stream::iter(0..options.accounts)
                .map(|n| {
                    let api = api.clone();
                    async move { make_account(&api, n).await }
                })
                .buffer_unordered(8);
            while let Some(account) = made.next().await {
                accounts.push(account?);
            }
        }
        let seeding = started.elapsed().as_secs_f64();
        eprintln!("Made {} accounts in {seeding:.1} s", accounts.len());
        accounts.sort_by(|a, b| a.email.cmp(&b.email));
        anyhow::Ok((seeding, (api, accounts)))
    })?;
    let (api, accounts) = accounts;
    let filled_db = cluster.number("SELECT pg_database_size(current_database())")?;
    let after_seed_rss = kb(server.pid(), "status", "VmRSS");

    for (name, about) in SCENARIOS {
        if options.only.as_ref().is_some_and(|only| !only.iter().any(|o| o == name)) {
            continue;
        }
        // bcrypt costs a CPU each: fewer in flight, as clients sign in rarely.
        let concurrency = if *name == "password" { options.concurrency.min(8) } else { options.concurrency };
        runtime.block_on(run(&api, name, &accounts, concurrency, 1.0))?;
        if options.statements {
            cluster.psql("SELECT extensions.pg_stat_statements_reset()")?;
        }
        let (server_cpu, pg_cpu) = (cpu_seconds(server.pid()), postgres_cpu(cluster.pid()));
        let ran = runtime.block_on(run(&api, name, &accounts, concurrency, options.seconds as f64))?;
        let server_cpu = cpu_seconds(server.pid()) - server_cpu;
        let pg_cpu = postgres_cpu(cluster.pid()) - pg_cpu;
        let per_k = |cpu: f64| if ran.requests == 0 { 0.0 } else { cpu * 1000.0 * 1000.0 / ran.requests as f64 };
        let mut row = json!({
            "about": about,
            "concurrency": concurrency,
            "requests": ran.requests,
            "failed": ran.failed,
            "rps": ran.requests as f64 / ran.seconds,
            "p50_ms": percentile(&ran.latencies, 0.50),
            "p90_ms": percentile(&ran.latencies, 0.90),
            "p99_ms": percentile(&ran.latencies, 0.99),
            "max_ms": percentile(&ran.latencies, 1.0),
            "server_cpu_ms_per_1k": per_k(server_cpu),
            "postgres_cpu_ms_per_1k": per_k(pg_cpu),
            "server_rss_kb": kb(server.pid(), "status", "VmRSS"),
        });
        if options.statements {
            row["statements"] = statements(&cluster, ran.requests)?;
        }
        eprintln!(
            "{name:>14}: {:>8.0} req/s, p50 {:.2} ms, p99 {:.2} ms, {} failed",
            row["rps"].as_f64().unwrap_or_default(),
            row["p50_ms"].as_f64().unwrap_or_default(),
            row["p99_ms"].as_f64().unwrap_or_default(),
            ran.failed
        );
        scenarios.insert((*name).to_owned(), row);
    }

    let connections = cluster.number("SELECT count(*) FROM pg_stat_activity WHERE backend_type = 'client backend'")?;
    let tables = cluster.psql(
        "SELECT coalesce(json_object_agg(name, bytes), '{}') FROM (
           SELECT n.nspname || '.' || c.relname AS name, pg_total_relation_size(c.oid) AS bytes
           FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
           WHERE c.relkind IN ('r', 'p') AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
           ORDER BY bytes DESC LIMIT 12) t",
    )?;
    let final_db = cluster.number("SELECT pg_database_size(current_database())")?;
    let accounts_n = i64::try_from(options.accounts).unwrap_or(1).max(1);
    Ok(json!({
        "when": chrono_free_now(),
        "options": {
            "accounts": options.accounts, "seconds": options.seconds, "concurrency": options.concurrency,
            "pool": options.pool,
        },
        "space": {
            "binary_bytes": std::fs::metadata(&options.binary)?.len(),
            "db_empty_bytes": empty_db,
            "db_migrated_bytes": migrated_db,
            "db_filled_bytes": filled_db,
            "db_bytes_per_account": (filled_db - migrated_db) / accounts_n,
            "db_after_run_bytes": final_db,
            "storage_bytes": dir_bytes(&dir.join("storage")),
            "largest_tables": serde_json::from_str::<Value>(&tables)?,
        },
        "memory": {
            "server_idle_rss_kb": idle_rss,
            "server_after_seeding_rss_kb": after_seed_rss,
            "server_end_rss_kb": kb(server.pid(), "status", "VmRSS"),
            "server_peak_rss_kb": kb(server.pid(), "status", "VmHWM"),
            "server_threads": kb(server.pid(), "status", "Threads"),
            "postgres_idle_pss_kb": idle_pg,
            "postgres_end_pss_kb": postgres_pss_kb(cluster.pid()),
            "postgres_connections": connections,
        },
        "time": {
            "start_ms": server.started_ms,
            "seeding_seconds": seeding,
        },
        "scenarios": scenarios,
    }))
}

/// What Postgres ran for the requests: statements a request, and the costliest.
fn statements(cluster: &Cluster, requests: u64) -> Result<Value> {
    let requests = requests.max(1);
    let text = cluster.psql(&format!(
        "SELECT json_build_object(
           'per_request', (SELECT sum(calls) FROM extensions.pg_stat_statements WHERE toplevel)::float8 / {requests},
           'top', (SELECT coalesce(json_agg(t), '[]') FROM (
             SELECT round((calls::float8 / {requests})::numeric, 2) AS per_request,
                    round(((total_exec_time + total_plan_time) * 1000 / {requests})::numeric, 1) AS us_per_request,
                    round((total_plan_time * 1000 / {requests})::numeric, 1) AS plan_us_per_request,
                    toplevel, left(regexp_replace(query, '\\s+', ' ', 'g'), 160) AS query
             FROM extensions.pg_stat_statements
             ORDER BY total_exec_time + total_plan_time DESC LIMIT 8) t))"
    ))?;
    Ok(serde_json::from_str(&text)?)
}

/// Seconds since 1970, for the record; the file's name says the rest.
fn chrono_free_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// --- The report ---

fn number(value: &Value, path: &[&str]) -> Option<f64> {
    path.iter().try_fold(value, |value, key| value.get(*key))?.as_f64()
}

/// A line of the table: this run, and the baseline with the change.
fn line(label: &str, now: Option<f64>, before: Option<f64>, digits: usize, lower_better: bool) {
    let Some(now) = now else { return };
    match before {
        Some(before) if before != 0.0 => {
            let change = (now - before) / before * 100.0;
            let better = if lower_better { change < -2.0 } else { change > 2.0 };
            let worse = if lower_better { change > 2.0 } else { change < -2.0 };
            let mark = if better {
                "better"
            } else if worse {
                "worse"
            } else {
                ""
            };
            println!("  {label:<34} {now:>14.digits$} {before:>14.digits$} {change:>+8.1}% {mark}");
        }
        _ => println!("  {label:<34} {now:>14.digits$}"),
    }
}

fn print_report(result: &Value, baseline: Option<&Value>) {
    let before = |path: &[&str]| baseline.and_then(|baseline| number(baseline, path));
    let show = |label: &str, path: &[&str], digits: usize, lower: bool| {
        line(label, number(result, path), before(path), digits, lower);
    };
    println!("\n  {:<34} {:>14} {:>14} {:>9}", "", "this run", "baseline", "change");
    println!("Space");
    show("binary (bytes)", &["space", "binary_bytes"], 0, true);
    show("database after migrations (bytes)", &["space", "db_migrated_bytes"], 0, true);
    show("database per account (bytes)", &["space", "db_bytes_per_account"], 0, true);
    show("database after the run (bytes)", &["space", "db_after_run_bytes"], 0, true);
    println!("Memory");
    show("server idle RSS (kB)", &["memory", "server_idle_rss_kb"], 0, true);
    show("server peak RSS (kB)", &["memory", "server_peak_rss_kb"], 0, true);
    show("server RSS at the end (kB)", &["memory", "server_end_rss_kb"], 0, true);
    show("Postgres PSS at the end (kB)", &["memory", "postgres_end_pss_kb"], 0, true);
    show("Postgres connections", &["memory", "postgres_connections"], 0, true);
    println!("Time");
    show("start, with migrations (ms)", &["time", "start_ms"], 0, true);
    show("making the accounts (s)", &["time", "seeding_seconds"], 2, true);
    if let Some(scenarios) = result["scenarios"].as_object() {
        for name in scenarios.keys() {
            println!("{name}");
            show("requests a second", &["scenarios", name, "rps"], 0, false);
            show("p50 (ms)", &["scenarios", name, "p50_ms"], 3, true);
            show("p99 (ms)", &["scenarios", name, "p99_ms"], 3, true);
            show("server CPU ms per 1000 requests", &["scenarios", name, "server_cpu_ms_per_1k"], 0, true);
            show("Postgres CPU ms per 1000 requests", &["scenarios", name, "postgres_cpu_ms_per_1k"], 0, true);
            show("failed", &["scenarios", name, "failed"], 0, true);
        }
    }
}
