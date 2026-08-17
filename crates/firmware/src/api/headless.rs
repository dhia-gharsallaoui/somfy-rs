//! The web server without the web UI.
//!
//! The other implementation of [`base`], selected when the `ui` feature is off.
//! It is one function and no routes, which is the whole of what "the API with
//! no browser front end" means: `/api/v1/…` is unchanged, and everything else
//! is a `404` because there is genuinely nothing else in the image.
//!
//! # What this build is for
//!
//! An integration or a test rig that speaks to `/api/v1/` directly and has no
//! use for a single-page app. It keeps about 100 KB of assets out of flash,
//! which is not the reason it exists — the reason is that `ui` and `http` are
//! separable, and a feature that is never built without its sibling is not a
//! feature, it is a comment.
//!
//! # Why a second module rather than a `#[cfg]` in the router
//!
//! Because the router is where the API's shape is written, and a build flag in
//! the middle of it would make that shape conditional. Here the condition is a
//! module declaration in [`crate::api`], and the two modules agree on one
//! signature — which is also what makes it obvious that turning the UI off
//! cannot change an API route.

use picoserve::routing::PathRouter;
use picoserve::Router;

/// The router every API route is added to.
///
/// `Router::new()` answers `404` to everything it is not given a route for,
/// which is the correct answer in this build: without the `ui` feature there is
/// no app shell to serve a deep link with, so `/shades/3` is a path this device
/// really does not have.
pub fn base() -> Router<impl PathRouter> {
    Router::new()
}

/// Say at boot that there is no UI, so an operator pointing a browser at this
/// device and getting `404` has already been told why.
pub fn report() {
    esp_println::println!(
        "api: no web UI in this image (built without the `ui` feature) — /api/v1 is served, \
         everything else answers 404"
    );
}
