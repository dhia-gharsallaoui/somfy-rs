//! The web server: the UI from flash, the REST API the UI already calls, and
//! the WebSocket its live positions arrive on.
//!
//! # It is a degradable service, like the broker
//!
//! Spec §9 and R9 put it plainly: a network service that is absent, broken or
//! misbehaving must not affect radio control. Everything here is arranged
//! around that, and the arrangement is structural rather than careful:
//!
//! 1. **No shared state.** Nothing in this module touches the store, the
//!    transmit queue, the frame channel or the registry. The only thing that
//!    crosses is [`crate::rpc`], which is a request and an answer — the state task
//!    never waits on anything a client controls, because `Signal::signal`
//!    overwrites rather than parks.
//! 2. **The radio tasks are already running.** [`start`] is called from
//!    `main::start_network`, after the `yield_now` that makes the radio's first
//!    poll a fact, and its failure is printed and ignored. There is no path on
//!    which a web server that will not start stops the controller.
//! 3. **Concurrency is fixed at compile time.** [`HTTP_TASKS`] connections, each
//!    with its own buffers, allocated as statics. There is no growth under load
//!    and nothing to exhaust that was not already spent at link time.
//! 4. **A client cannot hold capacity.** Connections are closed after each
//!    response (see [`CONFIG`]), and WebSockets are capped below the pool so
//!    they cannot consume what REST needs.
//!
//! # The lockout this is designed against
//!
//! A device of this kind capped WebSockets at five, one per open browser tab,
//! and a polling integration with a stale address exhausted the cap on its own
//! — leaving the operator unable to reach the network settings that would have
//! fixed it. [`events`] carries the detail and what is different here.
//! [`REST_TASKS_RESERVED`] is the part that is checked by the compiler.
//!
//! # What it costs, measured
//!
//! Static RAM is not free here in the ordinary sense: [`crate::heap`] sizes the
//! Wi-Fi driver's heap by *subtracting* the stack budget from whatever DRAM the
//! chip has left after its statics, so every byte declared below comes out of
//! the heap on the tightest chip. The buffer sizes are therefore argued rather
//! than rounded up, and `docs/provenance.md` records what the three chips
//! measured afterwards.

use embassy_executor::{SpawnError, Spawner};
use embassy_net::Stack;
use picoserve::{Config, Router, Timeouts};
use static_cell::StaticCell;

use crate::api::events::WS_MAX;

// The UI and its absence are two implementations of one function, chosen here
// and nowhere else. `assets` carries the embedded app, its deep-link fallback
// and its two asset routes; `headless` carries none of them and answers `404`.
// `routes` calls `shell::base()` either way, so no API route is conditional.
#[cfg(feature = "ui")]
mod assets;
#[cfg(feature = "ui")]
use assets as shell;
#[cfg(not(feature = "ui"))]
mod headless;
#[cfg(not(feature = "ui"))]
use headless as shell;

pub mod events;
pub mod routes;

/// Connections this device serves at once.
///
/// Four, and the number is a division rather than a preference: [`WS_MAX`] of
/// them may be held open by WebSockets, and what is left has to be enough for
/// the REST traffic a page load makes. The UI opens its dashboard with three
/// parallel `GET`s (`loadSnapshot`), so two spare tasks serve that in two
/// rounds with no queueing a person could perceive.
///
/// Each one costs [`HTTP_BUFFER_BYTES`] + [`TCP_RX_BYTES`] + [`TCP_TX_BYTES`]
/// of static RAM, which is why this is four and not eight.
pub const HTTP_TASKS: usize = 4;

/// Tasks that can never be taken by a WebSocket.
///
/// **This is the anti-lockout guarantee, and it is checked below rather than
/// argued for.** However many browser tabs, stale integrations or abandoned
/// sockets exist, at least this many tasks are free to accept a connection and
/// answer a request — including the requests that would let an operator fix
/// whatever is going wrong.
pub const REST_TASKS_RESERVED: usize = HTTP_TASKS - WS_MAX;

// The reservation is only a guarantee while it is positive, and "positive"
// is not enough: one spare task would mean a single slow client delaying every
// other request behind it. Two is the floor because the UI's own dashboard
// makes three parallel requests, and a device that served them strictly one at
// a time would look broken while being correct.
const _: () = assert!(
    REST_TASKS_RESERVED >= 2,
    "WS_MAX must leave at least two tasks free for REST, or a browser holding \
     every WebSocket slot could make the device look unreachable",
);

/// Request buffer: the request line, the headers, and the body together.
///
/// Sized from the largest real request rather than rounded. A browser `POST` to
/// `/api/v1/shades` carries roughly 600 bytes of headers — `User-Agent` alone
/// is about 120, and the `Sec-Fetch-*`, `sec-ch-ua-*`, `Referer`, `Origin` and
/// `Accept-Language` set adds a few hundred more — plus a `CreateShadeDto` body
/// of about 160. The WebSocket upgrade is similar without a body.
///
/// 1,536 leaves roughly 700 bytes of headroom over that, which is the margin
/// for a browser that grows another header. A request that overruns it is
/// refused by `picoserve` rather than truncated.
const HTTP_BUFFER_BYTES: usize = 1_536;

/// TCP receive window.
///
/// Smaller than [`HTTP_BUFFER_BYTES`] deliberately: this is the window, not the
/// request, and `picoserve` drains it into the request buffer as the request
/// arrives. 1,024 holds most requests in a single segment and costs the rest
/// one extra round trip on a link where a round trip is under a millisecond.
const TCP_RX_BYTES: usize = 1_024;

/// TCP send buffer.
///
/// This one bounds throughput rather than correctness: the largest response is
/// the compressed application script at about 21 KB, and the buffer is how much
/// of it can be in flight unacknowledged. At 1,024 bytes that is roughly twenty
/// round trips, which on a home network is a few tens of milliseconds — paid
/// once per page load, and not at all on a reload, because the assets carry
/// compile-time ETags and answer `304`.
///
/// It is the figure to raise if the UI ever feels slow to load, and the one to
/// check first against `heap::report` if it does not.
const TCP_TX_BYTES: usize = 1_024;

/// The port. 80, because the UI's own `fetch` and `WebSocket` calls are
/// same-origin and relative — there is no port to configure and nowhere to
/// configure it.
///
/// `pub` because `crate::mdns` advertises it in an SRV record, and a second
/// literal `80` there would be a number that could drift from the one the server
/// actually binds — an advertisement pointing at a closed port, which is worse
/// than no advertisement.
pub const PORT: u16 = 80;

/// How long a connection may sit after the handshake without sending a request.
///
/// A **policy figure.** A browser that has completed a TCP handshake sends its
/// request immediately; three seconds is the allowance for a bad link, not a
/// measurement of one. It matters because it is the only way an idle connection
/// can occupy a task at all — see [`CONFIG`] — so it bounds how long a client
/// that opens sockets and says nothing can keep one.
const START_REQUEST_TIMEOUT_S: u64 = 3;

/// How long a half-sent request may stall before the connection is abandoned.
const READ_REQUEST_TIMEOUT_S: u64 = 3;

/// How long a write may stall before the connection is abandoned.
///
/// Five seconds rather than `picoserve`'s default of one: the largest response
/// is 21 KB against a 1 KB send buffer, so a slow client legitimately takes
/// many round trips to drain one. One second was observed to be the default and
/// is right for small responses; this device has one large one.
const WRITE_TIMEOUT_S: u64 = 5;

/// The server's configuration.
///
/// **`KeepAlive::Close`, which is `picoserve`'s default and is also the single
/// most important line in this module.** One request per TCP connection means a
/// client cannot *hold* a REST task at all: the task answers and goes straight
/// back to accepting. The reference implementation's lockout needed a client to
/// hold a connection open; this removes the ability rather than bounding it.
///
/// What it costs is a TCP handshake per request. On the LAN this device lives
/// on that is well under a millisecond, and the UI makes three requests on load
/// and one per button press. That is a good price for a property that cannot be
/// eroded by a future change to a timeout.
static CONFIG: Config = Config::new(Timeouts {
    start_read_request: picoserve::time::Duration::from_secs(START_REQUEST_TIMEOUT_S),
    // Unused while connections close after each response, and set to the same
    // figure anyway so that turning keep-alive on later cannot silently inherit
    // a value nobody chose.
    persistent_start_read_request: picoserve::time::Duration::from_secs(START_REQUEST_TIMEOUT_S),
    read_request: picoserve::time::Duration::from_secs(READ_REQUEST_TIMEOUT_S),
    write: picoserve::time::Duration::from_secs(WRITE_TIMEOUT_S),
});

// The delta channel's subscriber slots are what cap WebSockets — see
// [`events`] for why the subscription *is* the permit. One belongs to the
// broker session, which takes it at boot and holds it for the life of the
// program, so the channel has to carry one more than [`WS_MAX`].
//
// Checked rather than commented, because the failure it prevents is quiet: a
// channel one slot short would refuse the *last* WebSocket, at random, only on
// a board that also has a broker provisioned.
// Every connection task can be inside `rpc::Rpc::call` at once, and the gate
// queues waiters in a fixed-size FIFO. A pool larger than that queue would make
// `acquire` able to fail, which is a request refused for a reason nobody asked
// about.
const _: () = assert!(
    HTTP_TASKS <= crate::rpc::GATE_WAITERS,
    "the request gate must be able to queue every connection task",
);

const _: () = assert!(
    somfy_tasks::DELTA_SUBSCRIBERS > WS_MAX,
    "the delta channel needs a subscriber slot per websocket plus one for the \
     broker session",
);

/// The router, built once and shared by every task.
static ROUTER: StaticCell<Router<routes::AppRouter>> = StaticCell::new();

/// Per-connection buffers, one set per task.
///
/// A single static array rather than a `StaticCell` each, so the cost is one
/// number a reader can multiply out: `HTTP_TASKS × (1536 + 1024 + 1024)`.
static BUFFERS: StaticCell<[Buffers; HTTP_TASKS]> = StaticCell::new();

/// One task's buffers.
struct Buffers {
    http: [u8; HTTP_BUFFER_BYTES],
    rx: [u8; TCP_RX_BYTES],
    tx: [u8; TCP_TX_BYTES],
}

impl Buffers {
    const fn new() -> Buffers {
        Buffers {
            http: [0; HTTP_BUFFER_BYTES],
            rx: [0; TCP_RX_BYTES],
            tx: [0; TCP_TX_BYTES],
        }
    }
}

/// Start the web server, and never fail in a way that matters.
///
/// Returns a `SpawnError` and nothing else; the caller prints it and carries
/// on, exactly as it does for Wi-Fi and for the broker. A board that cannot
/// start a web server still receives, decodes, tracks and — if a broker is
/// provisioned — still answers Home Assistant.
pub fn start(spawner: Spawner, stack: Stack<'static>) -> Result<(), SpawnError> {
    let router = ROUTER.init(picoserve::AppBuilder::build_app(routes::App));
    let buffers = BUFFERS.init([const { Buffers::new() }; HTTP_TASKS]);

    // All tokens first, then all spawns — the same discipline `net::start` uses,
    // and for the same reason: a spawn that failed part-way would leave some
    // connections served and others not, which is harder to diagnose than none.
    let mut tokens = heapless::Vec::<_, HTTP_TASKS>::new();
    for (id, buffers) in buffers.iter_mut().enumerate() {
        let Buffers { http, rx, tx } = buffers;
        let token = connection(id, stack, router, http, rx, tx)?;
        // Cannot fail: the vector's capacity is the loop's bound.
        let _ = tokens.push(token);
    }
    // The settings screen's restart, which cannot be a `software_reset` inside a
    // handler: the handler has not written its response yet, and resetting there
    // would answer a successful save with a dropped connection — which looks
    // exactly like a failed one.
    let restarter = routes::restarter()?;
    for token in tokens {
        spawner.spawn(token);
    }
    spawner.spawn(restarter);

    esp_println::println!(
        "api: serving the UI and /api/v1 on port {} — {} connections, at most {} websockets, \
         {} always free for REST",
        PORT,
        HTTP_TASKS,
        WS_MAX,
        REST_TASKS_RESERVED,
    );
    shell::report();
    Ok(())
}

/// One connection, for the life of the program.
///
/// `pool_size` is what makes [`HTTP_TASKS`] a compile-time bound rather than a
/// hope: `embassy-executor` allocates exactly this many task futures and
/// `spawn` fails if a fifth is asked for.
#[embassy_executor::task(pool_size = HTTP_TASKS)]
async fn connection(
    id: usize,
    stack: Stack<'static>,
    router: &'static Router<routes::AppRouter>,
    http: &'static mut [u8; HTTP_BUFFER_BYTES],
    rx: &'static mut [u8; TCP_RX_BYTES],
    tx: &'static mut [u8; TCP_TX_BYTES],
) {
    // `listen_and_serve` accepts, serves and loops forever. Its return type is
    // the shutdown reason, and this server is built with
    // `core::future::Pending` as its shutdown signal — so the value below is
    // one nothing can ever produce, and the task does not return.
    let _: picoserve::NoGracefulShutdown = picoserve::Server::new(router, &CONFIG, http)
        .listen_and_serve(id, stack, PORT, rx, tx)
        .await;
}
