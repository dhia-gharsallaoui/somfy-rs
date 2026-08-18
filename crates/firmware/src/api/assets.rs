//! The web UI, in the firmware image.
//!
//! # Why `include_bytes!` and not a partition
//!
//! Because an update has to be one thing. The UI and the firmware are a matched
//! pair — `ui/src/api/generated/` is written from `somfy-api`'s DTOs and CI
//! fails if the two disagree — so an image that could be updated without its UI,
//! or a UI updated without its image, would make a *combination* that was never
//! tested reachable by an operator who did nothing wrong. In one image, an OTA
//! is atomic across both by construction and there is no second thing to
//! version, no second thing to roll back, and no filesystem to mount.
//!
//! It also fits with room to spare, which is the part worth checking rather than
//! asserting: `partitions.csv` measures the app slot at 2,031,616 bytes with the
//! firmware using between 28.6% and 31.1% of it, so the three files below have
//! more than 1.3 MiB of slack to land in.
//!
//! The 32 KB left unallocated at 0x208000 was the alternative — a data region
//! for the UI — and it is both too small (the uncompressed JS alone does not
//! fit) and the wrong shape: filling it would need a filesystem or a second
//! record format, a second flashing step, and a way to notice that the two
//! halves had drifted apart.
//!
//! # Both encodings, and why the identity copy earns its flash
//!
//! `build.rs` writes each file into `OUT_DIR` twice, gzipped and as it stands,
//! and [`Negotiated`] picks between them by `Accept-Encoding`. Serving the
//! compressed bytes unconditionally is what the reference implementation does
//! and it mostly works — every browser sends `Accept-Encoding: gzip` — but it
//! hands binary labelled `text/html` to anything that does not, which includes
//! `curl` with no flags. That is the tool this project debugs devices with, so
//! the choice is between a device that answers `curl` and about 75 KB of a
//! 1.3 MiB surplus. The flash is the cheaper thing.
//!
//! # What comes for free
//!
//! `picoserve::response::File` computes a SHA-1 ETag over the body **at compile
//! time** and answers `304 Not Modified` to a matching `If-None-Match`. So a
//! reload costs a few hundred bytes rather than 25 KB, and nothing here had to
//! be written to get it. The two representations have different bodies and
//! therefore different ETags, which is correct — and why both carry
//! `Vary: Accept-Encoding`, so that a cache between the browser and the device
//! cannot hand the gzipped copy to a client that asked for neither.

use picoserve::io::Read;
use picoserve::request::{Path, Request};
use picoserve::response::{File, IntoResponse, NoContent, ResponseWriter, StatusCode};
use picoserve::routing::{get_service, PathRouter, PathRouterService, RequestHandlerService};
use picoserve::{ResponseSent, Router};

/// Headers every asset carries, whichever representation is chosen.
///
/// `no-cache` rather than `no-store` or a long `max-age`: it means "you may
/// keep this, but revalidate before using it", which is exactly what makes the
/// compile-time ETag useful. A long `max-age` would be faster and would also
/// mean a browser serving last month's UI against this month's firmware, with
/// nothing on the device able to correct it — and the two are only ever
/// released together.
const COMMON: &[(&str, &str)] = &[("Cache-Control", "no-cache"), ("Vary", "Accept-Encoding")];

/// The same, plus the encoding, for the compressed representation.
const COMPRESSED: &[(&str, &str)] = &[
    ("Cache-Control", "no-cache"),
    ("Vary", "Accept-Encoding"),
    ("Content-Encoding", "gzip"),
];

/// One asset, in both the representations this device holds.
///
/// A pair rather than one file with a flag, so that the ETag `File` computes
/// over each body stays attached to the body it describes.
pub struct Negotiated {
    /// Sent when the client accepts gzip.
    compressed: File,
    /// Sent when it does not.
    identity: File,
}

impl Negotiated {
    /// Build both representations of one asset from the bytes `build.rs` wrote.
    const fn new(
        content_type: &'static str,
        identity: &'static [u8],
        compressed: &'static [u8],
    ) -> Negotiated {
        Negotiated {
            compressed: File::with_content_type_and_headers(content_type, compressed, COMPRESSED),
            identity: File::with_content_type_and_headers(content_type, identity, COMMON),
        }
    }
}

/// Whether this request will accept gzip.
///
/// Absent means no. That is what the specification says and it is also the safe
/// direction: the cost of being wrong here is 75 KB of flash already spent,
/// while the cost of guessing yes is a client that receives bytes it cannot
/// read and no way to say so.
///
/// `gzip;q=0` is an explicit refusal and is honoured, because a client that
/// went to the trouble of spelling it out means it. Other `q` values are
/// preferences between codings, and there is only one coding here to prefer.
fn accepts_gzip<R: Read>(request: &Request<'_, R>) -> bool {
    let Some(header) = request.parts.headers().get("Accept-Encoding") else {
        return false;
    };
    header.split(b',').any(|coding| {
        let mut parameters = coding.split(b';');
        let Some(name) = parameters.next() else {
            return false;
        };
        // `*` is "anything you have", which includes gzip.
        (name == "gzip" || name == "*") && !parameters.any(|parameter| parameter == "q=0")
    })
}

impl<State, PathParameters> RequestHandlerService<State, PathParameters> for Negotiated {
    async fn call_request_handler_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        state: &State,
        path_parameters: PathParameters,
        request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        // Delegated whole, rather than reimplemented for each representation:
        // the conditional-request handling and the ETag are `File`'s, and this
        // only decides which `File` answers.
        let file = if accepts_gzip(&request) {
            &self.compressed
        } else {
            &self.identity
        };
        file.call_request_handler_service(state, path_parameters, request, response_writer)
            .await
    }
}

// The bytes themselves, named rather than inlined into the constructors below,
// because [`EMBEDDED_BYTES`] has to measure exactly what is embedded and
// `picoserve::response::File` keeps its body private.
const INDEX_HTML: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index.html"));
const INDEX_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index.html.gz"));
const APP_CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.css"));
const APP_CSS_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.css.gz"));
const APP_JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.js"));
const APP_JS_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.js.gz"));

/// The single-page app's shell.
pub const INDEX: Negotiated = Negotiated::new(File::MIME_HTML, INDEX_HTML, INDEX_HTML_GZ);

/// Its stylesheet.
pub const CSS: Negotiated = Negotiated::new(File::MIME_CSS, APP_CSS, APP_CSS_GZ);

/// Its script.
///
/// **`long_running_const_eval` is allowed here, and the lint is right rather
/// than wrong.** `picoserve::response::File::with_body` computes each asset's
/// ETag as a SHA-1 *at compile time*, so the whole application script is hashed
/// by the const evaluator — and rustc's default step budget for one constant is
/// exceeded once that script passes about a hundred kilobytes, which it did
/// when the diagnostics and backup screens landed (106 KB identity, 42.2 KiB
/// gzipped). The lint's own note says an allow is the remedy when the
/// evaluation is genuinely long rather than looping, which this is: it is a
/// fixed number of rounds over a fixed number of bytes.
///
/// What it costs is build time — a few seconds, once, per asset change — and
/// what it buys is the reason the ETag is compile-time at all: a browser that
/// has the script gets a `304` and no body, which is most of what makes a
/// reload of this UI free on a device with a 512-byte send buffer.
///
/// The alternative, hashing at boot, would put a SHA-1 over 100 KB of flash on
/// the critical path of every start-up, on a device that has a radio to bring
/// up.
#[allow(
    long_running_const_eval,
    reason = "the compile-time ETag hashes the whole application script; see above"
)]
pub const JS: Negotiated = Negotiated::new(File::MIME_JS, APP_JS, APP_JS_GZ);

/// How much flash the three assets take, both representations together.
///
/// Printed at boot beside the heap and stack figures, because it is the one
/// resource this feature spends that nothing else in the image reports — and
/// because `partitions.csv`'s "is 0x1F0000 enough?" arithmetic is only checkable
/// against a number somebody can read.
pub const EMBEDDED_BYTES: usize = INDEX_HTML.len()
    + INDEX_HTML_GZ.len()
    + APP_CSS.len()
    + APP_CSS_GZ.len()
    + APP_JS.len()
    + APP_JS_GZ.len();

/// The compressed half on its own, which is what a browser actually fetches.
const COMPRESSED_BYTES: usize = INDEX_HTML_GZ.len() + APP_CSS_GZ.len() + APP_JS_GZ.len();

/// Say at boot what the UI costs, in both the currency that matters.
///
/// Flash, because `partitions.csv` argues that the app slot is large enough and
/// that argument is only checkable against a number somebody can read; and the
/// compressed size, because that is what crosses the network on a first page
/// load and what `ui/scripts/size.ts` budgets at 200 KB.
pub fn report() {
    crate::logln!(
        "api: web UI embedded — {} bytes of flash, {} of it compressed (what a browser fetches)",
        EMBEDDED_BYTES,
        COMPRESSED_BYTES,
    );
}

/// The fallback: the app shell for a browser, a `404` for the API.
///
/// The split matters in both directions. A deep link like `/shades/3` is the
/// UI's own route and must load the shell, or every bookmark in the house
/// breaks. `/api/v1/typo` is a client mistake and must say so — answering it
/// with `200` and a page of HTML would make a broken request look like a
/// working one, and the UI's `fetch` would fail on the JSON parse with nothing
/// pointing at the cause.
struct SpaShell;

impl PathRouterService for SpaShell {
    async fn call_path_router_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        state: &(),
        path_parameters: (),
        path: Path<'_>,
        request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        if path.encoded().starts_with("/api/") {
            return (StatusCode::NOT_FOUND, NoContent)
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }
        picoserve::routing::RequestHandlerService::call_request_handler_service(
            &INDEX,
            state,
            path_parameters,
            request,
            response_writer,
        )
        .await
    }
}

/// The router every API route is added to.
///
/// The UI's own routes and its deep-link fallback, so that `api::routes` does
/// not have to know whether this build carries a browser front end. The
/// `headless` module is the other implementation of this one function, and the
/// choice between them is a `#[cfg]` at a module declaration in
/// [`crate::api`] — nowhere else.
pub fn base() -> Router<impl PathRouter> {
    Router::from_service(SpaShell)
        .route("/assets/app.css", get_service(CSS))
        .route("/assets/app.js", get_service(JS))
}
