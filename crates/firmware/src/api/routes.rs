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
    ApiErrorCode, ApiErrorDto, CalibrationStepDto, CommandDto, CreateShadeDto, MqttUpdateDto,
    PatchShadeDto, SettingsDto, WifiUpdateDto,
};
use somfy_domain::{GroupId, ShadeId};
use somfy_tasks::ControlCommand;

use crate::api::events::Events;
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
        .route("/api/v1/settings/wifi/confirm", post(confirm_wifi_trial))
        .route("/api/v1/settings/wifi/cancel", post(cancel_wifi_trial))
        .route(
            "/api/v1/settings/mqtt",
            picoserve::routing::put(save_mqtt).delete(clear_mqtt),
        )
        .route("/api/v1/events", get(events))
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// A JSON body at a status of the caller's choosing.
///
/// `picoserve`'s own `Json` response hardcodes `200`, and the type behind it is
/// private, so `201 Created` and `200 OK` after a `PATCH` need this. It is the
/// entire cost of that gap.
struct JsonBody<T: Serialize>(T);

impl<T: Serialize> Content for JsonBody<T> {
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
        let mut scratch = [0u8; ENTITY_JSON_BYTES];
        serde_json_core::to_slice(&self.0, &mut scratch).unwrap_or(0)
    }

    async fn write_content<W: Write>(self, mut writer: W) -> Result<(), W::Error> {
        let mut scratch = [0u8; ENTITY_JSON_BYTES];
        let written = match serde_json_core::to_slice(&self.0, &mut scratch) {
            Ok(written) => written,
            Err(_) => {
                esp_println::println!(
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
struct Refusal(ApiErrorDto);

impl IntoResponse for Refusal {
    async fn write_to<R: Read, W: ResponseWriter<Error = R::Error>>(
        self,
        connection: picoserve::response::Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        (
            StatusCode::new(self.0.code.http_status()),
            JsonBody(self.0),
        )
            .write_to(connection, response_writer)
            .await
    }
}

/// A refusal from a bare code, which is what every non-settings path has.
fn refuse(code: ApiErrorCode) -> Refusal {
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
                None => esp_println::println!(
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

async fn list_shades() -> impl IntoResponse {
    ChunkedResponse::new(Collection::Shades)
}

async fn list_groups() -> impl IntoResponse {
    ChunkedResponse::new(Collection::Groups)
}

async fn list_rooms() -> impl IntoResponse {
    ChunkedResponse::new(Collection::Rooms)
}

// ---------------------------------------------------------------------------
// One shade
// ---------------------------------------------------------------------------

async fn get_shade(id: u8) -> impl IntoResponse {
    match RPC.call(Rpc::Shade(ShadeId(id))).await {
        Some(Reply::Shade(Some(shade))) => Ok((StatusCode::OK, JsonBody(shade))),
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
async fn create_shade(Json(request): Json<CreateShadeDto>) -> impl IntoResponse {
    match RPC.call(Rpc::Edit(ShadeEdit::Add { request })).await {
        Some(Reply::Created(id)) => match RPC.call(Rpc::Shade(id)).await {
            Some(Reply::Shade(Some(shade))) => Ok((
                StatusCode::CREATED,
                ("Location", location_of(id)),
                JsonBody(shade),
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
async fn patch_shade(id: u8, Json(patch): Json<PatchShadeDto>) -> impl IntoResponse {
    let id = ShadeId(id);
    match RPC
        .call(Rpc::Edit(ShadeEdit::Reconfigure { id, patch }))
        .await
    {
        Some(Reply::Done) => match RPC.call(Rpc::Shade(id)).await {
            Some(Reply::Shade(Some(shade))) => Ok((StatusCode::OK, JsonBody(shade))),
            _ => Err(Ok(refuse(ApiErrorCode::NotFound))),
        },
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

async fn delete_shade(id: u8) -> impl IntoResponse {
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
async fn pair_shade(id: u8) -> impl IntoResponse {
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
async fn confirm_pairing(id: u8) -> impl IntoResponse {
    let id = ShadeId(id);
    match RPC.call(Rpc::Edit(ShadeEdit::ConfirmPairing { id })).await {
        Some(Reply::Done) => match RPC.call(Rpc::Shade(id)).await {
            Some(Reply::Shade(Some(shade))) => Ok((StatusCode::OK, JsonBody(shade))),
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

async fn command_shade(id: u8, Json(command): Json<CommandDto>) -> impl IntoResponse {
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
async fn command_group(id: u8, Json(command): Json<CommandDto>) -> impl IntoResponse {
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
async fn calibrate_shade(id: u8, Json(step): Json<CalibrationStepDto>) -> impl IntoResponse {
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
async fn events(upgrade: ws::WebSocketUpgrade) -> impl IntoResponse {
    match Events::admit() {
        Some(events) => Ok(upgrade.on_upgrade(events)),
        None => {
            esp_println::println!(
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
async fn get_settings() -> impl IntoResponse {
    match RPC.call(Rpc::Settings).await {
        Some(Reply::Settings(wifi, mqtt)) => Ok((
            StatusCode::OK,
            JsonBody(SettingsDto {
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
async fn start_wifi_trial(Json(update): Json<WifiUpdateDto>) -> impl IntoResponse {
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


/// The operator reached the device on the candidate network. Store it.
///
/// This is the only path on which a Wi-Fi credential reaches flash, and the
/// order is what makes it safe: the trial is asked whether it has been proved
/// **first**, the write happens second, and the trial is forgotten only once the
/// write has been acknowledged. A trial cleared before the write landed would
/// leave the device running on a credential it would not come back to after a
/// power cut.
async fn confirm_wifi_trial() -> impl IntoResponse {
    let candidate = match crate::trial::commit(Instant::now().as_millis()) {
        Ok(candidate) => candidate,
        Err(code) => return Err(Ok(refuse(code))),
    };
    match RPC.call(Rpc::SaveWifi(candidate)).await {
        Some(Reply::Done) => {
            crate::trial::end();
            Ok((StatusCode::NO_CONTENT, NoContent))
        }
        // The trial is deliberately **left running**: the credential is proved
        // and only the write failed, so the operator can retry the confirmation
        // rather than having to run the whole trial again. If they do not, the
        // confirmation deadline reverts the device as it always would.
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(refuse(ApiErrorCode::SettingsUnwritable))),
        None => Err(Err(Unavailable)),
    }
}

/// Put the previous credential back now rather than waiting out the deadline.
///
/// `202`: what happens next is a restart onto the stored credential, which this
/// response cannot outlive by much. See `crate::trial` for why a revert is a
/// reboot.
async fn cancel_wifi_trial() -> impl IntoResponse {
    match crate::trial::cancel() {
        Ok(()) => Ok((StatusCode::ACCEPTED, NoContent)),
        Err(code) => Err(Ok::<Refusal, Unavailable>(refuse(code))),
    }
}

/// Store broker settings and restart onto them.
///
/// `202`, because the settings are stored and then the device restarts, and the
/// restart is not optional — see [`restart_for_mqtt`].
async fn save_mqtt(Json(update): Json<MqttUpdateDto>) -> impl IntoResponse {
    apply_mqtt(Rpc::SaveMqtt(update)).await
}

/// Run without a broker, and restart.
///
/// A device with no broker still receives, decodes and tracks; it just publishes
/// nothing. That is a configuration an operator can mean, which is why it is a
/// `DELETE` on the resource rather than a `PUT` of something empty.
async fn clear_mqtt() -> impl IntoResponse {
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
    esp_println::println!(
        "config: broker settings stored — restarting so the retained topics of the \
         superseded namespaces are cleared before the new ones are published"
    );
    // Not immediate: the `202` has to leave the socket first, and this
    // connection is not going to be re-established. The Wi-Fi trial's settle
    // delay exists for the same reason and is the same figure.
    RESTART.signal(());
}

/// Raised when a settings change needs a restart, and awaited by [`restarter`].
///
/// A signal and a task rather than a call to `software_reset` inside the
/// handler, because the handler has not written its response yet: resetting
/// there would answer the operator's save with a dropped connection, which is
/// indistinguishable from the save having failed.
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
