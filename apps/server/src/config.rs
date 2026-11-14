//! Settings, from the environment (PLAN.md, "Moving off Cloudflare",
//! "Configuration"). The server refuses to start with a missing or malformed
//! setting, rather than fail on the first request that needs it.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::sync_core::{DEFAULT_DEVICE_LIMIT, MAX_DEVICES};

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

/// S3-compatible storage for the nightly backups, path-style:
/// `https://<endpoint>/<bucket>[/<prefix>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3 {
    /// `https` or `http`.
    pub scheme: String,
    /// The endpoint's host, and port if any.
    pub host: String,
    /// `/<bucket>[/<prefix>]`, with no trailing slash.
    pub path: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// The nightly copy of the database (PLAN.md, "Differences, on purpose").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    /// Where the copies go, kept BACKUP_KEEP_DAYS.
    pub dir: PathBuf,
    /// Also sent here, when set.
    pub s3: Option<S3>,
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
    /// The limit for an account the operator gave `granted` (accounts.tv_limit), or none.
    pub fn for_account(self, granted: Option<i64>) -> usize {
        granted.and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0).unwrap_or(self.default).min(self.max)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The site's origin, such as `https://velio.tv`, with no trailing slash.
    pub site_url: String,
    /// Its host, such as `velio.tv`, for the words on pages.
    pub site_host: String,
    pub listen: SocketAddr,
    pub database: PathBuf,
    /// The reverse proxy's addresses. Requests from them carry the client's
    /// IP in X-Forwarded-For, and the TLS details for /v1/probe.
    pub trusted_proxies: Vec<IpRange>,
    /// Nightly backups, when VELIO_BACKUP_DIR is set.
    pub backup: Option<Backup>,
    /// Who carries traffic to the server and ends HTTPS, such as Cloudflare,
    /// for the privacy notice.
    pub network_provider: Option<String>,
    /// The HMAC key for made-up salts (GET /prelogin).
    pub prelogin_secret: String,
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
        let database = value("VELIO_DATABASE").unwrap_or_else(|| "velio.db".into()).into();
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
        let backup = backup(&value, dev_mode)?;
        let network_provider = value("VELIO_NETWORK_PROVIDER");

        let prelogin_secret = required("PRELOGIN_SECRET")?;
        if !dev_mode && prelogin_secret.len() < 32 {
            bail!("PRELOGIN_SECRET must be at least 32 characters, such as `openssl rand -base64 32`");
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
            database,
            trusted_proxies,
            backup,
            network_provider,
            prelogin_secret,
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

fn backup(value: &impl Fn(&str) -> Option<String>, dev_mode: bool) -> Result<Option<Backup>> {
    let s3 = match value("VELIO_BACKUP_S3_URL") {
        None => {
            if value("VELIO_BACKUP_S3_ACCESS_KEY_ID").is_some() || value("VELIO_BACKUP_S3_SECRET_ACCESS_KEY").is_some()
            {
                bail!("VELIO_BACKUP_S3_URL must be set with the S3 keys");
            }
            None
        }
        Some(url) => {
            let (scheme, rest) = url
                .split_once("://")
                .filter(|(scheme, _)| *scheme == "https" || (dev_mode && *scheme == "http"))
                .context("VELIO_BACKUP_S3_URL must start with https://")?;
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let path = path.trim_end_matches('/');
            let safe = |text: &str| text.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b));
            if host.is_empty()
                || !safe(&host.replace(':', ""))
                || path.is_empty()
                || !path.split('/').all(|seg| !seg.is_empty() && safe(seg))
            {
                bail!(
                    "VELIO_BACKUP_S3_URL must be the endpoint and the bucket, such as https://s3.example.com/velio-backups"
                );
            }
            // As reqwest sends Host, which the signature covers.
            let default_port = if scheme == "https" { ":443" } else { ":80" };
            let host = host.strip_suffix(default_port).unwrap_or(host);
            let key = |name: &str| value(name).with_context(|| format!("{name} must be set with VELIO_BACKUP_S3_URL"));
            Some(S3 {
                scheme: scheme.into(),
                host: host.into(),
                path: format!("/{path}"),
                region: value("VELIO_BACKUP_S3_REGION").unwrap_or_else(|| "auto".into()),
                access_key_id: key("VELIO_BACKUP_S3_ACCESS_KEY_ID")?,
                secret_access_key: key("VELIO_BACKUP_S3_SECRET_ACCESS_KEY")?,
            })
        }
    };
    match value("VELIO_BACKUP_DIR") {
        Some(dir) => Ok(Some(Backup { dir: dir.into(), s3 })),
        None if s3.is_some() => {
            bail!("VELIO_BACKUP_DIR must be set for VELIO_BACKUP_S3_URL: copies are made there first")
        }
        None => Ok(None),
    }
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
            ("PRELOGIN_SECRET", "a-prelogin-secret-of-32-characters!"),
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
            (vec![("PRELOGIN_SECRET", "short")], "at least 32"),
            (vec![("SMTP_URL", "")], "Set SMTP_URL or RESEND_API_KEY"),
            (vec![("RESEND_API_KEY", "re_x")], "not both"),
            (vec![("SMTP_URL", "mail.example")], "smtp://"),
            (vec![("TURNSTILE_SITE_KEY", "0x4")], "or neither"),
            (vec![("VELIO_TRUSTED_PROXY", "caddy")], "IP addresses or ranges"),
            (vec![("VELIO_TRUSTED_PROXY", "192.0.2.1/33")], "IP addresses or ranges"),
            (vec![("VELIO_BACKUP_S3_URL", "https://s3.example.com/b")], "VELIO_BACKUP_S3_ACCESS_KEY_ID must be set"),
            (vec![("VELIO_BACKUP_S3_ACCESS_KEY_ID", "k")], "VELIO_BACKUP_S3_URL must be set"),
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
    fn reads_proxies_and_backups() {
        let config = config(&[
            ("VELIO_TRUSTED_PROXY", "127.0.0.1, 172.16.0.0/12,fd00::/8"),
            ("VELIO_BACKUP_DIR", "/data/backups"),
            ("VELIO_BACKUP_S3_URL", "https://account.r2.example:443/velio-backups/nightly/"),
            ("VELIO_BACKUP_S3_ACCESS_KEY_ID", "key"),
            ("VELIO_BACKUP_S3_SECRET_ACCESS_KEY", "secret"),
        ])
        .unwrap();
        let trusted = |ip: &str| config.trusted_proxies.iter().any(|range| range.contains(ip.parse().unwrap()));
        assert!(
            trusted("127.0.0.1") && trusted("172.18.0.5") && trusted("::ffff:172.31.255.255") && trusted("fd12::1")
        );
        assert!(!trusted("127.0.0.2") && !trusted("172.32.0.1") && !trusted("::1"));
        let backup = config.backup.unwrap();
        assert_eq!(backup.dir, PathBuf::from("/data/backups"));
        let s3 = backup.s3.unwrap();
        assert_eq!(
            (s3.host.as_str(), s3.path.as_str(), s3.region.as_str()),
            ("account.r2.example", "/velio-backups/nightly", "auto")
        );
        assert!(
            self::config(&[
                ("VELIO_BACKUP_S3_URL", "https://s3.example.com/b"),
                ("VELIO_BACKUP_S3_ACCESS_KEY_ID", "k"),
                ("VELIO_BACKUP_S3_SECRET_ACCESS_KEY", "s")
            ])
            .unwrap_err()
            .to_string()
            .contains("VELIO_BACKUP_DIR")
        );
        let everyone = self::config(&[("VELIO_TRUSTED_PROXY", "0.0.0.0/0")]).unwrap();
        assert!(everyone.trusted_proxies[0].contains("203.0.113.9".parse().unwrap()));
    }

    #[test]
    fn dev_mode_keeps_mail_in_the_outbox() {
        let config = config(&[("DEV_MODE", "1"), ("SMTP_URL", ""), ("PRELOGIN_SECRET", "dev")]).unwrap();
        assert_eq!(config.mail, Mail::DevOutbox);
        let local = self::config(&[("VELIO_SITE_URL", "http://127.0.0.1:8787")]).unwrap();
        assert_eq!(local.site_host, "127.0.0.1");
    }
}
