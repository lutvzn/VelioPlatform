//! Starts the server: settings from the environment (PLAN.md, "Moving off
//! Cloudflare", "Configuration"), the database, and the site.
//!
//! - `velio-server clean-up [--now MS]` runs the daily clean-up once and
//!   prints how many unconfirmed accounts it deleted (for the tests).
//! - `velio-server backup` makes the nightly backup now.
//! - `velio-server health` asks the running server's /healthz, for the
//!   container's health check: the image has no shell or curl.

use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing_subscriber::EnvFilter;
use velio_server::backup::back_up;
use velio_server::cleanup::{clean_up, spawn_daily};
use velio_server::config::Config;
use velio_server::db::Db;
use velio_server::time::now_ms;
use velio_server::{AppState, app};

const USAGE: &str = "Usage: velio-server [clean-up [--now MS] | backup | health]";

#[tokio::main]
async fn main() -> Result<()> {
    // The container runtime or journald adds the time, so lines carry only
    // the level and the message.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .without_time()
        .with_target(false)
        // No colour codes in container logs and journald.
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if args == ["health"] {
        return health().await;
    }

    let cfg = Config::from_env()?;
    let db = Db::open(&cfg.database)?;
    match args[..] {
        [] => {}
        ["clean-up"] => return run_clean_up(&db, now_ms()).await,
        ["clean-up", "--now", now] => return run_clean_up(&db, now.parse().context("--now takes milliseconds")?).await,
        ["backup"] => {
            let backup = cfg.backup.as_ref().context("Set VELIO_BACKUP_DIR to make backups")?;
            let http = reqwest::Client::new();
            let done = back_up(&db, backup, &http, now_ms()).await?;
            println!("{}", done.path.display());
            return Ok(());
        }
        _ => bail!(USAGE),
    }
    let listen = cfg.listen;
    if cfg.dev_mode {
        tracing::warn!("DEV_MODE is on: email goes to the dev outbox, and Turnstile is skipped");
    }
    if cfg.backup.is_none() {
        tracing::warn!("VELIO_BACKUP_DIR isn't set, so the database isn't backed up");
    }

    let state = AppState::new(cfg, db.clone())?;
    spawn_daily(db, state.cfg.backup.clone(), state.http.clone());
    let listener = tokio::net::TcpListener::bind(listen).await.with_context(|| format!("Listening on {listen}"))?;
    tracing::info!("Listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown())
        .await?;
    tracing::info!("Stopped");
    Ok(())
}

async fn run_clean_up(db: &Db, now: i64) -> Result<()> {
    let cleaned = db.call(move |conn| clean_up(conn, now)).await?;
    // The count alone on stdout, for the tests.
    if cleaned.purged > 0 {
        eprintln!("Deleted the data of {} closed accounts", cleaned.purged);
    }
    println!("{}", cleaned.unconfirmed);
    Ok(())
}

/// GET /healthz on VELIO_LISTEN, reached over loopback when it's 0.0.0.0 or [::].
async fn health() -> Result<()> {
    let listen = std::env::var("VELIO_LISTEN").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let mut addr: SocketAddr = listen.trim().parse().context("VELIO_LISTEN must be an address and port")?;
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => addr.set_ip(Ipv4Addr::LOCALHOST.into()),
        IpAddr::V6(ip) if ip.is_unspecified() => addr.set_ip(Ipv6Addr::LOCALHOST.into()),
        _ => {}
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?;
    let response = client.get(format!("http://{addr}/healthz")).send().await.context("The server doesn't answer")?;
    if !response.status().is_success() {
        bail!("The server answered {}", response.status());
    }
    Ok(())
}

/// Ctrl-C, or SIGTERM from systemd or Docker. Open sockets close; TVs reconnect.
async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            term.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}
