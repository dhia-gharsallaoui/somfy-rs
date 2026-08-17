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

use picoserve::extract::Json;
use picoserve::io::{Read, Write};
use picoserve::response::chunked::{ChunkWriter, ChunkedResponse, Chunks, ChunksWritten};
use picoserve::response::{ws, Content, IntoResponse, NoContent, ResponseWriter, StatusCode};
use picoserve::routing::{get, parse_path_segment, post, PathRouter};
use picoserve::{ResponseSent, Router};
use serde::Serialize;
use somfy_api::{ApiErrorCode, ApiErrorDto, CommandDto, CreateShadeDto, PatchShadeDto};
use somfy_domain::{GroupId, ShadeId};
use somfy_tasks::ControlCommand;

use crate::api::events::Events;
use crate::api::shell;
use crate::edits::ShadeEdit;
use crate::rpc::{Reply, Request as Rpc, RPC};

/// Longest JSON any one entity serialises to, in bytes.
///
/// Derived rather than rounded, from the widest [`ShadeDto`]:
///
/// - **~210 bytes of structure** — seventeen camelCase field names, their
///   quotes, colons and commas, and the enclosing braces.
/// - **192 bytes of name.** The field is a `heapless::String<32>`, and
///   `serde-json-core` escapes a control character as `\u00XX`, six bytes for
///   one. Nothing forbids a name of thirty-two of them, so the bound has to
///   assume it even though no real name comes close.
/// - **~70 bytes of values** — a 24-bit address, three `u32` travel times at up
///   to ten digits each, four percentages, and the longest of the enum
///   spellings (`"operatorSupplied"` at eighteen bytes, three times over).
///
/// 472 rounded to 512. [`GroupDto`] and [`RoomDto`] are smaller: the same name
/// bound plus at most thirty-two two-digit ids.
const ENTITY_JSON_BYTES: usize = 512;

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
            ("/api/v1/shades", parse_path_segment::<u8>(), "/command"),
            post(command_shade),
        )
        .route("/api/v1/groups", get(list_groups))
        .route(
            ("/api/v1/groups", parse_path_segment::<u8>(), "/command"),
            post(command_group),
        )
        .route("/api/v1/rooms", get(list_rooms))
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
        let mut scratch = [0u8; ENTITY_JSON_BYTES];
        serde_json_core::to_slice(&self.0, &mut scratch).unwrap_or(0)
    }

    async fn write_content<W: Write>(self, mut writer: W) -> Result<(), W::Error> {
        let mut scratch = [0u8; ENTITY_JSON_BYTES];
        let written = serde_json_core::to_slice(&self.0, &mut scratch).unwrap_or(0);
        writer.write_all(&scratch[..written]).await
    }
}

/// A refusal: the status the code carries, and the code as the body.
///
/// The status is not chosen here — [`ApiErrorCode::http_status`] decides it,
/// beside the variant it describes, so that a code added in `somfy-api` cannot
/// reach this router without somebody having said what it means over HTTP.
struct Refusal(ApiErrorCode);

impl IntoResponse for Refusal {
    async fn write_to<R: Read, W: ResponseWriter<Error = R::Error>>(
        self,
        connection: picoserve::response::Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        (
            StatusCode::new(self.0.http_status()),
            JsonBody(ApiErrorDto::from(self.0)),
        )
            .write_to(connection, response_writer)
            .await
    }
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

            let (written, next) = match self {
                Collection::Shades => match RPC.call(Rpc::ShadeFrom(slot)).await {
                    Some(Reply::Shade(Some(shade))) => (
                        serde_json_core::to_slice(&shade, &mut scratch[start..]).unwrap_or(0),
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
                        serde_json_core::to_slice(&group, &mut scratch[start..]).unwrap_or(0),
                        group.id.checked_add(1),
                    ),
                    _ => break,
                },
                Collection::Rooms => match RPC.call(Rpc::RoomFrom(slot)).await {
                    Some(Reply::Room(Some(room))) => (
                        serde_json_core::to_slice(&room, &mut scratch[start..]).unwrap_or(0),
                        room.id.checked_add(1),
                    ),
                    _ => break,
                },
            };

            writer.write_chunk(&scratch[..start + written]).await?;
            first = false;
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
        Some(Reply::Shade(None)) => Err(Ok(Refusal(ApiErrorCode::NotFound))),
        Some(_) => Err(Ok(Refusal(ApiErrorCode::NotFound))),
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
            _ => Err(Ok(Refusal(ApiErrorCode::NotFound))),
        },
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(Refusal(ApiErrorCode::InvalidAddress))),
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
            _ => Err(Ok(Refusal(ApiErrorCode::NotFound))),
        },
        Some(Reply::Refused(code)) => Err(Ok(Refusal(code))),
        Some(_) => Err(Ok(Refusal(ApiErrorCode::InvalidAddress))),
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
        Some(_) => Err(Ok(Refusal(ApiErrorCode::InvalidAddress))),
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
        Some(_) => Err(Ok(Refusal(ApiErrorCode::InvalidAddress))),
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
        Some(_) => Err(Ok(Refusal(ApiErrorCode::InvalidAddress))),
        None => Err(Err(Unavailable)),
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Upgrade to a WebSocket, if there is a slot.
///
/// The subscription **is** the slot — see [`crate::api::events`] — so this
/// cannot admit a client it has no capacity to serve, and cannot leak capacity
/// when one leaves.
async fn events(upgrade: ws::WebSocketUpgrade) -> impl IntoResponse {
    match crate::DELTAS.subscriber() {
        Ok(deltas) => Ok(upgrade.on_upgrade(Events::admit(deltas))),
        Err(_) => {
            esp_println::println!(
                "api: refusing a websocket — all {} slots are in use. REST is unaffected.",
                crate::api::events::WS_MAX,
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
