//! The `Origin`/`Host` check, as an extractor every `/api/v1` handler takes.
//!
//! The rules — which headers, what an absent one means, and why — are
//! [`somfy_api::origin`], on the host side of the fence where they are tested.
//! This file is the one thing that cannot live there: reading the two headers
//! out of a `picoserve` request.
//!
//! # An extractor, and the reason is measured rather than stylistic
//!
//! The obvious mechanism is `picoserve::Router::layer`, which wraps the *whole*
//! router and therefore cannot be forgotten by a route added later. It was
//! written that way first, and then measured, because everything in
//! [`crate::heap`] says to measure this: the connection future is monomorphised
//! [`super::HTTP_TASKS`] times into DRAM that the Wi-Fi driver's heap is carved
//! out of.
//!
//! On an ESP32-S3, `firmware::api::connection::POOL`:
//!
//! | | bytes | per task |
//! |---|---|---|
//! | no check at all | 67,840 | 16,960 |
//! | `Router::layer`, doing nothing but calling through | 72,736 | 18,184 |
//! | `Router::layer`, full check | 73,888 | 18,472 |
//! | **this extractor, on every `/api/v1` handler** | **67,840** | **16,960** |
//!
//! **The layer costs 6,048 bytes of DRAM and the extractor costs none.** Nearly
//! all of the layer's cost is structural and not the check: an empty
//! pass-through layer already costs 4,896, because `call_layer` is an `async fn`
//! that holds the entire inner router's future across its own await, and the
//! compiler does not overlap that with the refusal branch. The refusal body is
//! free either way — a bare `403`, a static `&str` and the full `JsonBody` all
//! measured 73,888.
//!
//! 6,048 bytes is not affordable. It comes out of the Wi-Fi heap, and on the
//! ESP32-C3 that heap would fall from 55 KiB to 48 KiB against a worst
//! announcement peak of 54,424 bytes — a board that panics part-way through
//! publishing its discovery configs. See [`crate::heap::DRAM_FOR_STACK_AND_HEAP`].
//!
//! # What that costs, and how it is contained
//!
//! An extractor is per handler, so it is a discipline rather than a
//! construction: **a new `/api/v1` route that does not take
//! [`FromThisDevice`] is unprotected, and nothing will say so.** Three things
//! hold it up, and they are the honest total:
//!
//! 1. Every handler in [`super::routes`] takes it today — the reads as well as
//!    the writes, so there is no "which ones need it" judgement to get wrong.
//! 2. `routes.rs`'s own module documentation carries the rule at the top, in the
//!    same audit table a reviewer already reads a diff against.
//! 3. It is a parameter, so its absence is visible in the diff that adds a
//!    route rather than in behaviour a year later.
//!
//! **The static asset routes and the SPA fallback deliberately do not take it**
//! — see [`super::routes`] for that boundary. They serve the compiled UI, which
//! is public bytes with nothing to disclose and nothing to actuate.
//!
//! # It needs no address, which is the other reason this shape works
//!
//! `embassy_net::Stack` is not `Sync`, so an extractor — which reaches its
//! inputs through a `static` rather than through a captured value — could not
//! hold one. That forced the question of whether the check needs this device's
//! address at all, and [`somfy_api::origin`] answers no: an attacker cannot
//! rebind an IP literal, only a name. The rule that falls out is stronger in
//! practice as well as cheaper, because it keeps working through a DHCP lease
//! change that nothing in the web server would otherwise hear about.

use picoserve::extract::FromRequestParts;
use picoserve::request::RequestParts;
use somfy_api::origin::DeviceNames;

use crate::api::routes::{refuse, Refusal};

/// Proof that a request was addressed to this device and made by a page this
/// device served.
///
/// A handler that takes one cannot run without the check having passed: the
/// extractor's rejection is written by `picoserve` before the handler body is
/// entered, and before the request body is even parsed — a refused `POST` never
/// reaches `Json<T>`.
pub struct FromThisDevice;

impl<'r> FromRequestParts<'r, ()> for FromThisDevice {
    type Rejection = Refusal;

    async fn from_request_parts(
        _state: &'r (),
        request_parts: &RequestParts<'r>,
    ) -> Result<Self, Refusal> {
        let headers = request_parts.headers();
        let hostname = crate::identity::hostname();
        let device = DeviceNames {
            hostname: &hostname,
            port: super::PORT,
        };
        // A header whose value is not UTF-8 is read as absent rather than as a
        // refusal. It cannot name this device — every name this device has is
        // ASCII — so the only question is which refusal it draws, and "absent"
        // keeps the rule single: absence means not-a-browser, and a browser does
        // not send bytes like these.
        let verdict = somfy_api::origin::admit(
            headers.get("host").and_then(|value| value.as_str().ok()),
            headers.get("origin").and_then(|value| value.as_str().ok()),
            &device,
        );

        let Err(code) = verdict else {
            return Ok(FromThisDevice);
        };

        // Said out loud, because this is the one refusal on the device that
        // means somebody else's page is talking to it, and a console line is the
        // only way an operator finds out that it happened at all.
        crate::logln!(
            "api: refusing {} {} — {:?}. This device answers to its own address and to \
             {}.local; a request naming anything else was addressed elsewhere, or was made \
             by a page this device did not serve. See somfy_api::origin.",
            request_parts.method(),
            request_parts.path(),
            code,
            device.hostname,
        );
        Err(refuse(code))
    }
}
