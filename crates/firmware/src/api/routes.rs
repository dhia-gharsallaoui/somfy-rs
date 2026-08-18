//! The `/api/v1/` surface, and the UI it is served beside.
//!
//! # This file contains no rules
//!
//! That is the design constraint, stated first because it is the one worth
//! checking a diff against. Every handler below does exactly three things:
//! parse a request, hand it to [`crate::rpc`], and turn the answer into a
//! status code. Position arithmetic, address allocation, name and travel-time
//! validation, and the decision about what may be paired all live behind that
//! seam, in the same functions the MQTT session reaches.
//!
//! Concretely, and this is the list to audit:
//!
//! | What | Where it is decided | Also reached by |
//! |---|---|---|
//! | What a shade may be called, be, and travel in | `somfy_api::CreateShadeDto::to_config` / `PatchShadeDto::apply` | — (HTTP is the only writer today) |
//! | What a command does to a shade | `somfy_tasks::StateMachine::apply` via `tasks::run_command` | the MQTT command channel |
//! | What adding, editing or removing a shade does | `tasks::apply_edit` | `edits::EditChannel` |
//! | Whether a shade may be paired | `somfy_domain::RemoteIdentity::is_allocated` | `inventory::Inventory::snapshot` |
//! | Whether a shade has Home Assistant entities at all | `tasks::announce_shade` | `inventory::Inventory::snapshot` |
//! | Which status a refusal carries | `somfy_api::ApiErrorCode::http_status` | `ui/mock/plugin.ts` |
//! | Whether a command may be sent *yet* | `somfy_tasks::CommandLimiter` via `StateMachine::apply` | the MQTT command channel |
//! | Whether the caller may ask at all | `somfy_api::origin::admit` via [`FromThisDevice`] | — (HTTP is the only door with headers) |
//! | Whether an uploaded image may be written and booted | `somfy_ota::image::Verifier` via `crate::ota` | — (HTTP is the only way in) |
//!
//! # Every handler below takes [`FromThisDevice`], and a new one must too
//!
//! **This is the one rule in this file that a reviewer has to enforce by
//! reading**, so it is stated where a diff is read rather than left to the
//! module that implements it. `_from_this_device: FromThisDevice` is the
//! `Origin`/`Host` check — the non-authentication half of design spec §7.3, and
//! the thing that stops a page in somebody else's browser tab driving these
//! shades. It is an extractor rather than a `picoserve::Router::layer`, which
//! *would* have been unforgettable, because the layer costs 6,048 bytes of DRAM
//! and the extractor costs none; [`crate::api::origin`] carries that
//! measurement and the argument.
//!
//! The consequence is the discipline: **an `/api/v1` handler added without that
//! parameter is unprotected, and nothing will say so.** Every handler here has
//! it today — the reads as well as the writes, so there is no judgement about
//! which ones need it — and it sits after any path parameters and before the
//! body extractor, because that is the order `picoserve` requires and the order
//! in which a refused `POST` never reaches `Json<T>`.
//!
//! The asset routes and the SPA fallback deliberately do **not** take it. They
//! serve the compiled UI: public bytes with nothing to disclose and nothing to
//! actuate, and `shell::base()` is a fallback rather than a handler with a
//! signature to extend.
//!
//! **One `/api/v1` route is not a handler function and so cannot take it as a
//! parameter: [`OtaUpload`].** It is a `RequestHandlerService`, because a
//! firmware image cannot be extracted into a value the way every `picoserve`
//! body extractor produces one. It therefore calls
//! `FromThisDevice::from_request_parts` itself, as its **first** statement,
//! before the session lock and before a byte of the body is read. That is the
//! one place in this file where the rule above is kept by hand rather than by a
//! signature, and it is the route where it matters most — a `POST` there
//! replaces the firmware.
//!
//! # The contract is `ui/mock/plugin.ts`
//!
//! That Vite plugin serves these exact paths so the same client code runs
//! against the mock and against this device with no "mock mode" branch. Where
//! its comments defend a status code — `202` for pairing because RTS never
//! reports success, `201` with `Location` because a create that answered `200`
//! would leave the client guessing the id, `409` rather than `507` for a full
//! registry — the same choice is made here, and
//! [`somfy_api::ApiErrorCode::http_status`] is where the shared half of it
//! lives.
//!
//! # One narrow divergence in how a bad name is refused
//!
//! `picoserve`'s `Json` extractor unescapes strings through a 32-byte scratch
//! buffer, and it uses it **only when the string contains a backslash** — the
//! common path, including every accented character a browser sends, is borrowed
//! straight out of the request and never touches it. So the one input that
//! behaves differently here from the mock is a name that is *both* over 32
//! bytes once unescaped *and* contains a `\"`, `\\` or `\uXXXX` sequence: it is
//! refused as a malformed body rather than as [`ApiErrorCode::NameTooLong`].
//!
//! Both answers are `400` and both names were going to be refused — 32 bytes is
//! the domain's own limit — so what is lost is the *reason*, on an input a
//! person reaches by putting a quotation mark in a name they typed too long.
//! Recorded rather than fixed because the fix is a larger buffer in every
//! connection task's future, which is the resource this module is tightest on.
//!
//! # Deep links, and why there is a catch-all
//!
//! The UI routes in the browser with the history API, so `/shades/3` is a URL a
//! person can reload or bookmark, and the device has to answer it with the app
//! shell. [`SpaShell`] does that for anything unmatched — **except** under
//! `/api/`, where an unknown path is a client error and must say so rather than
//! returning HTML with a `200`.
//!
//! Enumerating the UI's routes here instead was the alternative, and it is the
//! kind of second copy this project spends its rules avoiding: the list lives
//! in `ui/src/app.tsx`, nothing would tie the two together, and the failure
//! would be a bookmark that stopped working one release after somebody added a
//! screen.

use embassy_sync::signal::Signal;
use embassy_time::Instant;
use picoserve::extract::Json;
use picoserve::io::{Read, Write};
use picoserve::response::chunked::{ChunkWriter, ChunkedResponse, Chunks, ChunksWritten};
use picoserve::response::{ws, Content, IntoResponse, NoContent, ResponseWriter, StatusCode};
use picoserve::routing::{get, parse_path_segment, post, PathRouter};
use picoserve::{ResponseSent, Router};
use serde::Serialize;
use somfy_api::{
    ApiErrorCode, ApiErrorDto, CalibrationStepDto, ChipDto, CommandDto, CreateShadeDto, HeapDto,
    MqttUpdateDto, PatchShadeDto, SettingsDto, StackDto, SystemDto, TrialDecisionDto,
    WifiUpdateDto,
};
use somfy_domain::{GroupId, ShadeId};
use somfy_tasks::ControlCommand;

use crate::api::events::Events;
use crate::api::origin::FromThisDevice;
use crate::api::shell;
use crate::edits::ShadeEdit;
use crate::rpc::{Reply, Request as Rpc, RPC};

/// Longest JSON any one entity serialises to, in bytes.
///
/// **Not counted here — taken from [`somfy_api::SHADE_JSON_MAX_BYTES`]**, which
/// lives beside the DTOs and is checked by `somfy-api`'s `tests/wire_width.rs`
/// against the widest legal value of each type, from both sides: never over it,
/// and never more than 128 bytes under it.
///
/// It was counted here once, at 512, and the count was wrong by 160 bytes. The
/// widest shade is 540: a `heapless::String<32>` name of control characters
/// escapes to `\u00XX` six bytes at a time, and nothing refuses such a name —
/// `picoserve`'s inbound unescape buffer is exactly 32 bytes, which is enough to
/// deliver thirty-two of them, and the result is written to flash. So one shade
/// could have made `GET /api/v1/shades` answer with malformed JSON forever,
/// including for the screen an operator would use to delete it.
const ENTITY_JSON_BYTES: usize = somfy_api::SHADE_JSON_MAX_BYTES;

/// How long a client is asked to wait when every WebSocket slot is taken.
///
/// A policy figure, and its job is to make the refusal *actionable* rather than
/// to predict anything: slots are freed when a tab closes or when TCP notices a
/// dead peer, which `picoserve` bounds at 45 s. Five seconds is short enough
/// that a genuinely transient collision — two tabs opening at once, one
/// reloading — resolves without the user doing anything, and the UI's own
/// reconnect backoff takes over from there.
const RETRY_AFTER_S: &str = "5";

/// The router's type, which is otherwise unnameable.
///
/// `Router::route` returns `Router<impl PathRouter>`, so the type of a chain of
/// them exists but has no spelling. An `#[embassy_executor::task]` cannot be
/// generic — it allocates a static sized to one concrete future — so the task
/// below needs a name for it, and this is `picoserve`'s own way of providing
/// one. It is why `main.rs` carries `#![feature(impl_trait_in_assoc_type)]`.
pub type AppRouter = <App as picoserve::AppWithStateBuilder>::PathRouter;

/// The app, as something with a nameable router type.
pub struct App;

impl picoserve::AppBuilder for App {
    type PathRouter = impl PathRouter;

    fn build_app(self) -> Router<Self::PathRouter> {
        router()
    }
}

/// Build the router.
///
/// The base is [`SpaShell`] rather than `Router::new()`, because
/// `Router::new()`'s fallback is a bare 404 and this device's fallback has a
/// job — see the module docs. Routes added afterwards are matched first.
fn router() -> Router<impl PathRouter> {
    shell::base()
        .route("/api/v1/shades", get(list_shades).post(create_shade))
        .route(
            ("/api/v1/shades", parse_path_segment::<u8>()),
            get(get_shade).patch(patch_shade).delete(delete_shade),
        )
        .route(
            ("/api/v1/shades", parse_path_segment::<u8>(), "/pair"),
            post(pair_shade),
        )
        .route(
            (
                "/api/v1/shades",
                parse_path_segment::<u8>(),
                "/confirm-pairing",
            ),
            post(confirm_pairing),
        )
        .route(
            ("/api/v1/shades", parse_path_segment::<u8>(), "/command"),
            post(command_shade),
        )
        // Same `(&str, id, &str)` shape as the three above, which is why it adds
        // no frame to the router's monomorphised call chain — see
        // `crate::heap::REQUEST_CHAIN_BYTES`, and `somfy_api::CalibrationStepDto`
        // for why the whole conversation is one route with a step in the body.
        .route(
            ("/api/v1/shades", parse_path_segment::<u8>(), "/calibrate"),
            post(calibrate_shade),
        )
        .route("/api/v1/groups", get(list_groups))
        .route(
            ("/api/v1/groups", parse_path_segment::<u8>(), "/command"),
            post(command_group),
        )
        .route("/api/v1/rooms", get(list_rooms))
        // Settings. Every one is a plain literal path — the same
        // monomorphisation family `/api/v1/shades`, `/api/v1/groups` and
        // `/api/v1/rooms` are already in — so none of them deepens
        // `crate::heap::REQUEST_CHAIN_BYTES`. That is a deliberate choice of URL
        // shape and not a happy accident; see the note on that constant.
        .route("/api/v1/settings", get(get_settings))
        .route(
            "/api/v1/settings/wifi",
            picoserve::routing::put(start_wifi_trial),
        )
        // One route for both endings of a trial, with the decision in the
        // body. Not tidiness: `picoserve`'s router is a type per route and
        // there are `HTTP_TASKS` copies of the resulting future, statically
        // allocated out of the same DRAM the Wi-Fi heap comes from. See
        // `somfy_api::TrialDecisionDto`, and `/calibrate` for the precedent.
        .route("/api/v1/settings/wifi/trial", post(settle_wifi_trial))
        .route(
            "/api/v1/settings/mqtt",
            picoserve::routing::put(save_mqtt).delete(clear_mqtt),
        )
        // The `system` resource design spec §7.2 promises. Three plain literal
        // paths, so the same `&str` monomorphisation family the settings routes
        // are already in — none of them deepens
        // `crate::heap::REQUEST_CHAIN_BYTES`, which is the reason the URLs are
        // shaped this way rather than as `/api/v1/system?what=log`.
        .route("/api/v1/system", get(get_system).delete(forget_the_past))
        .route("/api/v1/system/log", get(get_log))
        // Two methods on one path, the shape `/api/v1/settings/mqtt` is already
        // in: `GET` is the export and `POST` is the restore. They are one
        // resource — the backup this device has — read one way and written the
        // other, and a second path would have been a second route out of the
        // DRAM the connection task futures come from.
        .route(
            "/api/v1/system/backup",
            get(get_backup).post_service(RestoreUpload),
        )
        .route("/api/v1/system/restore", get(get_restore))
        .route("/api/v1/events", get(events))
        // The one route that is a *service* rather than a handler function,
        // because it is the one that must not have its body extracted for it.
        // A firmware image is over a megabyte and this device's whole heap is
        // under seventy kilobytes, so the body is streamed to flash a page at a
        // time and never exists anywhere in one piece. See [`OtaUpload`].
        .route("/api/v1/ota", picoserve::routing::post_service(OtaUpload))
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// A JSON body at a status of the caller's choosing, into a buffer of `N`.
///
/// `picoserve`'s own `Json` response hardcodes `200`, and the type behind it is
/// private, so `201 Created` and `200 OK` after a `PATCH` need this. It is the
/// entire cost of that gap.
///
/// # Why the capacity is a parameter, and what was tried instead
///
/// [`Content::write_content`] is `async` and its scratch is live across the
/// write, so the buffer sits **inside the connection task's future** — and
/// there are [`crate::api::HTTP_TASKS`] of those, statically allocated out of
/// the DRAM the Wi-Fi driver's heap is carved from. Every byte here is spent
/// four times, in Wi-Fi headroom, on every boot including the boots where
/// nobody opens the UI.
///
/// The settings document is much wider than a shade — three of its strings can
/// hold text an access point or a broker chose, and control characters escape
/// six bytes at a time — so one shared constant would have to be the larger of
/// the two and every shade response would carry the difference. A parameter
/// lets each call site name the bound its own type is measured against in
/// `somfy-api`.
///
/// **`picoserve::response::Json` was measured as the alternative and is worse
/// here**, which is worth recording so it is not tried again: it re-serialises
/// in 128-byte windows instead of holding a buffer, but its `JsonStream` keeps
/// the value *and* a serializer state live across the write, and the connection
/// task future grew from 16,960 bytes to 18,904 — 7,776 bytes of DRAM across
/// the four tasks, against the 2,688 the wider buffer costs.
struct JsonBody<T: Serialize, const N: usize>(T);

impl<T: Serialize, const N: usize> Content for JsonBody<T, N> {
    fn content_type(&self) -> &'static str {
        "application/json; charset=utf-8"
    }

    fn content_length(&self) -> usize {
        // Serialised twice — once to measure, once to send — because
        // `Content::content_length` is synchronous and has nowhere to keep the
        // result. It is a few hundred bytes of `memcpy` on a path a person
        // walks a handful of times per page, against the alternative of a
        // chunked response for a single object, which costs the client a
        // streaming parse for no gain.
        //
        // Both halves fall back to zero on an encoder that could not fit the
        // value, so the header and the body agree and the connection is not
        // desynchronised. That is damage control, not correctness: the client
        // gets `200 OK` with an empty body. It is unreachable — see
        // `ENTITY_JSON_BYTES` — and `write_content` says so out loud if it ever
        // happens, which is the half a header cannot.
        let mut scratch = [0u8; N];
        serde_json_core::to_slice(&self.0, &mut scratch).unwrap_or(0)
    }

    async fn write_content<W: Write>(self, mut writer: W) -> Result<(), W::Error> {
        let mut scratch = [0u8; N];
        let written = match serde_json_core::to_slice(&self.0, &mut scratch) {
            Ok(written) => written,
            Err(_) => {
                crate::logln!(
                    "api: an entity did not fit {} bytes and was answered as an empty body — \
                     this is a bug in somfy_api::SHADE_JSON_MAX_BYTES, not in the request",
                    ENTITY_JSON_BYTES,
                );
                0
            }
        };
        writer.write_all(&scratch[..written]).await
    }
}

/// A refusal: the status the code carries, and the code as the body.
///
/// The status is not chosen here — [`ApiErrorCode::http_status`] decides it,
/// beside the variant it describes, so that a code added in `somfy-api` cannot
/// reach this router without somebody having said what it means over HTTP.
///
/// It carries the whole [`ApiErrorDto`] rather than the bare code, because a
/// settings rejection also names the field it is about — `ApiErrorDto::field`
/// — and that field is what lets the settings form highlight the input the
/// operator has to fix. `From<ApiErrorCode>` fills it in as absent, so every
/// other refusal is written exactly as it was.
pub struct Refusal(ApiErrorDto);

impl IntoResponse for Refusal {
    async fn write_to<R: Read, W: ResponseWriter<Error = R::Error>>(
        self,
        connection: picoserve::response::Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        (
            StatusCode::new(self.0.code.http_status()),
            JsonBody::<_, ENTITY_JSON_BYTES>(self.0),
        )
            .write_to(connection, response_writer)
            .await
    }
}

/// A refusal from a bare code, which is what every non-settings path has.
///
/// `pub(crate)` for one caller outside this module: the `Origin`/`Host` layer,
/// which refuses before any handler runs and must answer in the same shape a
/// handler would — see [`crate::api::origin`].
pub fn refuse(code: ApiErrorCode) -> Refusal {
    Refusal(ApiErrorDto::from(code))
}

/// The state task did not answer.
///
/// A bare `503` with no body, deliberately: [`ApiErrorCode`]'s admission test
/// is that every variant is something a user can act on, and "this device is
/// faulty" is not. The UI's `parseApiErrorCode` treats a body it cannot read as
/// "the device did not say why", which is exactly true here and better than
/// borrowing a code that means something else.
struct Unavailable;

impl IntoResponse for Unavailable {
    async fn write_to<R: Read, W: ResponseWriter<Error = R::Error>>(
        self,
        connection: picoserve::response::Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            ("Retry-After", RETRY_AFTER_S),
            NoContent,
        )
            .write_to(connection, response_writer)
            .await
    }
}

// ---------------------------------------------------------------------------
// Collections
// ---------------------------------------------------------------------------

/// Which collection a listing walks.
///
/// One type for three endpoints because the walk is identical and only the
/// request and the DTO differ — and because a chunked body has to name its own
/// content type, which a closure could not.
#[derive(Clone, Copy)]
enum Collection {
    Shades,
    Groups,
    Rooms,
}

impl Chunks for Collection {
    fn content_type(&self) -> &'static str {
        "application/json; charset=utf-8"
    }

    /// Stream the array, asking the state task for one entity at a time.
    ///
    /// Chunked because the length is not knowable in advance without either
    /// walking the registry twice or holding every DTO at once, and the second
    /// is a 2.5 KB static this device pays for in Wi-Fi headroom on every boot.
    /// Streaming also hands the executor back between entities, so a listing
    /// cannot sit between the state task and an arrival stop.
    async fn write_chunks<W: Write>(
        self,
        mut writer: ChunkWriter<W>,
    ) -> Result<ChunksWritten, W::Error> {
        let mut scratch = [0u8; ENTITY_JSON_BYTES + 1];
        writer.write_chunk(b"[").await?;

        let mut slot = 0u8;
        let mut first = true;
        loop {
            // The separator goes in front of the element rather than after it,
            // so the array cannot end in a trailing comma however it stops.
            let start = usize::from(!first);
            scratch[0] = b',';

            // **`Option`, not a length.** An encoder that could not fit the
            // entity must not be read as "wrote nothing": that would emit the
            // separator with no element after it and hand the client
            // `[{…},,{…}]`, which is not JSON — and would do it on every
            // request until somebody deleted the offending shade through the
            // very list endpoint it had just broken.
            //
            // `somfy_api::SHADE_JSON_MAX_BYTES` is measured against the widest
            // legal value of each type, so this cannot happen; it is handled
            // rather than asserted because a panic here reboots the board.
            let (encoded, next) = match self {
                Collection::Shades => match RPC.call(Rpc::ShadeFrom(slot)).await {
                    Some(Reply::Shade(Some(shade))) => (
                        serde_json_core::to_slice(&shade, &mut scratch[start..]).ok(),
                        shade.id.checked_add(1),
                    ),
                    // Either the walk is done, or the state task did not
                    // answer. Both end the array here: a truncated list is
                    // still valid JSON and the client sees what exists, which
                    // beats a response that never terminates.
                    _ => break,
                },
                Collection::Groups => match RPC.call(Rpc::GroupFrom(slot)).await {
                    Some(Reply::Group(Some(group))) => (
                        serde_json_core::to_slice(&group, &mut scratch[start..]).ok(),
                        group.id.checked_add(1),
                    ),
                    _ => break,
                },
                Collection::Rooms => match RPC.call(Rpc::RoomFrom(slot)).await {
                    Some(Reply::Room(Some(room))) => (
                        serde_json_core::to_slice(&room, &mut scratch[start..]).ok(),
                        room.id.checked_add(1),
                    ),
                    _ => break,
                },
            };

            match encoded {
                Some(written) => {
                    writer.write_chunk(&scratch[..start + written]).await?;
                    // Only once something has actually been written, so the
                    // next element does not inherit a separator for an element
                    // that was skipped.
                    first = false;
                }
                None => crate::logln!(
                    "api: an entity did not fit {} bytes and was left out of the list — \
                     this is a bug in somfy_api::SHADE_JSON_MAX_BYTES, not in the request",
                    ENTITY_JSON_BYTES,
                ),
            }

            let Some(next) = next else { break };
            slot = next;
        }

        writer.write_chunk(b"]").await?;
        writer.finalize().await
    }
}

async fn list_shades(_from_this_device: FromThisDevice) -> impl IntoResponse {
    ChunkedResponse::new(Collection::Shades)
}

async fn list_groups(_from_this_device: FromThisDevice) -> impl IntoResponse {
    ChunkedResponse::new(Collection::Groups)
}

async fn list_rooms(_from_this_device: FromThisDevice) -> impl IntoResponse {
    ChunkedResponse::new(Collection::Rooms)
}

// ---------------------------------------------------------------------------
// One shade
// ---------------------------------------------------------------------------

async fn get_shade(id: u8, _from_this_device: FromThisDevice) -> impl IntoResponse {
    match RPC.call(Rpc::Shade(ShadeId(id))).await {
        Some(Reply::Shade(Some(shade))) => {
            Ok((StatusCode::OK, JsonBody::<_, ENTITY_JSON_BYTES>(shade)))
        }
        Some(Reply::Shade(None)) => Err(Ok(refuse(ApiErrorCode::NotFound))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::NotFound))),
        None => Err(Err(Unavailable)),
    }
}

/// `201`, the created shade, and a `Location` — never `200`.
///
/// The answer carries what the request could not: the id the registry assigned
/// and the address this controller allocated. A `200` with a body would leave
/// the client to find the id inside it, which is what `Location` is for.
async fn create_shade(
    _from_this_device: FromThisDevice,
    Json(request): Json<CreateShadeDto>,
) -> impl IntoResponse {
    match RPC.call(Rpc::Edit(ShadeEdit::Add { request })).await {
        Some(Reply::Created(id)) => match RPC.call(Rpc::Shade(id)).await {
            Some(Reply::Shade(Some(shade))) => Ok((
                StatusCode::CREATED,
                ("Location", location_of(id)),
                JsonBody::<_, ENTITY_JSON_BYTES>(shade),
            )),
            // The shade was created and then could not be read back, which
            // means something removed it in between. Reported as created but
            // missing rather than as a failure, because it *was* created.
            _ => Err(Ok(refuse(ApiErrorCode::NotFound))),
        },
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

/// `200` with the whole shade, not `204`.
///
/// The client needs the recomputed calibration sources back: whether a travel
/// time counts as measured is derived from its value, so a `PATCH` that
/// answered "no content" would make the UI guess at the very thing the edit was
/// for.
async fn patch_shade(
    id: u8,
    _from_this_device: FromThisDevice,
    Json(patch): Json<PatchShadeDto>,
) -> impl IntoResponse {
    let id = ShadeId(id);
    match RPC
        .call(Rpc::Edit(ShadeEdit::Reconfigure { id, patch }))
        .await
    {
        Some(Reply::Done) => match RPC.call(Rpc::Shade(id)).await {
            Some(Reply::Shade(Some(shade))) => {
                Ok((StatusCode::OK, JsonBody::<_, ENTITY_JSON_BYTES>(shade)))
            }
            _ => Err(Ok(refuse(ApiErrorCode::NotFound))),
        },
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

async fn delete_shade(id: u8, _from_this_device: FromThisDevice) -> impl IntoResponse {
    match RPC
        .call(Rpc::Edit(ShadeEdit::Remove { id: ShadeId(id) }))
        .await
    {
        Some(Reply::Done) => Ok((StatusCode::NO_CONTENT, NoContent)),
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

/// `202 Accepted`, and it can never be `200 OK`.
///
/// RTS is one-way: the device queues a `Prog` burst and never learns whether
/// the motor took it. The only acknowledgement that exists anywhere in this
/// protocol is the shade jogging, watched by a person standing at it. `202` is
/// the honest code for "this has been accepted for processing" with no claim
/// about the outcome — and the outcome genuinely lives outside the system.
async fn pair_shade(id: u8, _from_this_device: FromThisDevice) -> impl IntoResponse {
    match RPC.call(Rpc::Pair(ShadeId(id))).await {
        Some(Reply::Done) => Ok((StatusCode::ACCEPTED, NoContent)),
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

/// `200 OK` with the whole shade — and unlike `/pair` it may say `200`.
///
/// # What this route is, in one sentence
///
/// The operator reporting that they commanded the shade and watched it move.
/// That is a fact about a person, not about a motor, and it is the only kind of
/// fact this protocol permits: RTS is one-way, and
/// `somfy_domain::PairingState` carries the argument.
///
/// # Why `200` here and `202` next door
///
/// `/pair` answers `202` because the device has queued something whose outcome
/// it will never learn. This one has no such gap: recording the report and
/// announcing the entities both happen before the response is written, so by
/// the time the client sees `200` the shade really does have entities on the
/// broker. The body is the recomputed [`somfy_api::ShadeDto`], because the
/// client needs the new `pairingState` in order to stop presenting the shade as
/// an unfinished setup — the same reason `PATCH` answers with a body.
///
/// # Why a route rather than a `PATCH` field
///
/// A `PATCH` field would be settable in both directions, and the other
/// direction retires a working shade's entities — an automation pointing at a
/// cover that stops existing because a client round-tripped a whole `ShadeDto`
/// back with one field stale. There is deliberately no way to say
/// "unconfirmed"; removing the shade is the way to undo this, and it is
/// deliberately the loud way. It is also not idempotent in the shape `PATCH`
/// implies — it *is* idempotent, but what it triggers is a publish, and a
/// verb whose job is "set these fields" is the wrong place for that.
async fn confirm_pairing(id: u8, _from_this_device: FromThisDevice) -> impl IntoResponse {
    let id = ShadeId(id);
    match RPC.call(Rpc::Edit(ShadeEdit::ConfirmPairing { id })).await {
        Some(Reply::Done) => match RPC.call(Rpc::Shade(id)).await {
            Some(Reply::Shade(Some(shade))) => {
                Ok((StatusCode::OK, JsonBody::<_, ENTITY_JSON_BYTES>(shade)))
            }
            _ => Err(Ok(refuse(ApiErrorCode::NotFound))),
        },
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn command_shade(
    id: u8,
    _from_this_device: FromThisDevice,
    Json(command): Json<CommandDto>,
) -> impl IntoResponse {
    dispatch(ControlCommand::Shade {
        id: ShadeId(id),
        command: command.to_domain(),
    })
    .await
}

/// Group commands are per-shade fan-out, not a single group frame — the domain
/// plans one transmission per member. Pairing is deliberately absent here and
/// has no group route at all: fanned across a group it is a `Prog` burst at
/// every shade in the house with nobody standing at any of them, which
/// `Controller::command_group` refuses outright.
async fn command_group(
    id: u8,
    _from_this_device: FromThisDevice,
    Json(command): Json<CommandDto>,
) -> impl IntoResponse {
    dispatch(ControlCommand::Group {
        id: GroupId(id),
        command: command.to_domain(),
    })
    .await
}

/// One step of a guided travel-time calibration.
///
/// `204` for every step that is accepted, and the body is deliberately empty
/// even for `finish`: what the run measured is now part of the shade, so the
/// client re-reads `GET /api/v1/shades/{id}` and gets the travel times, the
/// bands **and** their `calibrationSource` — which is the thing that actually
/// changed and the thing the screen has to show. Returning the raw measurement
/// instead would be a second, narrower view of the same fact, free to disagree
/// with the first.
///
/// It is not `202`: unlike `/pair`, the device is not making a claim about
/// something it will never learn. `begin` has queued a traverse and started a
/// clock, and `finish` has stored numbers — both are facts about this device
/// that the client can act on immediately.
async fn calibrate_shade(
    id: u8,
    _from_this_device: FromThisDevice,
    Json(step): Json<CalibrationStepDto>,
) -> impl IntoResponse {
    match RPC.call(Rpc::Calibrate(ShadeId(id), step)).await {
        Some(Reply::Done) => Ok((StatusCode::NO_CONTENT, NoContent)),
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

/// Hand one command to the state task and render what it says.
///
/// `204`: the command has been applied to this device's model and its frames
/// are on the radio queue. It is not a claim that a motor moved — nothing in
/// this protocol can make that claim — but unlike pairing it *is* a claim that
/// the device accepted the instruction and updated the position it will report,
/// which is a real thing the client can act on.
async fn dispatch(
    command: ControlCommand,
) -> Result<impl IntoResponse, Result<Refusal, Unavailable>> {
    match RPC.call(Rpc::Command(command)).await {
        Some(Reply::Done) => Ok((StatusCode::NO_CONTENT, NoContent)),
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Upgrade to a WebSocket, if there is a slot.
///
/// [`Events::admit`] takes both the permit that bounds how many exist and the
/// subscription the deltas arrive on, or neither — see [`crate::api::events`]
/// for why the subscription alone was not a sufficient bound, and for the
/// lockout that oversight would have reproduced.
async fn events(
    _from_this_device: FromThisDevice,
    upgrade: ws::WebSocketUpgrade,
) -> impl IntoResponse {
    match Events::admit() {
        Some(events) => Ok(upgrade.on_upgrade(events)),
        None => {
            crate::logln!(
                "api: refusing a websocket — {} of {} slots are in use. REST is unaffected: \
                 {} of this device's {} connection tasks can never be taken by one.",
                Events::held(),
                crate::api::events::WS_MAX,
                crate::api::REST_TASKS_RESERVED,
                crate::api::HTTP_TASKS,
            );
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                ("Retry-After", RETRY_AFTER_S),
                NoContent,
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Settings
//
// # The one thing to keep in mind reading these
//
// **No handler here holds a secret and none can send one.** The write-only rule
// is `somfy_api`'s, carried by the types: nothing in `SettingsDto` has a field a
// passphrase could be written into, and a `SecretDto::Keep` is resolved against
// flash by the state task rather than by anything on this side of the seam. The
// one exception is deliberate and goes the other way — `Reply::WifiCandidate`
// carries a resolved passphrase *inbound* from the state task to the radio,
// because that is where a credential has to end up to be tried.
// ---------------------------------------------------------------------------

/// What the device is provisioned with, minus every secret.
///
/// The trial half is read here rather than asked of the state task: a live trial
/// is not in flash, it belongs to `crate::trial`, and the state task has no
/// business knowing about the radio.
/// The buffer is [`somfy_api::SETTINGS_JSON_MAX_BYTES`] rather than
/// [`ENTITY_JSON_BYTES`], because this document is the widest thing this API
/// emits and 672 is not enough for it — the failure mode of getting that wrong
/// is `200 OK` with an empty body, which is why the bound is measured in
/// `somfy-api`'s own tests rather than counted here. See [`JsonBody`].
async fn get_settings(_from_this_device: FromThisDevice) -> impl IntoResponse {
    match RPC.call(Rpc::Settings).await {
        Some(Reply::Settings(wifi, mqtt)) => Ok((
            StatusCode::OK,
            JsonBody::<_, { somfy_api::SETTINGS_JSON_MAX_BYTES }>(SettingsDto {
                wifi,
                mqtt,
                wifi_trial: crate::trial::status(Instant::now().as_millis()),
            }),
        )),
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::SettingsUnwritable))),
        None => Err(Err(Unavailable)),
    }
}

/// Try a candidate Wi-Fi credential without storing it.
///
/// `202`, never `200` or `204`, and the distinction is the whole design: what
/// has happened when this returns is that a trial has been *accepted*, not that
/// a credential has been changed. The device is about to leave the network this
/// request arrived over, and whether the change sticks depends on somebody
/// reaching it on the other one and confirming — see `crate::trial`, and
/// `somfy_config::WifiTrial` for why that and not association is the test.
///
/// The candidate is validated **before** the radio is touched, so an SSID one
/// byte too long costs no connection at all.
async fn start_wifi_trial(
    _from_this_device: FromThisDevice,
    Json(update): Json<WifiUpdateDto>,
) -> impl IntoResponse {
    let candidate = match RPC.call(Rpc::PrepareWifi(update)).await {
        Some(Reply::WifiCandidate(candidate)) => candidate,
        Some(Reply::Refused(code)) => return Err(Ok(Refusal(code))),
        Some(_) => return Err(Ok(refuse(ApiErrorCode::SettingsUnwritable))),
        None => return Err(Err(Unavailable)),
    };
    match crate::trial::request(candidate) {
        Ok(()) => Ok((StatusCode::ACCEPTED, NoContent)),
        Err(code) => Err(Ok(refuse(code))),
    }
}

/// End a live trial, one way or the other.
///
/// # Confirm
///
/// The only path on which a Wi-Fi credential reaches flash, and the order is
/// what makes it safe: the trial is asked whether it has been proved **first**,
/// the write happens second, and the trial is forgotten only once the write has
/// been acknowledged. A trial cleared before the write landed would leave the
/// device running on a credential it would not come back to after a power cut.
///
/// Answers `204`, because the change is complete: the device is on the new
/// network and the credential is stored.
///
/// # Cancel
///
/// Answers `202`, because what happens next is a restart onto the stored
/// credential — see `crate::trial` for why a revert is a reboot — and this
/// response cannot outlive it by much.
async fn settle_wifi_trial(
    _from_this_device: FromThisDevice,
    Json(decision): Json<TrialDecisionDto>,
) -> impl IntoResponse {
    match decision {
        TrialDecisionDto::Confirm => {
            let candidate = match crate::trial::commit(Instant::now().as_millis()) {
                Ok(candidate) => candidate,
                Err(code) => return Err(Ok(refuse(code))),
            };
            match RPC.call(Rpc::SaveWifi(candidate)).await {
                Some(Reply::Done) => {
                    crate::trial::end();
                    Ok((StatusCode::NO_CONTENT, NoContent))
                }
                // The trial is deliberately **left running**: the credential is
                // proved and only the write failed, so the operator can retry
                // the confirmation rather than run the whole trial again. If
                // they do not, the confirmation deadline reverts the device as
                // it always would.
                Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
                Some(_) => Err(Ok(refuse(ApiErrorCode::SettingsUnwritable))),
                None => Err(Err(Unavailable)),
            }
        }
        TrialDecisionDto::Cancel => match crate::trial::cancel() {
            Ok(()) => Ok((StatusCode::ACCEPTED, NoContent)),
            Err(code) => Err(Ok(refuse(code))),
        },
    }
}

/// Store broker settings and restart onto them.
///
/// `202`, because the settings are stored and then the device restarts, and the
/// restart is not optional — see [`restart_for_mqtt`].
async fn save_mqtt(
    _from_this_device: FromThisDevice,
    Json(update): Json<MqttUpdateDto>,
) -> impl IntoResponse {
    apply_mqtt(Rpc::SaveMqtt(update)).await
}

/// Run without a broker, and restart.
///
/// A device with no broker still receives, decodes and tracks; it just publishes
/// nothing. That is a configuration an operator can mean, which is why it is a
/// `DELETE` on the resource rather than a `PUT` of something empty.
async fn clear_mqtt(_from_this_device: FromThisDevice) -> impl IntoResponse {
    apply_mqtt(Rpc::ClearMqtt).await
}

/// The half `save_mqtt` and `clear_mqtt` share.
async fn apply_mqtt(request: Rpc) -> Result<(StatusCode, NoContent), Result<Refusal, Unavailable>> {
    match RPC.call(request).await {
        Some(Reply::Done) => {
            restart_for_mqtt();
            Ok((StatusCode::ACCEPTED, NoContent))
        }
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::SettingsUnwritable))),
        None => Err(Err(Unavailable)),
    }
}

/// Restart, shortly, so the new broker settings take effect.
///
/// # Why a restart and not a reconfiguration in place
///
/// Not laziness — it is the only path on which R5 is already true. Changing
/// `state_root` or `discovery_prefix` requires the retained discovery configs
/// published under the **old** namespaces to be deleted before the new ones go
/// out, and the only record of those old values is the older records still
/// readable in the configuration ring. `crate::config::ConfigStore::load`
/// computes exactly that set at boot, `crate::mqtt::start` turns each pair into
/// a configuration whose retained topics are cleared first, and
/// `somfy_mqtt::reconfigure` is the only way to obtain the two halves together
/// and emits them in that order.
///
/// Reconfiguring the live session would mean recomputing the superseded set on a
/// second code path, in a task holding a broker connection, with the ordering
/// rule restated rather than reused. The boot path is already hardware-proven
/// and its retirement is idempotent, so this reaches it by the front door.
///
/// The cost is a few seconds of radio downtime on an operator-initiated action.
/// Rolling codes, the shade table and the announced set are all in flash and
/// survive it.
fn restart_for_mqtt() {
    crate::logln!(
        "config: broker settings stored — restarting so the retained topics of the \
         superseded namespaces are cleared before the new ones are published"
    );
    // Not immediate: the `202` has to leave the socket first, and this
    // connection is not going to be re-established. The Wi-Fi trial's settle
    // delay exists for the same reason and is the same figure.
    RESTART.signal(());
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Bytes of log copied out of the ring per chunk.
///
/// **64, which is [`crate::restore::EXPORT_CHUNK_BYTES`], and one figure rather
/// than two is the point.** Both are a scratch buffer held across a socket
/// write inside each connection task's future — four copies each, out of the
/// DRAM the Wi-Fi driver's heap is carved from — and two different sizes would
/// have been two slots the compiler has no reason to overlap.
///
/// Each chunk is one critical section over a `memcpy` in RTC RAM plus about
/// eight bytes of chunked-transfer framing on the wire, so the whole 4 KiB ring
/// is 64 chunks: half a kilobyte of framing and 64 critical sections, once, on a
/// page a person opens when something has already gone wrong.
///
/// It is deliberately *not* the ring's size. A single chunk would hold the
/// critical section for the length of a socket write, which is the one thing a
/// degradable service may never do — see [`crate::diag::log_read`].
const LOG_CHUNK_BYTES: usize = crate::restore::EXPORT_CHUNK_BYTES;

/// The log ring, streamed as text.
///
/// `text/plain` rather than JSON, and the reason is the buffer above: escaping
/// four kilobytes into a JSON string would need a buffer up to six times that,
/// held across the write, four times over. A `<pre>` is what the UI does with it
/// either way, and `curl` shows it without a parser.
///
/// Chunked because the length is not knowable in advance — the ring is being
/// appended to while it is read — and streaming hands the executor back between
/// chunks so a log fetch cannot sit between the state task and an arrival stop.
struct LogText;

impl Chunks for LogText {
    fn content_type(&self) -> &'static str {
        // `charset=utf-8` because a log line carries shade names, and a name is
        // whatever the operator typed. The ring evicts **whole lines**, so a
        // response can never begin part-way through a character.
        "text/plain; charset=utf-8"
    }

    async fn write_chunks<W: Write>(
        self,
        mut writer: ChunkWriter<W>,
    ) -> Result<ChunksWritten, W::Error> {
        let mut scratch = [0u8; LOG_CHUNK_BYTES];
        let mut at = 0usize;
        loop {
            let taken = crate::diag::log_read(at, &mut scratch);
            if taken == 0 {
                break;
            }
            writer.write_chunk(&scratch[..taken]).await?;
            at += taken;
        }
        writer.finalize().await
    }
}

/// Which part this image was built for.
#[cfg(feature = "chip-s3")]
const THIS_CHIP: ChipDto = ChipDto::Esp32S3;
/// See the `chip-s3` definition above.
#[cfg(feature = "chip-c3")]
const THIS_CHIP: ChipDto = ChipDto::Esp32C3;

/// Everything the diagnostics screen reads.
///
/// **Assembled here rather than in [`crate::diag`]**, which is where it started
/// and where it did not belong: that module is included by path into the
/// `tx-check` bring-up harness, which takes `logln!` and has neither a host name
/// nor a painted stack, so a `SystemDto` built there referred to three things
/// that do not exist. Composing the DTO is an API concern and this is the API.
///
/// Every term is a global [`crate::diag`] or [`crate::heap`] owns, and none is
/// behind [`crate::rpc`]: nothing here touches the store, the registry, the
/// transmit queue or the frame channel, so a request for it cannot make the
/// state task wait. That is the same separation rule `crate::api`'s module docs
/// state, kept by there being nothing shared rather than by a lock.
fn system() -> SystemDto {
    let mut firmware = heapless::String::new();
    // Cannot fail: `MAX_VERSION_LEN` is 16 and this crate's version is `0.1.0`.
    // Truncation would be silent, so it is checked at compile time instead.
    let _ = firmware.push_str(env!("CARGO_PKG_VERSION"));
    let mut host = heapless::String::new();
    // Cannot fail: `identity::hostname` is `somfy-` plus twelve hex digits,
    // which is `somfy_api::MAX_HOST_LEN` exactly.
    let _ = host.push_str(crate::identity::hostname().as_str());

    SystemDto {
        chip: THIS_CHIP,
        firmware,
        host,
        uptime_s: crate::diag::uptime_s(),
        reset_reason: crate::diag::reset_reason(),
        stack: StackDto {
            available: crate::stack_available() as u32,
            required: crate::heap::REQUIRED_STACK_BYTES as u32,
            // Zero is `crate::stack_used`'s "could not measure" — the paint
            // covers `PAINT_HEADROOM_BYTES` at minimum on any boot that
            // happened at all — so it becomes an absent field rather than a
            // number claiming the stack was untouched.
            used: match crate::stack_used() {
                0 => None,
                used => Some(used as u32),
            },
        },
        heap: HeapDto {
            size: crate::heap::size_bytes() as u32,
            used: crate::heap::used_bytes() as u32,
            peak: crate::heap::peak_bytes() as u32,
        },
        log: crate::diag::log_stats(),
        last_panic: crate::diag::last_panic(),
    }
}

const _: () = assert!(
    env!("CARGO_PKG_VERSION").len() <= somfy_api::MAX_VERSION_LEN,
    "this crate's version does not fit somfy_api::MAX_VERSION_LEN, and a truncated \
     version string on a diagnostics screen is worse than none",
);

/// `GET /api/v1/system` — what the device knows about itself.
///
/// **Not behind [`crate::rpc`]**, unlike every other read in this file, and the
/// difference is worth naming rather than looking like an oversight: every term
/// in the answer is a global that [`crate::diag`] or [`crate::heap`] owns, and
/// none of them is the registry, the store, the transmit queue or the frame
/// channel. There is nothing here for the state task to contend for, so a
/// request for it cannot make the state task wait — which is the property the
/// seam exists to provide, arrived at by there being nothing shared rather than
/// by a rendezvous.
async fn get_system(_from_this_device: FromThisDevice) -> impl IntoResponse {
    (
        StatusCode::OK,
        JsonBody::<_, { somfy_api::SYSTEM_JSON_MAX_BYTES }>(system()),
    )
}

/// `GET /api/v1/system/log` — the ring, oldest line first.
async fn get_log(_from_this_device: FromThisDevice) -> impl IntoResponse {
    ChunkedResponse::new(LogText)
}

/// `DELETE /api/v1/system` — forget the panic record and empty the log.
///
/// **One action rather than two, and the coupling is the point**: both are what
/// this device remembers about its own past, and an operator who has read a
/// panic and wants the screen to stop showing it wants the lines that produced
/// it gone too. Splitting them would also have cost a third route, which is not
/// free — see [`crate::heap::DRAM_FOR_STACK_AND_HEAP`] for what a route costs in
/// the connection task futures and which chip pays for it.
///
/// `204` and not `404` when there was nothing to forget: the request is
/// idempotent and its postcondition — this device remembers nothing from before
/// now — holds either way. A client that deletes twice has not made a mistake.
async fn forget_the_past(_from_this_device: FromThisDevice) -> impl IntoResponse {
    crate::diag::forget();
    (StatusCode::NO_CONTENT, NoContent)
}

// ---------------------------------------------------------------------------
// Backup and restore
// ---------------------------------------------------------------------------

/// The `Content-Disposition` an export answers with.
///
/// Fixed rather than built from the host name, and the reason is the one
/// [`location_of`] gives: `picoserve` takes headers as `&str` borrowed for the
/// response, so a `heapless::String` built in a handler would be gone by the
/// time the header was written. A table of every possible name is not an option
/// here — there are 2^48 host names — so the name is constant and the device it
/// came from is inside the file.
///
/// The extension is the container magic, lower-cased. It is deliberately not a
/// name a firmware image could have: uploading one file to the other's route is
/// the mistake both routes spend a refusal code on, and a file picker showing
/// `.rtsb` next to `.bin` is the cheapest place to prevent it.
const BACKUP_FILENAME: &str = "attachment; filename=\"somfy-rs.rtsb\"";

/// `GET /api/v1/system/backup` — this device's configuration, as a file.
///
/// Chunked, and streamed out of flash sixty-four bytes at a time through
/// [`crate::rpc`]. Nothing four kilobytes long exists anywhere on this path:
/// `crate::restore::export_chunk` is where each chunk is assembled, and
/// `somfy_backup` is what is in the file and what is deliberately not.
struct BackupFile;

impl Chunks for BackupFile {
    fn content_type(&self) -> &'static str {
        // Not JSON and not text: it is two flash records, a code block and a
        // checksum, and a browser that tried to display it would show mojibake.
        // `octet-stream` plus the disposition header is what makes it save.
        "application/octet-stream"
    }

    async fn write_chunks<W: Write>(
        self,
        mut writer: ChunkWriter<W>,
    ) -> Result<ChunksWritten, W::Error> {
        let mut at = 0u32;
        loop {
            match RPC.call(Rpc::BackupChunk { at }).await {
                Some(Reply::BackupChunk { len: 0, .. }) => break,
                Some(Reply::BackupChunk { len, bytes }) => {
                    let len = usize::from(len).min(bytes.len());
                    writer.write_chunk(&bytes[..len]).await?;
                    at += len as u32;
                }
                // A refusal or a state task that did not answer ends the body
                // here. **A truncated backup is not a valid one** — the
                // container's length field and its checksum both cover it — so
                // what the client gets is a file `somfy_backup::decode` refuses,
                // which is honest and better than a response that never
                // terminates.
                _ => {
                    crate::logln!(
                        "backup: the export stopped after {} bytes — the file will not check out",
                        at,
                    );
                    break;
                }
            }
        }
        writer.finalize().await
    }
}

async fn get_backup(_from_this_device: FromThisDevice) -> impl IntoResponse {
    // `into_response().with_headers(..)` rather than the `(status, header,
    // content)` tuple every other handler here returns: those tuple impls
    // require the body to be `Content`, which a chunked body is not — it is a
    // `Body` that writes itself. This is `picoserve`'s own way of adding a
    // header to one.
    ChunkedResponse::new(BackupFile)
        .into_response()
        .with_headers([("Content-Disposition", BACKUP_FILENAME)])
}

/// `GET /api/v1/system/restore` — what the last upload did.
///
/// **Not behind [`crate::rpc`]**, for the reason `crate::diag::system` gives:
/// the report is a value the state task settles once at boot and once per
/// upload, and a poll for it must not be able to make that task wait. See
/// `crate::restore::report`.
async fn get_restore(_from_this_device: FromThisDevice) -> impl IntoResponse {
    (
        StatusCode::OK,
        JsonBody::<_, { somfy_api::RESTORE_JSON_MAX_BYTES }>(crate::restore::report()),
    )
}

/// `POST /api/v1/system/backup` — a backup, staged for the next boot.
///
/// A `RequestHandlerService` for the reason [`OtaUpload`] is: the body is a
/// file, and every `picoserve` body extractor produces the body as a *value*
/// that has to fit the 1,536-byte request buffer. So it keeps by hand the two
/// things a handler function gets for free, in this order:
///
/// 1. **The `Origin`/`Host` check**, as its first statement, before the session
///    lock and before a byte of the body is read.
/// 2. **The session lock**, which is [`crate::ota::take`] — *the same lock a
///    firmware upload takes*. Not reuse for its own sake: both stream a file to
///    flash through one page buffer, so two at once would interleave their
///    pages, and one lock makes that inexpressible rather than checked. A
///    second upload of either kind is refused with
///    [`ApiErrorCode::UpdateInProgress`].
///
/// # What a failure leaves behind
///
/// Nothing that will be applied. The staged bytes are only reachable through a
/// state record that says `Staged`, and that record is the last thing written —
/// so a partial upload, a refused file and a dropped socket all leave a region
/// full of bytes nothing will ever read.
struct RestoreUpload;

impl picoserve::routing::RequestHandlerService<(), ()> for RestoreUpload {
    async fn call_request_handler_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        state: &(),
        (): (),
        mut request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        use picoserve::extract::FromRequestParts;

        // One exit, for the reason [`OtaUpload`] records: written as four
        // `finalize().write_to()` pairs, that handler's `select` frame grew by
        // 14,672 bytes, because each pair is inlined into the poll with its own
        // response writer beneath it.
        let answer: Result<(StatusCode, NoContent), Result<Refusal, Unavailable>> = 'answer: {
            if let Err(refusal) = FromThisDevice::from_request_parts(state, &request.parts).await {
                break 'answer Err(Ok(refusal));
            }

            let declared = request.body_connection.content_length();
            let Some(mut upload) = crate::ota::take() else {
                break 'answer Err(Ok(refuse(ApiErrorCode::UpdateInProgress)));
            };

            let outcome = receive_backup(&mut request, &mut upload, declared).await?;
            // Dropped before the response is written, so a client that
            // immediately retries a refused upload is not told the device is
            // busy with itself.
            drop(upload);

            match outcome {
                Ok(()) => {
                    crate::logln!(
                        "restore: staged — restarting to apply it. Nothing has been validated \
                         yet; the next boot reads it, and GET /api/v1/system/restore is where it \
                         says what it did."
                    );
                    RESTART.signal(());
                    Ok((StatusCode::ACCEPTED, NoContent))
                }
                Err(Some(code)) => Err(Ok(Refusal(code))),
                Err(None) => Err(Err(Unavailable)),
            }
        };

        answer
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

/// Stream the body to the state task, one page at a time.
///
/// The same shape as [`receive`], which is the firmware-image half, sharing its
/// page channel and its read timeout. Two differences: four different requests,
/// and one rule it does not need — a backup is at most sixteen kilobytes into an
/// already-erased region, so there is no sector-alignment rule for a short page
/// in the middle to break, and `crate::restore::Staging::page` rounds the last
/// page up rather than refusing it.
async fn receive_backup<R: Read>(
    request: &mut picoserve::request::Request<'_, R>,
    upload: &mut crate::ota::Upload,
    declared: usize,
) -> Result<Result<(), Option<ApiErrorDto>>, R::Error> {
    let began = match RPC
        .call(Rpc::RestoreBegin {
            declared: declared as u32,
        })
        .await
    {
        Some(Reply::Done) => Ok(()),
        Some(Reply::Refused(code)) => Err(Some(code)),
        _ => Err(None),
    };
    if let Err(refusal) = began {
        return Ok(Err(refusal));
    }

    let mut remaining = declared;
    let outcome = {
        let mut reader = request
            .body_connection
            .body()
            .reader()
            .with_different_timeout(embassy_time::Duration::from_secs(UPLOAD_READ_TIMEOUT_S));
        loop {
            if remaining == 0 {
                break Ok(());
            }
            let want = remaining.min(crate::restore::PAGE_BYTES);
            let Some(page) = upload.lend().await else {
                break Err(None);
            };
            match reader.read_exact(&mut page.bytes[..want]).await {
                Ok(()) => {}
                Err(picoserve::io::ReadExactError::UnexpectedEof) => {
                    // The client stopped sending. Nothing was marked staged, so
                    // this is a failed upload rather than a damaged device.
                    break Err(Some(ApiErrorDto::from(ApiErrorCode::BackupDamaged)));
                }
                Err(picoserve::io::ReadExactError::Other(error)) => return Err(error),
            }
            upload.post();
            match RPC.call(Rpc::RestorePage { len: want as u16 }).await {
                Some(Reply::Done) => {}
                Some(Reply::Refused(code)) => break Err(Some(code)),
                _ => break Err(None),
            }
            remaining -= want;
        }
    };

    match outcome {
        Ok(()) => Ok(match RPC.call(Rpc::RestoreFinish).await {
            Some(Reply::Done) => Ok(()),
            Some(Reply::Refused(code)) => Err(Some(code)),
            _ => Err(None),
        }),
        Err(refusal) => {
            // "Abort what you started", unconditionally, for the reason the
            // firmware upload gives: it survives a later change to which side
            // refuses.
            let _ = RPC.call(Rpc::RestoreAbort).await;
            Ok(Err(refusal))
        }
    }
}

// ---------------------------------------------------------------------------
// Firmware upload
// ---------------------------------------------------------------------------

/// How long the whole body may take to arrive once the first byte has.
///
/// **Five minutes, and it is a stall detector rather than a performance
/// budget.** `picoserve`'s ordinary `read_request` timeout is three seconds,
/// which is right for a request that fits in a segment and wrong for one that
/// interleaves a megabyte of socket reads with a few hundred flash sector
/// erases — each of which runs with interrupts disabled for tens of
/// milliseconds, with a datasheet worst case in the hundreds. A full 2,031,616
/// byte slot is 496 sectors, so the flash side alone can plausibly account for
/// minutes on a part at the slow end of its specification.
///
/// What it bounds is the thing that actually needs bounding: a client that
/// opens an upload and then stops sending would otherwise hold a connection
/// task for the life of the boot. [`crate::api::REST_TASKS_RESERVED`] keeps two
/// tasks free for REST even while one is stuck here, so the cost of a generous
/// figure is bounded by construction rather than by the figure.
const UPLOAD_READ_TIMEOUT_S: u64 = 300;

/// `POST /api/v1/ota` — a firmware image, streamed to the inactive slot.
///
/// # Why this is a service and every other route is a function
///
/// A handler function receives its body through a `picoserve::extract::FromRequest`
/// extractor, and every extractor `picoserve` has produces the body *as a
/// value* — a slice, a string, a `Json<T>`. All of them need the whole body in
/// the request buffer, which is 1,536 bytes. A firmware image is three orders
/// of magnitude larger than that.
///
/// A `picoserve::routing::RequestHandlerService` is handed the raw
/// `picoserve::request::Request` instead, which is what makes a streaming read
/// possible at all. The cost is that the two things a handler function gets for
/// free have to be done by hand, and both are done first, in this order:
///
/// 1. **The `Origin`/`Host` check.** [`FromThisDevice`] is a
///    `FromRequestParts`, so it is *callable* here even though nothing calls it
///    for us. This route is the most consequential one on the device — it
///    replaces the firmware — so it is worth saying plainly that it sits behind
///    exactly the same check as every other `/api/v1` route, and that the check
///    happens before a byte of the body is read.
/// 2. **The session lock.** [`crate::ota::take`] is the lock *and* the right to
///    send pages, so a second concurrent upload is refused rather than
///    interleaved with the first.
///
/// # What a failure leaves behind
///
/// Nothing that can run. Every path out of here that is not the happy one
/// leaves `otadata` naming the slot this image is executing from, so a partial
/// upload, a refused image and a dropped socket are all the same thing from the
/// bootloader's point of view: a slot it is not asked to boot. The half-written
/// bytes sit there until the next upload erases them.
struct OtaUpload;

impl picoserve::routing::RequestHandlerService<(), ()> for OtaUpload {
    async fn call_request_handler_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        state: &(),
        (): (),
        mut request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        use picoserve::extract::FromRequestParts;

        // **One exit, and it is measured rather than tidy.** Written first with
        // four `finalize().write_to()` pairs — one per outcome, which is what a
        // service handler invites — the connection's `select` frame grew by
        // **14,672 bytes**, because each pair is inlined into the poll with its
        // own response writer beneath it and the compiler does not overlap
        // them. Collapsing them into one `Result<_, Result<Refusal, Unavailable>>`,
        // which is the shape every other handler in this file already returns,
        // took most of that back. See `crate::heap::REQUEST_CHAIN_BYTES`.
        let answer: Result<(StatusCode, NoContent), Result<Refusal, Unavailable>> = 'answer: {
            // Point 1 above. Before the body, before the lock, before anything.
            if let Err(refusal) = FromThisDevice::from_request_parts(state, &request.parts).await {
                break 'answer Err(Ok(refusal));
            }

            let declared = request.body_connection.content_length();
            let Some(mut upload) = crate::ota::take() else {
                break 'answer Err(Ok(refuse(ApiErrorCode::UpdateInProgress)));
            };

            let outcome = receive(&mut request, &mut upload, declared).await?;
            // Dropped explicitly rather than at the end of the block, so the
            // session lock is released before the response is written and a
            // client that immediately retries a refused upload is not told the
            // device is busy with itself.
            drop(upload);

            match outcome {
                Ok(()) => {
                    crate::logln!(
                        "ota: update accepted — restarting into it. If it does not confirm \
                         itself, this board comes back to the image it is running now."
                    );
                    RESTART.signal(());
                    Ok((StatusCode::ACCEPTED, NoContent))
                }
                Err(Some(code)) => Err(Ok(Refusal(code))),
                Err(None) => Err(Err(Unavailable)),
            }
        };

        answer
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

/// Stream the body to the state task, one page at a time.
///
/// The outer `Result` is the socket: an I/O error is not something to answer,
/// it is a connection that has gone. The inner one is the device's answer —
/// `Some(code)` is a refusal the client can act on, `None` is the state task
/// not answering, which becomes a `503` for the reason [`Unavailable`] gives.
///
/// **Every page is filled completely except the last**, and that is not
/// housekeeping: [`crate::ota`] writes through `NorFlash`, whose lengths must be
/// word-aligned, and it decides where a flash sector begins from the running
/// total. A short page in the middle would put every later page off its sector
/// boundary and leave part of a sector unerased under a write.
async fn receive<R: Read>(
    request: &mut picoserve::request::Request<'_, R>,
    upload: &mut crate::ota::Upload,
    declared: usize,
) -> Result<Result<(), Option<ApiErrorDto>>, R::Error> {
    let began = match RPC
        .call(Rpc::OtaBegin {
            declared: declared as u32,
        })
        .await
    {
        Some(Reply::Done) => Ok(()),
        Some(Reply::Refused(code)) => Err(Some(code)),
        _ => Err(None),
    };
    if let Err(refusal) = began {
        return Ok(Err(refusal));
    }

    let mut remaining = declared;
    // Scoped so the reader — which borrows the body connection — is dropped
    // before the caller finalizes that connection into a response.
    let outcome = {
        let mut reader = request
            .body_connection
            .body()
            .reader()
            .with_different_timeout(embassy_time::Duration::from_secs(UPLOAD_READ_TIMEOUT_S));
        loop {
            if remaining == 0 {
                break Ok(());
            }
            let want = remaining.min(crate::ota::PAGE_BYTES);
            // `None` is unreachable — the session lock and the sender are the
            // same object — and it answers `503` rather than panicking, which
            // is what every other unreachable arm on this path does.
            let Some(page) = upload.lend().await else {
                break Err(None);
            };
            match reader.read_exact(&mut page.bytes[..want]).await {
                Ok(()) => {}
                Err(picoserve::io::ReadExactError::UnexpectedEof) => {
                    // The client stopped sending. Nothing was marked bootable,
                    // so this is a failed update rather than a damaged device —
                    // which is what the code says.
                    break Err(Some(ApiErrorDto::from(ApiErrorCode::ImageDamaged)));
                }
                Err(picoserve::io::ReadExactError::Other(error)) => return Err(error),
            }
            upload.post();
            match RPC.call(Rpc::OtaPage { len: want as u16 }).await {
                Some(Reply::Done) => {}
                Some(Reply::Refused(code)) => break Err(Some(code)),
                _ => break Err(None),
            }
            remaining -= want;
        }
    };

    match outcome {
        Ok(()) => Ok(match RPC.call(Rpc::OtaFinish).await {
            Some(Reply::Done) => Ok(()),
            Some(Reply::Refused(code)) => Err(Some(code)),
            _ => Err(None),
        }),
        Err(refusal) => {
            // The state task drops its half of the session on a refusal it
            // raised itself, so this is only strictly needed when the *client*
            // stopped — and it is unconditional anyway, because "abort what you
            // started" is a rule that survives a later change to which side
            // refuses.
            let _ = RPC.call(Rpc::OtaAbort).await;
            Ok(Err(refusal))
        }
    }
}

/// Raised when a settings change needs a restart, and awaited by [`restarter`].
///
/// A signal and a task rather than a call to `software_reset` inside the
/// handler, because the handler has not written its response yet: resetting
/// there would answer the operator's save with a dropped connection, which is
/// indistinguishable from the save having failed.
///
/// The firmware upload above raises it for the same reason and gets the same
/// settle delay: an update that takes effect on the next boot has to *have* a
/// next boot, and an operator whose `curl` returned before the reset is an
/// operator who knows the upload landed.
static RESTART: Signal<crate::tasks::Mutex, ()> = Signal::new();

/// Restart once a settings change has asked for one and its response has left.
#[embassy_executor::task]
pub async fn restarter() -> ! {
    RESTART.wait().await;
    embassy_time::Timer::after(crate::trial::settle()).await;
    esp_hal::system::software_reset();
}

// ---------------------------------------------------------------------------
// The app shell, and the 404 that is not it
// ---------------------------------------------------------------------------

/// The `Location` a freshly created shade lives at.
///
/// A table rather than formatting, because the alternative needs a buffer that
/// outlives the call: `picoserve` takes headers as `&str` borrowed for the
/// response, and a `heapless::String` built here would be gone by then. Every
/// possible answer is known at compile time — `somfy_domain::MAX_SHADES` ids
/// against one fixed path — so the table is the whole set.
///
/// Out of range is unreachable: a shade's id is its registry slot, and there
/// are `MAX_SHADES` of those. It answers with the collection rather than
/// panicking, because a panic here reboots the board over a header.
fn location_of(id: ShadeId) -> &'static str {
    LOCATIONS.get(id.0 as usize).copied().unwrap_or(SHADES_PATH)
}

/// The collection itself, which is also the `Location` fallback above.
const SHADES_PATH: &str = "/api/v1/shades";

/// Every `Location` this device can answer with, indexed by shade id.
///
/// Length-checked against the registry's own bound by the assertion below, so
/// growing `MAX_SHADES` without extending this list is a build failure rather
/// than a `Location` pointing at the collection.
const LOCATIONS: [&str; 32] = [
    "/api/v1/shades/0",
    "/api/v1/shades/1",
    "/api/v1/shades/2",
    "/api/v1/shades/3",
    "/api/v1/shades/4",
    "/api/v1/shades/5",
    "/api/v1/shades/6",
    "/api/v1/shades/7",
    "/api/v1/shades/8",
    "/api/v1/shades/9",
    "/api/v1/shades/10",
    "/api/v1/shades/11",
    "/api/v1/shades/12",
    "/api/v1/shades/13",
    "/api/v1/shades/14",
    "/api/v1/shades/15",
    "/api/v1/shades/16",
    "/api/v1/shades/17",
    "/api/v1/shades/18",
    "/api/v1/shades/19",
    "/api/v1/shades/20",
    "/api/v1/shades/21",
    "/api/v1/shades/22",
    "/api/v1/shades/23",
    "/api/v1/shades/24",
    "/api/v1/shades/25",
    "/api/v1/shades/26",
    "/api/v1/shades/27",
    "/api/v1/shades/28",
    "/api/v1/shades/29",
    "/api/v1/shades/30",
    "/api/v1/shades/31",
];

// One `Location` per registry slot. If the registry grows, this list has to,
// and a build failure is how that gets noticed rather than a `201` pointing at
// the collection.
const _: () = assert!(
    LOCATIONS.len() == somfy_domain::MAX_SHADES,
    "LOCATIONS must hold one path per registry slot",
);
