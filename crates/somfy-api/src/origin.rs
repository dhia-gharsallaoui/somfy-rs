//! Whether a request was addressed to this device, and made by a page this
//! device served.
//!
//! # This is not authentication, and it defends against something a password
//! would not
//!
//! The device has no login and, by the owner's decision, is not getting one
//! yet. That decision is not what this module is about. **Reachability is not
//! membership**: any page in any browser tab can issue requests to a LAN
//! address, and the browser will *deliver* a cross-origin `POST` even though it
//! refuses to let the attacking page read the answer. So "only people on my
//! Wi-Fi can reach it" is false in the one direction that matters — and a
//! password would not close it either, since the classic form of this attack is
//! aimed at routers, which do have passwords.
//!
//! What closes it is refusing to act on a request that a page somewhere else
//! made. Two headers say so, and they cover two different attacks:
//!
//! - **`Origin` names the page that made the request.** A cross-origin `fetch`
//!   or `XMLHttpRequest`, a form submission, `sendBeacon`, and **every**
//!   WebSocket handshake carry it, and a page can neither forge nor suppress it.
//! - **`Host` names the address the client dialled.** It is the answer to *DNS
//!   rebinding*, where the attacker's page keeps its own origin and moves the
//!   name underneath it: `evil.example` re-resolves to this device's address,
//!   the browser now considers the request same-origin, and `Origin` agrees
//!   with `Host` because both say `evil.example`.
//!
//! # The two rules
//!
//! 1. **`Host` must be an IPv4 literal, or this device's own name** — bare or
//!    with `.local` — on the port this server is bound to.
//! 2. **`Origin`, if present, must be `http://` and the same authority the
//!    request was addressed to.**
//!
//! Together those are exactly as strong as checking both against a list of this
//! device's addresses, and they need no knowledge of what this device's address
//! currently *is* — which matters, because it is a DHCP lease that moves under
//! a running device. Walking the attacks:
//!
//! - **Cross-origin `POST` from `https://evil.example` to this device's
//!   address.** `Host` is the address, which passes rule 1; `Origin` is
//!   `https://evil.example`, which is neither `http` nor the authority dialled,
//!   so rule 2 refuses it.
//! - **DNS rebinding.** `Host` is `evil.example` — not a literal, not this
//!   device's name — so rule 1 refuses it before rule 2 is even consulted.
//!   Comparing `Origin` to `Host` alone would have *passed* this, which is why
//!   rule 1 exists.
//! - **A page served by some other host on the LAN over plain `http`.** `Host`
//!   is this device's address, `Origin` is the other host's, and they differ, so
//!   rule 2 refuses it.
//!
//! **Why rule 1 may accept an address that is not ours.** Because it cannot be
//! exploited: DNS rebinding needs a *name*, since a name is the only thing an
//! attacker can move. An IP literal bypasses DNS entirely, so a browser that
//! sends `Host: 10.0.0.9` has connected to 10.0.0.9 and is not talking to us at
//! all. The one way to reach this device while claiming a different address is
//! to be on the network path — and an attacker there can read and rewrite
//! everything anyway, which no header check addresses.
//!
//! # Absence means "not a browser", and that is the whole rule
//!
//! Every decision below about a **missing** header follows from one fact: the
//! attack requires a browser, and a browser cannot be made to omit these
//! headers where it sends them. `Host` is mandatory in HTTP/1.1. `Origin` is
//! sent by every browser on every `POST`, `PUT`, `PATCH` and `DELETE`, and on
//! every WebSocket handshake, whether or not the request is same-origin. So a
//! request that omits one was not made by a browser, and refusing it would
//! break `curl`, a shell script and this project's own test rigs while stopping
//! nothing.
//!
//! That leaves the one case where absence is *not* evidence: a **cross-origin
//! `GET` issued by a tag rather than by script** — `<img src>`, `<script src>`,
//! `<link>`, a top-level navigation — sends no `Origin` at all. A browser
//! genuinely can produce an `Origin`-less `GET` at this device, and this module
//! admits it. That is safe here for two reasons which are properties of the API
//! rather than of this file, and they are the two things to re-check if either
//! changes:
//!
//! 1. **No `GET` on this device changes anything.** Every one is a read; the
//!    state-changing surface is `POST`, `PUT`, `PATCH` and `DELETE`, and a
//!    browser carries `Origin` on all four.
//! 2. **No response carries `Access-Control-Allow-Origin`.** Without it the
//!    browser hands the attacking page nothing, so an `Origin`-less `GET` it
//!    can make is a `GET` it cannot read.
//!
//! The alternative — refusing `Origin`-less requests on safe methods — would
//! make the device unreachable from a browser's address bar, since a typed URL
//! is exactly such a request.
//!
//! Note that rule 1 still applies to those `GET`s, and that is what stops a
//! rebound page *reading* the shade list: rebinding defeats the same-origin
//! policy in both directions, so the response would otherwise be readable.

use core::net::Ipv4Addr;

use crate::ApiErrorCode;

/// An authority with no port means the scheme's default, which for `http` is
/// 80. Compared rather than assumed, so this module cannot disagree with a
/// listener that moves: if the server is ever bound elsewhere, a bare authority
/// stops naming it — which is exactly what a browser would decide.
const DEFAULT_HTTP_PORT: u16 = 80;

/// What this device answers to, for [`admit`].
///
/// Deliberately **not** an address. See the module docs: the rules are written
/// so that this device's current IPv4 address does not have to be known, which
/// is what keeps the check working through a DHCP lease change that no part of
/// the web server would otherwise hear about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceNames<'a> {
    /// The mDNS label, **without** the `.local` — `somfy-<mac>`.
    ///
    /// Accepted bare and with `.local` appended, so the caller passes what it
    /// advertises rather than a second spelling of it.
    pub hostname: &'a str,
    /// The port the server is bound to.
    ///
    /// Passed rather than assumed to be 80 so that this module and the listener
    /// cannot disagree: an authority carrying an explicit port must carry
    /// *this* one.
    pub port: u16,
}

/// Decide whether a request may be served.
///
/// `host` and `origin` are the raw header values, or `None` where the request
/// did not carry them. `Ok(())` admits the request; the `Err` is the code the
/// caller refuses with, and both refusals are `403` — see
/// [`ApiErrorCode::http_status`].
///
/// `Host` is checked first because it is the stronger statement: a request
/// addressed to a name that is not this device is wrong regardless of who made
/// it, and reporting that in preference to the origin gives an operator the
/// more useful of the two messages.
pub fn admit(
    host: Option<&str>,
    origin: Option<&str>,
    device: &DeviceNames<'_>,
) -> Result<(), ApiErrorCode> {
    if let Some(host) = host {
        if authority(host, device).is_none() {
            return Err(ApiErrorCode::HostNotThisDevice);
        }
    }
    let Some(origin) = origin else {
        return Ok(());
    };
    // `null`, `https://…`, `file://…` and anything else that is not a page this
    // device could have served all fail here, which is correct: this device
    // speaks `http` on one port, so a page it served has exactly one possible
    // scheme.
    let origin = http_authority(origin).ok_or(ApiErrorCode::OriginNotThisDevice)?;
    let origin = authority(origin, device).ok_or(ApiErrorCode::OriginNotThisDevice)?;
    match host {
        // Rule 2: the page that made the request must be the address it was
        // sent to. Both have already passed rule 1, so what this adds is that
        // they are the *same* one.
        Some(host) => {
            let host = authority(host, device).ok_or(ApiErrorCode::HostNotThisDevice)?;
            if origin == host {
                Ok(())
            } else {
                Err(ApiErrorCode::OriginNotThisDevice)
            }
        }
        // No `Host` to compare against, which HTTP/1.1 forbids and no browser
        // does. Rule 1 has already been applied to the origin, which is the
        // most that can be said without one.
        None => Ok(()),
    }
}

/// The authority of an `http://` origin, or `None` for anything else.
///
/// An origin is a scheme, a host and an optional port and **nothing else** (RFC
/// 6454 §6.1), so a `/` anywhere after the scheme means this is not one and it
/// is refused rather than parsed leniently.
fn http_authority(origin: &str) -> Option<&str> {
    const SCHEME: &str = "http://";
    if !origin.get(..SCHEME.len())?.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let authority = &origin[SCHEME.len()..];
    if authority.contains('/') {
        return None;
    }
    Some(authority)
}

/// One authority — a `Host` value, or the authority of an `Origin` — reduced to
/// the form two of them can be compared in, or `None` if it does not name this
/// device at all.
///
/// An address is returned as a number rather than as text, which is what makes
/// alternative spellings a non-issue: [`Ipv4Addr`]'s parser is strict about
/// leading zeros and octet count, so a form it accepts has exactly one meaning
/// and a form it rejects falls through to the name comparison and fails there.
fn authority(raw: &str, device: &DeviceNames<'_>) -> Option<Named> {
    // An IPv6 literal is bracketed, and this device serves no IPv6 address, so
    // the whole shape is refused before the port split can mistake a colon
    // inside one for a port separator.
    if raw.starts_with('[') {
        return None;
    }
    let (host, port) = match raw.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (raw, DEFAULT_HTTP_PORT),
    };
    if port != device.port {
        return None;
    }
    // A second colon is an unbracketed IPv6 literal, or a malformed authority.
    // Either way it is neither of the two shapes below.
    if host.contains(':') {
        return None;
    }
    // A fully-qualified name may carry the root dot, and a browser passes on
    // whatever the operator typed. One, and only one, is absorbed.
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return Some(Named::Address(address));
    }
    if host.eq_ignore_ascii_case(device.hostname) || is_dot_local(host, device.hostname) {
        return Some(Named::ThisDevice);
    }
    None
}

/// An authority that passed rule 1, in a form two of them compare equal in.
///
/// The bare label and the `.local` form collapse to one value deliberately:
/// they are two spellings of one name, and a page served from one fetching from
/// the other is the same origin by any reading that matters here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// This device, by its mDNS name.
    ThisDevice,
    /// Some IPv4 address. *Which* one is not checked — see the module docs.
    Address(Ipv4Addr),
}

/// Whether `host` is `label` followed by `.local`, without building the
/// concatenation.
fn is_dot_local(host: &str, label: &str) -> bool {
    const LOCAL: &str = ".local";
    host.len() == label.len() + LOCAL.len()
        && host.is_char_boundary(label.len())
        && host[..label.len()].eq_ignore_ascii_case(label)
        && host[label.len()..].eq_ignore_ascii_case(LOCAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTNAME: &str = "somfy-a1b2c3d4e5f6";

    fn device() -> DeviceNames<'static> {
        DeviceNames {
            hostname: HOSTNAME,
            port: 80,
        }
    }

    #[test]
    fn a_request_with_neither_header_is_admitted() {
        assert_eq!(admit(None, None, &device()), Ok(()));
    }

    #[test]
    fn the_device_answers_to_an_address_to_its_label_and_to_its_dot_local() {
        for host in [
            "10.0.0.7",
            "10.0.0.7:80",
            "192.168.1.42",
            HOSTNAME,
            "somfy-a1b2c3d4e5f6.local",
            "SOMFY-A1B2C3D4E5F6.LOCAL",
            "somfy-a1b2c3d4e5f6.local.",
            "somfy-a1b2c3d4e5f6.local:80",
        ] {
            assert_eq!(admit(Some(host), None, &device()), Ok(()), "host {host}");
        }
    }

    #[test]
    fn a_host_naming_anything_else_is_refused() {
        for host in [
            // The DNS-rebinding case: a name the attacker controls, resolved to
            // this device's address. This is the one that matters.
            "evil.example",
            "localhost",
            // The label as a prefix of a domain somebody else owns, which is
            // the cheapest way to defeat a suffix-insensitive comparison.
            "somfy-a1b2c3d4e5f6.evil.example",
            "somfy-a1b2c3d4e5f6.local.evil.example",
            // Not this device's label.
            "somfy-000000000000",
            // The right shape at a port nothing is bound to.
            "10.0.0.7:8080",
            "somfy-a1b2c3d4e5f6.local:8080",
            "",
            ":80",
        ] {
            assert_eq!(
                admit(Some(host), None, &device()),
                Err(ApiErrorCode::HostNotThisDevice),
                "host {host}",
            );
        }
    }

    #[test]
    fn an_ipv6_literal_is_refused_rather_than_split_on_its_colons() {
        for host in ["[::1]", "[::1]:80", "::1", "fe80::1:80"] {
            assert_eq!(
                admit(Some(host), None, &device()),
                Err(ApiErrorCode::HostNotThisDevice),
                "host {host}",
            );
        }
    }

    #[test]
    fn an_address_is_parsed_strictly_so_alternative_spellings_do_not_slip_past() {
        // Each of these is `10.0.0.7` to some parser somewhere, and none is a
        // form a browser produces. All fall through to the name comparison and
        // fail there.
        for host in ["010.0.0.7", "10.0.0.007", "167772167", "0xa.0.0.7", "10.7"] {
            assert_eq!(
                admit(Some(host), None, &device()),
                Err(ApiErrorCode::HostNotThisDevice),
                "host {host}",
            );
        }
    }

    #[test]
    fn a_page_this_device_served_is_admitted() {
        for (host, origin) in [
            ("10.0.0.7", "http://10.0.0.7"),
            ("10.0.0.7", "http://10.0.0.7:80"),
            ("10.0.0.7:80", "http://10.0.0.7"),
            (
                "somfy-a1b2c3d4e5f6.local",
                "http://somfy-a1b2c3d4e5f6.local",
            ),
            (
                "somfy-a1b2c3d4e5f6.local",
                "HTTP://SOMFY-A1B2C3D4E5F6.LOCAL",
            ),
            // The bare label and the `.local` form are two spellings of one
            // name, so a page served from one may fetch from the other.
            ("somfy-a1b2c3d4e5f6.local", "http://somfy-a1b2c3d4e5f6"),
        ] {
            assert_eq!(
                admit(Some(host), Some(origin), &device()),
                Ok(()),
                "host {host} origin {origin}",
            );
        }
    }

    /// **The classic router attack.** The request is addressed to this device
    /// perfectly correctly; what is wrong is the page that made it.
    #[test]
    fn a_page_anywhere_else_is_refused_even_when_it_addresses_this_device() {
        for origin in [
            "https://evil.example",
            "http://evil.example",
            // A page served over TLS from this device's own address is still
            // not a page this device served: it speaks no TLS.
            "https://10.0.0.7",
            // Another host on the same LAN, over plain http.
            "http://10.0.0.9",
            // Opaque origins — a sandboxed iframe, a `data:` document.
            "null",
            "",
            // An origin is a scheme, a host and a port; a path means it is not
            // one.
            "http://10.0.0.7/",
            "http://10.0.0.7/api/v1",
            // The scheme-relative form, which is not an origin.
            "//10.0.0.7",
        ] {
            assert_eq!(
                admit(Some("10.0.0.7"), Some(origin), &device()),
                Err(ApiErrorCode::OriginNotThisDevice),
                "origin {origin}",
            );
        }
    }

    /// **The rebinding case, and the one a same-origin comparison would miss.**
    /// The page's own origin and the address it dialled are the same name, so
    /// any check of one against the other passes.
    #[test]
    fn rebinding_is_caught_by_the_host_even_though_the_origin_agrees_with_it() {
        assert_eq!(
            admit(Some("evil.example"), Some("http://evil.example"), &device()),
            Err(ApiErrorCode::HostNotThisDevice),
        );
    }

    #[test]
    fn the_host_is_reported_in_preference_to_the_origin() {
        assert_eq!(
            admit(
                Some("evil.example"),
                Some("https://elsewhere.example"),
                &device()
            ),
            Err(ApiErrorCode::HostNotThisDevice),
        );
    }

    #[test]
    fn an_origin_with_no_host_beside_it_is_still_held_to_the_first_rule() {
        assert_eq!(admit(None, Some("http://10.0.0.7"), &device()), Ok(()));
        assert_eq!(
            admit(None, Some("http://evil.example"), &device()),
            Err(ApiErrorCode::OriginNotThisDevice),
        );
    }

    #[test]
    fn the_port_compared_against_is_the_one_the_caller_is_listening_on() {
        let device = DeviceNames {
            hostname: HOSTNAME,
            port: 8080,
        };
        assert_eq!(admit(Some("10.0.0.7:8080"), None, &device), Ok(()));
        assert_eq!(
            admit(Some("10.0.0.7:80"), None, &device),
            Err(ApiErrorCode::HostNotThisDevice),
        );
        // A bare authority means the scheme's default port, which is 80 and is
        // not what this device is bound to.
        assert_eq!(
            admit(Some("10.0.0.7"), None, &device),
            Err(ApiErrorCode::HostNotThisDevice),
        );
    }
}
