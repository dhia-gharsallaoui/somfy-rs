//! Wall-clock time, kept strictly separate from the clock everything else runs
//! on.
//!
//! # The one rule this module exists to keep
//!
//! **Nothing in this firmware may read the time from here.** Not the
//! rolling-code store, not the receive debounce, not the position estimator, not
//! a timeout anywhere. Those run on [`Instant`], which counts forward from boot
//! and cannot be adjusted, and that is not an accident of implementation — it is
//! the property that makes them correct.
//!
//! An SNTP correction moves the wall clock, and it can move it *backwards*: a
//! device that boots with no network and then reaches a server has its idea of
//! "now" jump by however long it was wrong. Anything that measured a duration by
//! subtracting two wall-clock readings would see negative time. In the position
//! estimator that is a shade at the wrong height; in the rolling-code store's
//! debounce it is a write that never lands. So the separation is structural:
//!
//! 1. **The wall clock is only ever an absolute answer.** [`unix_seconds`]
//!    returns a point in time and there is no `elapsed`, no `since`, and no
//!    subtraction helper anywhere in this module. Computing a duration from it
//!    takes deliberate arithmetic at the call site rather than a method that
//!    invites it.
//! 2. **It is an [`Option`], and it is `None` until a server has answered.**
//!    There is no plausible default and none is offered — a made-up epoch would
//!    be a confidently wrong answer, which is the failure class this project
//!    avoids everywhere else.
//! 3. **It is derived from [`Instant`] rather than replacing it.** [`Anchor`]
//!    stores the server's answer *and* the monotonic instant it applied to;
//!    every later reading is that pair plus monotonic elapsed time. So the
//!    monotonic clock is underneath the wall clock, not beside it, and a
//!    correction replaces the anchor without touching the thing everything else
//!    measures.
//! 4. **With the `sntp` feature off there is no wall clock in the image at
//!    all** — not an unset one, not a zero. That is the strongest form of the
//!    claim: it is not that nothing reads it, it is that there is nothing to
//!    read.
//!
//! # It is a degradable service, and it degrades to nothing happening
//!
//! Spec §11 and R9. A device with no internet, a blocked port 123, a resolver
//! that does not answer or an NTP server that refuses keeps working exactly as
//! it did before this module existed: [`unix_seconds`] stays `None`, the loop
//! backs off, and no shade command, position estimate or rolling code depends on
//! any of it. There is no path here that can fail in a way the radio notices —
//! the module is handed a [`Stack`] and returns a number, and it touches no
//! store, no queue and no channel.
//!
//! # Being a good citizen towards a public time server
//!
//! [`SERVER`] is a pool that volunteers run. Three things follow:
//!
//! - **One question an hour once the answer is known.** See
//!   [`RESYNC_INTERVAL_S`], which is set far below what the crystal's drift
//!   would require.
//! - **Bounded backoff on failure**, from [`RETRY_MIN_MS`] up — never a retry
//!   loop against a server that is refusing.
//! - **Kiss-o'-Death is honoured**, which is the mechanism RFC 5905 §7.4 gives a
//!   server to say "stop". See [`sync_once`].

use core::net::{SocketAddr, SocketAddrV4};

use embassy_executor::{SpawnError, Spawner};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use sntpc::{NtpContext, NtpTimestampGenerator};
use sntpc_net_embassy::UdpSocketWrapper;
use somfy_tasks::Backoff;

/// Sockets this module opens on the stack, plus the resolver's.
///
/// Two: the UDP socket the exchange runs on, and the DNS socket
/// `embassy_net::new` adds because this feature turns `embassy-net/dns` on. See
/// the neighbours of `crate::net`'s `SOCKETS` for the accounting.
pub const SOCKETS: usize = 2;

/// The time source.
///
/// `pool.ntp.org` is what deployed controllers of this kind default to, and
/// there is no better answer available to a device that ships without knowing
/// which network it will live on: it is a name rather than an address, so it
/// survives servers being retired, and it resolves to whichever pool members are
/// nearest.
///
/// **It is a name, which is why this module needs a resolver at all** — see
/// [`crate::net::resolve`], which also accepts an address literal, so the day
/// this becomes a configurable field a user may type either.
///
/// The pool's own guidance asks vendors shipping at volume to register a vendor
/// subdomain. This is not that; what a single device owes the pool is a low
/// question rate, which [`RESYNC_INTERVAL_S`] and [`RETRY_MAX_MS`] provide.
const SERVER: &str = "pool.ntp.org";

/// The NTP port. RFC 5905 §7.2.
const PORT: u16 = 123;

/// How long after a successful sync before asking again, in seconds.
///
/// **A policy figure inside a range the drift permits, and the range is wide.**
/// The bound that actually matters is how far the clock may wander between
/// answers, and this board's time base is a crystal specified at ±10 to ±20 ppm:
/// at 20 ppm an hour of drift is 72 ms and a day is 1.7 s. Both are far inside
/// what either consumer needs — a log line to the second, and a TLS certificate
/// whose validity window is measured in days.
///
/// So the drift argument permits a day, and an hour is chosen instead for a
/// different reason: it bounds how long a *wrong* answer persists. A single bad
/// exchange — a pool member with a broken clock, a captive portal answering port
/// 123 — is corrected within the hour rather than within the day.
///
/// What it costs the network is one 48-byte question and one 48-byte answer an
/// hour: about 2.3 KB a day including the DNS lookup.
const RESYNC_INTERVAL_S: u64 = 3_600;

/// Shortest wait between failed attempts, in milliseconds.
///
/// RFC 4330 §10 is explicit that a client "MUST NOT under any conditions use a
/// poll interval less than 15 seconds". A minute is four times that floor, which
/// is the right side of it to be on for a service nothing waits for: a board
/// that boots without internet is not made worse by learning the time a minute
/// later, and a public pool is made measurably worse by a fleet retrying every
/// fifteen.
const RETRY_MIN_MS: u32 = 60_000;

/// Longest wait between failed attempts, in milliseconds.
///
/// The same hour as [`RESYNC_INTERVAL_S`], deliberately: at the ceiling a device
/// that cannot reach a time server behaves exactly like one that can and is
/// simply refreshing. There is nothing to gain from asking a network that has
/// been silent for an hour any more often than a network that answered.
const RETRY_MAX_MS: u32 = 3_600_000;

/// How long one exchange may take before it is abandoned, in seconds.
///
/// **A policy figure**, and it is here because `sntpc` has no timeout of its
/// own: `get_time` awaits a datagram that a firewalled port 123 will never send,
/// and without this the task would stop for good on the first blocked network
/// while reporting nothing. Its own repository's timeout example is where the
/// remedy comes from.
///
/// Five seconds is roughly ten times the worst plausible round trip — a LAN
/// server answers in under a millisecond and a pool member across a domestic
/// connection in well under 500 ms — and dropping the future is safe: the socket
/// stays bound and is closed on the next line.
const REQUEST_TIMEOUT_S: u64 = 5;

/// Receive buffer for the exchange, in bytes.
///
/// An SNTP packet is 48 bytes (RFC 5905 §7.3) and `sntpc` refuses anything of a
/// different size as `IncorrectPayload`. 128 is that with room for the socket's
/// own bookkeeping and no attempt to be clever: this is the smallest buffer in
/// the image and the one place where rounding up costs nothing worth counting.
const PACKET_BYTES: usize = 128;

/// The server's answer, and the monotonic instant it was true at.
///
/// Both halves are the point. Storing only `unix_seconds` would make the wall
/// clock a value that goes stale silently; storing the [`Instant`] with it makes
/// every later reading an extrapolation from a monotonic base, which is what
/// keeps [`Instant`] underneath the wall clock rather than beside it.
#[derive(Clone, Copy)]
struct Anchor {
    /// Seconds since 1970-01-01T00:00:00Z, as the server reported them.
    unix_seconds: u64,
    /// Uptime when that was true.
    at: Instant,
}

/// The one wall clock in this firmware.
///
/// **A `blocking_mutex` around a `Cell` rather than an atomic, for the reason
/// `crate::net::SIGNAL_DBM` gives**: `riscv32imc` — the ESP32-C3's target — has
/// no atomic read-modify-write instruction, so the natural shape is unavailable
/// on one of the supported chips. A critical-section mutex costs a handful of
/// instructions and is held for a single load or store.
///
/// `None` until a server has answered, and it never returns to `None`: an answer
/// that was true an hour ago plus monotonic elapsed time is still a better
/// estimate than nothing, and losing the network does not make the past
/// uncertain.
static WALL_CLOCK: Mutex<CriticalSectionRawMutex, core::cell::Cell<Option<Anchor>>> =
    Mutex::new(core::cell::Cell::new(None));

/// Seconds since the UNIX epoch, or `None` if no server has answered yet.
///
/// **Not a substitute for [`Instant::now`], and it is shaped so that using it as
/// one is awkward.** There is no `elapsed`, no `duration_since` and no
/// `Sub` impl — a caller that wants an interval has to write the subtraction
/// itself, at which point the question "what happens if the clock stepped
/// backwards between these two readings?" is in front of them rather than
/// hidden behind a method name. Today nothing subtracts two of these, and this
/// module's docs say why nothing should.
#[allow(
    dead_code,
    reason = "Plan 6 Task 6's TLS certificate validity and the log timestamps are \
              the consumers; the clock is acquired and reported before either \
              exists, because a clock that starts working only once something \
              needs it cannot be observed to work"
)]
pub fn unix_seconds() -> Option<u64> {
    let anchor = WALL_CLOCK.lock(core::cell::Cell::get)?;
    // Saturating rather than wrapping: `Instant::now()` cannot precede `at`, but
    // an arithmetic surprise here would be a wall clock reading decades out and
    // nothing would say so.
    Some(anchor.unix_seconds + anchor.at.elapsed().as_secs())
}

/// Start the SNTP client, and never fail in a way that matters.
///
/// Returns a `SpawnError` and nothing else; the caller prints it and carries on,
/// exactly as it does for Wi-Fi, the web server and the broker.
pub fn start(spawner: Spawner, stack: Stack<'static>) -> Result<(), SpawnError> {
    let token = client(stack)?;
    spawner.spawn(token);
    esp_println::println!(
        "sntp: asking {} for the time, then every {} s. Nothing here affects the radio.",
        SERVER,
        RESYNC_INTERVAL_S,
    );
    Ok(())
}

/// Ask for the time, forever.
///
/// Same shape as `crate::net`'s `wifi_link`: attempt, report, wait. The wait is
/// outside the match so there is no path around it.
#[embassy_executor::task]
async fn client(stack: Stack<'static>) -> ! {
    let mut backoff = Backoff::new(RETRY_MIN_MS, RETRY_MAX_MS);
    loop {
        // An address, not just an association: a station can be associated and
        // have no route, which is the state in which a resolver answers nothing.
        stack.wait_config_up().await;

        let waiting = match sync_once(stack).await {
            Ok(seconds) => {
                let first = WALL_CLOCK.lock(core::cell::Cell::get).is_none();
                WALL_CLOCK.lock(|cell| {
                    cell.set(Some(Anchor {
                        unix_seconds: seconds,
                        at: Instant::now(),
                    }))
                });
                // Only the first is worth a line. After that it is one message
                // an hour saying the clock is still the clock, and `crate::net`
                // already carries the argument about what a log line costs a
                // cooperative executor.
                if first {
                    esp_println::println!(
                        "sntp: wall clock set — {} seconds since the UNIX epoch. \
                         Monotonic uptime is unaffected and remains what the store, \
                         the debounce and the position estimator run on.",
                        seconds,
                    );
                }
                backoff.succeed();
                RESYNC_INTERVAL_S * 1_000
            }
            Err(reason) => {
                let waiting = backoff.fail();
                esp_println::println!(
                    "sntp: no time this round ({}) — retrying in {} ms. \
                     The controller is unaffected.",
                    reason,
                    waiting,
                );
                u64::from(waiting)
            }
        };

        Timer::after(Duration::from_millis(waiting)).await;
    }
}

/// One resolve-and-ask, with every way it can fail bounded.
async fn sync_once(stack: Stack<'static>) -> Result<u64, &'static str> {
    let address = crate::net::resolve(stack, SERVER)
        .await
        .ok_or("the server name did not resolve")?;

    let mut receive_meta = [PacketMetadata::EMPTY; 1];
    let mut receive = [0u8; PACKET_BYTES];
    let mut transmit_meta = [PacketMetadata::EMPTY; 1];
    let mut transmit = [0u8; PACKET_BYTES];

    let mut socket = UdpSocket::new(
        stack,
        &mut receive_meta,
        &mut receive,
        &mut transmit_meta,
        &mut transmit,
    );
    // Port 0 asks `embassy-net` for an ephemeral one. Not 123: this is a client,
    // and binding the service port would make the device answer time queries it
    // has no business answering.
    socket.bind(0).map_err(|_| "no local port to bind")?;

    // **`SocketAddr::V4`, explicitly.** `sntpc-net-embassy`'s address conversion
    // answers an IPv6 `SocketAddr` with `unreachable!()` unless its `ipv6`
    // feature is on, and a panic here would reset the board. `net::resolve` asks
    // only for `A` records and hands back an `Ipv4Addr`, so the type makes that
    // unreachable for real rather than by inspection.
    let server = SocketAddr::V4(SocketAddrV4::new(address, PORT));
    let wrapped = UdpSocketWrapper::from(socket);
    let context = NtpContext::new(Pivot::default());

    // `sntpc` has no timeout of its own; see `REQUEST_TIMEOUT_S`.
    let answer = with_timeout(
        Duration::from_secs(REQUEST_TIMEOUT_S),
        sntpc::get_time(server, &wrapped, context),
    )
    .await
    .map_err(|_| "the server did not answer in time")?;

    match answer {
        Ok(result) => Ok(result.sec()),
        // **Kiss-o'-Death, RFC 5905 §7.4: the server is telling us to stop.**
        // Every code is answered the same way and deliberately so — the backoff
        // goes to its ceiling and this device asks once an hour, which is what
        // `RATE` requires and is well inside what `DENY` and `RSTR` are asking
        // for from a client with one server name.
        //
        // It is *not* treated as a permanent stop for the boot, because the name
        // is re-resolved every round and the pool hands out a different member
        // each time: refusing the pool forever because one of its members
        // refused once would turn a rate limit into a dead clock.
        Err(sntpc::Error::KissOfDeath(_)) => {
            Err("the server sent a Kiss-o'-Death and is being left alone")
        }
        Err(sntpc::Error::UnsynchronizedClock) => {
            Err("the server says its own clock is unsynchronized")
        }
        Err(_) => Err("the server's answer did not check out"),
    }
}

/// The timestamp source `sntpc` reads, and it is doing two jobs.
///
/// The obvious one is the round-trip arithmetic: `sntpc` stamps the outgoing
/// packet, compares the echo, and computes a delay. Any monotonic source does
/// that correctly, which is what the crate's own `sntpc-time-embassy` supplies.
///
/// The other job is why that crate is not used here. `timestamp_sec` is also the
/// **pivot** `sntpc` reconstructs the NTP era from: wire timestamps wrap every
/// 2^32 seconds, and the crate picks whichever of the three eras nearest the
/// pivot best matches, *silently* — a pivot more than half an era out yields a
/// plausible, wrong answer with no error raised, which its own tests assert.
///
/// `sntpc-time-embassy`'s pivot is uptime, so it is near zero forever. That is
/// not wrong today — with a pivot at the epoch the reconstruction is exact until
/// roughly 2106, because era-0 candidates below the epoch are skipped — and it
/// can never become exact either, because the crate offers no way to hand it a
/// better one: its generator has a private field, a `Default` impl and no
/// constructor.
///
/// This one uses the device's own wall clock as the pivot once there is one, and
/// falls back to uptime before that. So the first exchange of a boot is as
/// correct as the crate's version, and every one after it is exact rather than
/// lucky.
#[derive(Clone, Copy, Default)]
struct Pivot {
    /// Microseconds since the UNIX epoch if the wall clock is set, and since
    /// boot if it is not. Captured whole so the two accessors below cannot
    /// disagree about which instant they describe.
    micros: u64,
}

impl NtpTimestampGenerator for Pivot {
    fn init(&mut self) {
        let uptime = Instant::now();
        self.micros = match WALL_CLOCK.lock(core::cell::Cell::get) {
            Some(anchor) => anchor.unix_seconds * 1_000_000 + (uptime - anchor.at).as_micros(),
            None => uptime.as_micros(),
        };
    }

    fn timestamp_sec(&self) -> u64 {
        self.micros / 1_000_000
    }

    fn timestamp_subsec_micros(&self) -> u32 {
        // Cannot exceed 999_999, so the cast cannot lose anything.
        (self.micros % 1_000_000) as u32
    }
}

// A buffer too small to hold one packet is a client that fails on every network
// and says `IncorrectPayload` about it, which names the server rather than the
// buffer. Confirmed to fire by writing 32 here and restoring.
const _: () = assert!(
    PACKET_BYTES >= 48,
    "an SNTP packet is 48 bytes (RFC 5905 §7.3); a buffer below that cannot \
     hold one, and the failure would be reported as a bad answer from the \
     server rather than as this",
);

// The resync interval must not fall below RFC 4330 §10's floor either, and it is
// the constant most likely to be "tuned" by somebody wanting the clock to settle
// faster. Confirmed to fire by writing 10 above and restoring.
const _: () = assert!(
    RESYNC_INTERVAL_S * 1_000 >= RETRY_MIN_MS as u64,
    "asking again sooner than the retry floor would make a working server \
     queried harder than a broken one; see sntp::RETRY_MIN_MS for the RFC 4330 \
     §10 minimum it is built on",
);
