//! What's signed in to the account: TVs, apps and browsers, as Nuvio's
//! list_my_sessions lists them.

use maud::{Markup, html};

use super::accounts::problem;
use super::layout::{ICON_PHONE, ICON_SERVER, ICON_TV, icon};
use crate::time::day;

pub struct DeviceRow {
    pub session_id: String,
    pub name: String,
    /// "Nuvio TV 1.4.0 · android", or what's known of it.
    pub detail: Option<String>,
    pub kind: DeviceKind,
    pub created_at: i64,
    pub last_active_at: i64,
    pub current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Tv,
    App,
    Browser,
}

impl DeviceKind {
    /// From Nuvio's client names, and VelioTV's.
    pub fn of(client_name: Option<&str>) -> Self {
        match client_name {
            Some("Nuvio TV" | "Velio TV") => Self::Tv,
            Some("Nuvio Web") => Self::Browser,
            _ => Self::App,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Tv => ICON_TV,
            Self::App => ICON_PHONE,
            Self::Browser => ICON_SERVER,
        }
    }
}

pub struct DevicesView<'a> {
    pub devices: &'a [DeviceRow],
    pub tvs: usize,
    pub tv_limit: usize,
    pub notice: Option<&'a str>,
}

pub fn devices_page(view: &DevicesView<'_>) -> Markup {
    html! {
        section {
            h1 { "TVs and apps" }
            @if let Some(notice) = view.notice { p .done role="status" { (notice) } }
            p .note {
                "Everything signed in to your account. Signing one out ends its session at once; it has to sign in again."
            }
            p .note { (view.tvs) " of " (view.tv_limit) " TVs signed in." }
            (problem())
            ul .rows {
                @for device in view.devices {
                    li {
                        div .row-head {
                            (icon(device.kind.icon()))
                            strong { (device.name) }
                            @if device.current { span .note { "This browser" } }
                        }
                        @if let Some(detail) = &device.detail { p .note { (detail) } }
                        p .note {
                            "Signed in " (day(device.created_at)) ", last used " (day(device.last_active_at))
                        }
                        @if !device.current {
                            form method="post" action={ "/devices/" (device.session_id) "/signout" }
                                hx-post={ "/devices/" (device.session_id) "/signout" } hx-target="#main" {
                                button type="submit" .danger { "Sign it out" }
                            }
                        }
                    }
                }
            }
            p { a href="/link" .button { "Sign in a TV or app with a code" } }
        }
    }
}
