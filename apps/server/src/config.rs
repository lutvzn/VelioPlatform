//! Settings, from the environment (PLAN.md, "Moving off Cloudflare",
//! "Configuration"). The server refuses to start with a missing or malformed
//! setting, rather than fail on the first request that needs it.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::jwt;

/// Every account's TV limit, unless the operator gives it another.
pub const DEFAULT_DEVICE_LIMIT: usize = 10;
/// The most TVs the operator may give an account.
pub const MAX_DEVICES: usize = 50;
/// Connections to Postgres, unless VELIO_DB_POOL says otherwise.
pub const DB_POOL: usize = 16;

/// How email leaves the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mail {
    /// DEV_MODE: written to `dev_outbox`, shown at /dev/outbox.
    DevOutbox,
    /// Such as `smtps://user:pass@mail.example:465`.
    Smtp(String),
    /// Resend's HTTP API, with this key.
    Resend(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turnstile {
    pub site_key: String,
    pub secret: String,
}

/// An address, or a range of them such as 172.16.0.0/12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRange {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl IpRange {
    fn parse(text: &str) -> Option<Self> {
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr.parse::<IpAddr>().ok()?, Some(prefix.parse::<u8>().ok()?)),
            None => (text.parse().ok()?, None),
        };
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then_some(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        // A v4 client of a server listening on [::] shows as ::ffff:a.b.c.d.
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(range), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
                u32::from(range) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(range), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.prefix)).unwrap_or(0);
                u128::from(range) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// How many TVs an account may have (PLAN.md, "TV limits").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TvLimits {
    /// VELIO_TV_LIMIT: every account's, unless the operator gave it another.
    pub default: usize,
    /// VELIO_TV_LIMIT_MAX: the most the operator may give, at most MAX_DEVICES.
    pub max: usize,
}

impl Default for TvLimits {
    fn default() -> Self {
        Self { default: DEFAULT_DEVICE_LIMIT, max: MAX_DEVICES }
    }
}

impl TvLimits {
    /// The limit for an account the operator gave `granted` (velio_private.tv_limits), or none.
    pub fn for_account(self, granted: Option<i64>) -> usize {
        granted.and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0).unwrap_or(self.default).min(self.max)
    }
}

/// Supabase Auth's settings, under the names Nuvio's self-host gives them.
#[derive(Debug, Clone)]
pub struct Auth {
    /// JWT_SECRET: signs access tokens, and the anon and service keys.
    pub jwt_secret: String,
    /// JWT_EXPIRY: how long an access token lasts, in seconds.
    pub jwt_expiry: i64,
    /// ANON_KEY and SERVICE_ROLE_KEY: legacy API keys, tokens signed with
    /// JWT_SECRET. Made from it when not set.
    pub anon_key: String,
    pub service_role_key: String,
    /// SUPABASE_PUBLISHABLE_KEY and SUPABASE_SECRET_KEY: the newer sb_ keys,
    /// which stand for the anon and service keys.
    pub publishable_key: Option<String>,
    pub secret_key: Option<String>,
    pub disable_signup: bool,
    /// ENABLE_ANONYMOUS_USERS: Nuvio's apps start as a guest.
    pub anonymous_users: bool,
    /// ENABLE_EMAIL_AUTOCONFIRM: accounts can sign in before confirming the email.
    pub autoconfirm: bool,
    pub password_min_length: usize,
}

impl Auth {
    /// The key Nuvio's clients send as `apikey` (/.well-known/nuvio).
    pub fn public_key(&self) -> &str {
        self.publishable_key.as_deref().unwrap_or(&self.anon_key)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The site's origin, such as `https://velio.tv`, with no trailing slash.
    /// Also the backend Nuvio's clients are given.
    pub site_url: String,
    /// Its host, such as `velio.tv`, for the words on pages.
    pub site_host: String,
    pub listen: SocketAddr,
    pub database_url: String,
    /// The most connections the server keeps to Postgres (VELIO_DB_POOL).
    pub db_pool: usize,
    /// Where Storage keeps its files (avatars).
    pub storage_dir: PathBuf,
    /// The reverse proxy's addresses. Requests from them carry the client's
    /// IP in X-Forwarded-For.
    pub trusted_proxies: Vec<IpRange>,
    /// Who carries traffic to the server and ends HTTPS, such as Cloudflare,
    /// for the privacy notice.
    pub network_provider: Option<String>,
    pub auth: Auth,
    /// The catalog add-on new profiles get (Nuvio's NUVIO_DEFAULT_CATALOG_URL).
    pub default_catalog_url: String,
    /// The operator's token; /ops is a 404 without it.
    pub ops_token: Option<String>,
    pub email_from: String,
    pub abuse_email: String,
    pub mail: Mail,
    /// None turns Turnstile off: the widget, the check and its CSP entry.
    pub turnstile: Option<Turnstile>,
    pub tv_limits: TvLimits,
    pub dev_mode: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads the settings through `get`, so tests can pass their own.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let value = |name: &str| get(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let required = |name: &str| value(name).with_context(|| format!("{name} must be set"));

        let dev_mode = match value("DEV_MODE").as_deref() {
            None | Some("0") => false,
            Some("1") => true,
            Some(other) => bail!("DEV_MODE must be 1 or 0, not {other:?}"),
        };

        let (site_url, site_host) = site_origin(&required("VELIO_SITE_URL")?)?;
        let listen = value("VELIO_LISTEN")
            .unwrap_or_else(|| "127.0.0.1:8787".into())
            .parse()
            .context("VELIO_LISTEN must be an address and port, such as 127.0.0.1:8787")?;
        let database_url = required("DATABASE_URL")?;
        let db_pool = match value("VELIO_DB_POOL") {
            None => DB_POOL,
            Some(text) => text
                .parse()
                .ok()
                .filter(|n| (1..=200).contains(n))
                .context("VELIO_DB_POOL must be a number of connections from 1 to 200")?,
        };
        let storage_dir = value("VELIO_STORAGE_DIR").unwrap_or_else(|| "storage".into()).into();
        let trusted_proxies = value("VELIO_TRUSTED_PROXY")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| {
                IpRange::parse(part).with_context(|| {
                    format!("VELIO_TRUSTED_PROXY must be IP addresses or ranges, such as 172.16.0.0/12, not {part:?}")
                })
            })
            .collect::<Result<_>>()?;
        let network_provider = value("VELIO_NETWORK_PROVIDER");

        let auth = auth(&value, dev_mode)?;
        let default_catalog_url =
            value("DEFAULT_CATALOG_URL").unwrap_or_else(|| "https://catalog.nuvio.tv/manifest.json".into());
        if !default_catalog_url.starts_with("https://") && !default_catalog_url.starts_with("http://") {
            bail!("DEFAULT_CATALOG_URL must be an add-on's manifest address");
        }
        let ops_token = value("OPS_TOKEN");
        if !dev_mode && ops_token.as_ref().is_some_and(|token| token.len() < 32) {
            bail!("OPS_TOKEN must be at least 32 characters, such as `openssl rand -base64 32`");
        }

        let email_from = required("EMAIL_FROM")?;
        let abuse_email = required("ABUSE_EMAIL")?;
        let mail = match (value("SMTP_URL"), value("RESEND_API_KEY")) {
            _ if dev_mode => Mail::DevOutbox,
            (Some(_), Some(_)) => bail!("Set SMTP_URL or RESEND_API_KEY, not both"),
            (Some(url), None) => {
                if !(url.starts_with("smtp://") || url.starts_with("smtps://")) {
                    bail!("SMTP_URL must start with smtp:// or smtps://");
                }
                Mail::Smtp(url)
            }
            (None, Some(key)) => Mail::Resend(key),
            (None, None) => bail!("Set SMTP_URL or RESEND_API_KEY, so the server can send email"),
        };

        let turnstile = match (value("TURNSTILE_SITE_KEY"), value("TURNSTILE_SECRET")) {
            (Some(site_key), Some(secret)) => Some(Turnstile { site_key, secret }),
            (None, None) => None,
            _ => bail!("Set both TURNSTILE_SITE_KEY and TURNSTILE_SECRET, or neither"),
        };

        let tv_limits = tv_limits(&value)?;

        Ok(Self {
            site_url,
            site_host,
            listen,
            database_url,
            db_pool,
            storage_dir,
            trusted_proxies,
            network_provider,
            auth,
            default_catalog_url,
            ops_token,
            email_from,
            abuse_email,
            mail,
            turnstile,
            tv_limits,
            dev_mode,
        })
    }
}

fn tv_limits(value: &impl Fn(&str) -> Option<String>) -> Result<TvLimits> {
    let number = |name: &str, default: usize| -> Result<usize> {
        value(name).map_or(Ok(default), |text| {
            text.parse::<usize>()
                .ok()
                .filter(|n| (1..=MAX_DEVICES).contains(n))
                .with_context(|| format!("{name} must be a number of TVs from 1 to {MAX_DEVICES}, not {text:?}"))
        })
    };
    let default = number("VELIO_TV_LIMIT", DEFAULT_DEVICE_LIMIT)?;
    let max = number("VELIO_TV_LIMIT_MAX", MAX_DEVICES.max(default))?;
    if max < default {
        bail!("VELIO_TV_LIMIT_MAX ({max}) must be at least VELIO_TV_LIMIT ({default})");
    }
    Ok(TvLimits { default, max })
}

fn flag(value: &impl Fn(&str) -> Option<String>, name: &str, default: bool) -> Result<bool> {
    match value(name).as_deref() {
        None => Ok(default),
        Some("true" | "1") => Ok(true),
        Some("false" | "0") => Ok(false),
        Some(other) => bail!("{name} must be true or false, not {other:?}"),
    }
}

fn auth(value: &impl Fn(&str) -> Option<String>, dev_mode: bool) -> Result<Auth> {
    let jwt_secret = value("JWT_SECRET").context("JWT_SECRET must be set, such as `openssl rand -base64 48`")?;
    if !dev_mode && jwt_secret.len() < 32 {
        bail!("JWT_SECRET must be at least 32 characters, such as `openssl rand -base64 48`");
    }
    let jwt_expiry = match value("JWT_EXPIRY") {
        None => 3600,
        Some(text) => text
            .parse::<i64>()
            .ok()
            .filter(|seconds| (60..=7 * 24 * 3600).contains(seconds))
            .with_context(|| format!("JWT_EXPIRY must be seconds, from 60 to a week, not {text:?}"))?,
    };
    let key = |name: &str, role: &str| -> Result<String> {
        let Some(key) = value(name) else { return Ok(jwt::api_key(role, &jwt_secret)) };
        let claims = jwt::verify(&key, &jwt_secret, 0)
            .map_err(|_| anyhow::anyhow!("{name} must be a token signed with JWT_SECRET"))?;
        if claims.get("role").and_then(serde_json::Value::as_str) != Some(role) {
            bail!("{name} must be a token for the {role} role");
        }
        Ok(key)
    };
    let sb_key = |name: &str, prefix: &str| -> Result<Option<String>> {
        match value(name) {
            Some(key) if !key.starts_with(prefix) => bail!("{name} must start with {prefix}"),
            other => Ok(other),
        }
    };
    let password_min_length = match value("PASSWORD_MIN_LENGTH") {
        None => 6,
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|n| (6..=72).contains(n))
            .with_context(|| format!("PASSWORD_MIN_LENGTH must be from 6 to 72, not {text:?}"))?,
    };
    Ok(Auth {
        anon_key: key("ANON_KEY", "anon")?,
        service_role_key: key("SERVICE_ROLE_KEY", "service_role")?,
        publishable_key: sb_key("SUPABASE_PUBLISHABLE_KEY", "sb_publishable_")?,
        secret_key: sb_key("SUPABASE_SECRET_KEY", "sb_secret_")?,
        disable_signup: flag(value, "DISABLE_SIGNUP", false)?,
        anonymous_users: flag(value, "ENABLE_ANONYMOUS_USERS", true)?,
        autoconfirm: flag(value, "ENABLE_EMAIL_AUTOCONFIRM", true)?,
        password_min_length,
        jwt_secret,
        jwt_expiry,
    })
}

/// `https://velio.tv` or `http://127.0.0.1:8787`: a scheme and a host, no path.
fn site_origin(url: &str) -> Result<(String, String)> {
    let url = url.trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .context("VELIO_SITE_URL must start with https:// (or http:// for a local server)")?;
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        bail!("VELIO_SITE_URL must be only a scheme and a host, such as https://velio.tv");
    }
    let host = rest.rsplit_once(':').map_or(rest, |(host, _)| host);
    Ok((url.to_owned(), host.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(pairs: &[(&str, &str)]) -> Result<Config> {
        let base: HashMap<&str, &str> = [
            ("VELIO_SITE_URL", "https://velio.tv/"),
            ("JWT_SECRET", "a-jwt-secret-of-at-least-32-characters!"),
            ("DATABASE_URL", "postgres://velio@127.0.0.1/velio"),
            ("EMAIL_FROM", "no-reply@velio.tv"),
            ("ABUSE_EMAIL", "abuse@velio.tv"),
            ("SMTP_URL", "smtps://user:pass@mail.example:465"),
        ]
        .into_iter()
        .chain(pairs.iter().copied())
        .collect();
        Config::from_lookup(|name| base.get(name).map(|v| (*v).to_owned()))
    }

    #[test]
    fn reads_a_production_setup() {
        let config = config(&[]).unwrap();
        assert_eq!(config.site_url, "https://velio.tv");
        assert_eq!(config.site_host, "velio.tv");
        assert_eq!(config.listen, "127.0.0.1:8787".parse().unwrap());
        assert_eq!(config.mail, Mail::Smtp("smtps://user:pass@mail.example:465".into()));
        assert_eq!(config.turnstile, None);
        assert_eq!(config.tv_limits, TvLimits { default: 10, max: 50 });
        assert!(!config.dev_mode);
        assert_eq!(config.default_catalog_url, "https://catalog.nuvio.tv/manifest.json");
        let auth = &config.auth;
        assert_eq!((auth.jwt_expiry, auth.password_min_length), (3600, 6));
        assert!(auth.anonymous_users && auth.autoconfirm && !auth.disable_signup);
        assert_eq!(jwt::verify(&auth.anon_key, &auth.jwt_secret, 0).unwrap()["role"], "anon");
        assert_eq!(jwt::verify(&auth.service_role_key, &auth.jwt_secret, 0).unwrap()["role"], "service_role");
        assert_eq!(auth.public_key(), auth.anon_key);
    }

    #[test]
    fn reads_tv_limits() {
        let limits = config(&[("VELIO_TV_LIMIT", "5"), ("VELIO_TV_LIMIT_MAX", "20")]).unwrap().tv_limits;
        assert_eq!(limits, TvLimits { default: 5, max: 20 });
        assert_eq!(limits.for_account(None), 5);
        assert_eq!(limits.for_account(Some(12)), 12);
        assert_eq!(limits.for_account(Some(2)), 2);
        // A limit given before VELIO_TV_LIMIT_MAX went down is held to it.
        assert_eq!(limits.for_account(Some(40)), 20);
        assert_eq!(limits.for_account(Some(0)), 5);
    }

    #[test]
    fn refuses_what_would_fail_later() {
        for (pairs, problem) in [
            (vec![("VELIO_SITE_URL", "velio.tv")], "must start with https://"),
            (vec![("VELIO_SITE_URL", "https://velio.tv/app")], "only a scheme and a host"),
            (vec![("JWT_SECRET", "short")], "at least 32"),
            (vec![("ANON_KEY", "not-a-token")], "ANON_KEY must be a token signed with JWT_SECRET"),
            (vec![("SUPABASE_SECRET_KEY", "secret")], "must start with sb_secret_"),
            (vec![("DISABLE_SIGNUP", "maybe")], "DISABLE_SIGNUP must be true or false"),
            (vec![("JWT_EXPIRY", "5")], "JWT_EXPIRY must be seconds"),
            (vec![("PASSWORD_MIN_LENGTH", "3")], "PASSWORD_MIN_LENGTH must be from 6"),
            (vec![("SMTP_URL", "")], "Set SMTP_URL or RESEND_API_KEY"),
            (vec![("RESEND_API_KEY", "re_x")], "not both"),
            (vec![("SMTP_URL", "mail.example")], "smtp://"),
            (vec![("TURNSTILE_SITE_KEY", "0x4")], "or neither"),
            (vec![("VELIO_TRUSTED_PROXY", "caddy")], "IP addresses or ranges"),
            (vec![("VELIO_TRUSTED_PROXY", "192.0.2.1/33")], "IP addresses or ranges"),
            (vec![("DEV_MODE", "yes")], "DEV_MODE"),
            (vec![("VELIO_TV_LIMIT", "0")], "VELIO_TV_LIMIT must be a number of TVs from 1 to 50"),
            (vec![("VELIO_TV_LIMIT_MAX", "51")], "VELIO_TV_LIMIT_MAX must be a number of TVs from 1 to 50"),
            (vec![("VELIO_TV_LIMIT", "ten")], "VELIO_TV_LIMIT must be"),
            (vec![("VELIO_TV_LIMIT", "20"), ("VELIO_TV_LIMIT_MAX", "15")], "must be at least VELIO_TV_LIMIT"),
        ] {
            let error = config(&pairs).unwrap_err().to_string();
            assert!(error.contains(problem), "{pairs:?}: {error}");
        }
    }

    #[test]
    fn reads_proxies() {
        let config = config(&[("VELIO_TRUSTED_PROXY", "127.0.0.1, 172.16.0.0/12,fd00::/8")]).unwrap();
        let trusted = |ip: &str| config.trusted_proxies.iter().any(|range| range.contains(ip.parse().unwrap()));
        assert!(
            trusted("127.0.0.1") && trusted("172.18.0.5") && trusted("::ffff:172.31.255.255") && trusted("fd12::1")
        );
        assert!(!trusted("127.0.0.2") && !trusted("172.32.0.1") && !trusted("::1"));
        let everyone = self::config(&[("VELIO_TRUSTED_PROXY", "0.0.0.0/0")]).unwrap();
        assert!(everyone.trusted_proxies[0].contains("203.0.113.9".parse().unwrap()));
    }

    #[test]
    fn dev_mode_keeps_mail_in_the_outbox() {
        let config = config(&[("DEV_MODE", "1"), ("SMTP_URL", ""), ("JWT_SECRET", "dev")]).unwrap();
        assert_eq!(config.mail, Mail::DevOutbox);
        let local = self::config(&[("VELIO_SITE_URL", "http://127.0.0.1:8787")]).unwrap();
        assert_eq!(local.site_host, "127.0.0.1");
    }
}
