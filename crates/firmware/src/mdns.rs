//! `http://<hostname>.local` — the mDNS responder, and the reasons it is quiet.
//!
//! # What it is for
//!
//! Without it the web UI lives at whatever address DHCP handed out, which a
//! person has to read off a serial console or a router's client list and then
//! remember until the lease moves. With it the address is a name derived from
//! this board's own MAC, and [`crate::identity`] carries why that name is the
//! one the broker session already uses.
//!
//! # It is a degradable service, like the broker and the web server
//!
//! Spec §11 and R9: a network service that is absent, broken or misbehaving must
//! not affect radio control. The same four structural properties
//! [`crate::api`] lists hold here, and one of them is stronger:
//!
//! 1. **No shared state.** Nothing in this module touches the store, the
//!    transmit queue, the frame channel or the registry. It is handed a
//!    [`Stack`] and a string, and there is no other way in.
//! 2. **The radio tasks are already running** when [`start`] is called, and its
//!    failure is printed and ignored.
//! 3. **Fixed cost.** One task, one socket, four buffers, all sized below.
//!    Nothing here grows under load, because nothing here has a per-client
//!    anything — an mDNS responder answers a datagram and forgets it.
//! 4. **A remote peer cannot make it allocate.** `edge-mdns` is no-alloc: the
//!    parse buffer and the response buffer are the two arrays declared in
//!    [`responder`], and a query larger than the first is dropped by the socket
//!    rather than growing anything.
//!
//! # Being a good citizen on a multicast group
//!
//! mDNS is the one thing this firmware does that every other device on the
//! network is obliged to receive. Four decisions follow from that, and the
//! first is the one that matters:
//!
//! - **Nothing here is periodic.** `edge-mdns`'s broadcast half is not a timer;
//!   it blocks on a [`Signal`] between announcements, and this module owns the
//!   signal. So the complete list of unsolicited packets this device puts on
//!   224.0.0.251 is: [`ANNOUNCEMENTS`] of them when a DHCP address arrives, and
//!   [`ANNOUNCEMENTS`] more if the address ever changes. In the steady state it
//!   transmits only in reply to a question addressed to it.
//! - **The announcement burst is the shape RFC 6762 §8.3 asks for** — at least
//!   two, one second apart, with the interval at least doubling — and no more.
//!   See [`ANNOUNCEMENTS`] and [`FIRST_ANNOUNCE_GAP_MS`].
//! - **No probing, because the name cannot collide.** §8.1 requires a responder
//!   to probe for conflicts before claiming a name, and `edge-mdns` does not.
//!   That is sound *only* while the name is derived from a globally unique
//!   hardware identifier, which is what [`crate::identity::hostname`] is. It
//!   stops being sound the day the hostname becomes user-supplied, and that is
//!   recorded there as well as here.
//! - **One deviation, stated rather than hidden.** §6 asks for a 20-120 ms
//!   random delay before answering a query for a *shared* record;
//!   `edge-mdns`'s `HostAnswersMdnsHandler` answers immediately (`delay: false`).
//!   For our unique records — the A, SRV and TXT — replying at once is what §6
//!   permits. The `_http._tcp` PTR is formally shared, so a network with two
//!   somfy-rs boards would see their two replies un-jittered. With one service
//!   instance per board and a name per board, that is a collision of timing
//!   rather than of content, and it costs a duplicate packet nobody parses
//!   twice.
//!
//! # Why this is gated on `http`
//!
//! Because `_http._tcp` on port 80 is the only thing this device has to
//! advertise, and `crates/firmware/Cargo.toml` carries that argument in full.

use core::convert::Infallible;
use core::net::{Ipv4Addr, Ipv6Addr};

use edge_mdns::buf::VecBufAccess;
use edge_mdns::domain::base::Ttl;
use edge_mdns::host::{Host, Service, ServiceAnswers};
use edge_mdns::io::{Mdns, IPV4_DEFAULT_SOCKET};
use edge_mdns::HostAnswersMdnsHandler;
use edge_nal::UdpSplit;
use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_executor::{SpawnError, Spawner};
use embassy_futures::select::select;
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

/// Sockets this module opens on the stack. Read by [`crate::net`].
pub const SOCKETS: usize = 1;

/// The record lifetime this device advertises, in seconds.
///
/// **120, and it is one number doing two jobs badly on purpose.** RFC 6762 §10
/// asks for 120 seconds on records naming a host — the A record, and the SRV
/// that contains a host name — and 75 minutes on everything else, which here
/// means the PTR and TXT records of the service. `edge-mdns` has one TTL for all
/// of them, so one has to be chosen.
///
/// It is chosen for the A record, because that is the one whose staleness is a
/// failure a person meets: a DHCP lease moves, a cached A record points at an
/// address this board no longer holds, and `http://<hostname>.local` reaches
/// either nothing or somebody else's device. Two minutes bounds that.
///
/// What it costs is that the service records are re-queried far more often than
/// they need to be. That cost is smaller than it looks: cache-maintenance
/// queries (§5.2) are issued by *queriers*, at 80-95% of the TTL, and only while
/// something is actively browsing for `_http._tcp`. A network with nobody
/// browsing pays nothing.
const TTL_SECONDS: u32 = 120;

/// Unsolicited announcements sent when the address arrives or changes.
///
/// RFC 6762 §8.3: "MUST send at least two unsolicited responses, one second
/// apart", and MAY send up to eight with the interval doubling. Three is the
/// minimum plus one, which is what buys tolerance of a single dropped packet
/// without spending the multicast group's attention on a device that has nothing
/// urgent to say.
///
/// The first is not scheduled here: `edge-mdns`'s broadcast loop announces once
/// on entry and *then* waits on the signal, so this module raises the signal
/// [`ANNOUNCEMENTS`] − 1 times.
const ANNOUNCEMENTS: u32 = 3;

/// Gap before the second announcement, in milliseconds.
///
/// One second, which is the floor §8.3 states. The gaps after it double, so the
/// burst finishes 3 seconds after the address arrives.
const FIRST_ANNOUNCE_GAP_MS: u64 = 1_000;

/// How long the responder waits before restarting after a fatal error.
///
/// A **policy figure**, and it exists because `edge-mdns`'s `run` can return an
/// `Err` that ends the responder for good — a response that does not fit
/// [`RESPONSE_BYTES`] is the realistic one. Restarting turns that from a service
/// that silently stopped into a service that logs and comes back; five seconds
/// is short enough not to matter and long enough that a permanently failing
/// configuration is a line every five seconds rather than a spin.
const RESTART_DELAY_MS: u64 = 5_000;

/// Bytes reserved for one outgoing mDNS message.
///
/// **Derived, and the derivation matters because overrunning it is fatal rather
/// than lossy**: `domain`'s builder answers a full buffer with a `PushError`,
/// `edge-mdns` turns that into an error out of `run`, and the responder ends.
/// [`RESTART_DELAY_MS`] is the safety net; this is the number that keeps the net
/// from being needed.
///
/// The largest message is the answer to a `_http._tcp` browse, which carries
/// every record this device owns. Uncompressed, with an 18-character hostname:
///
/// ```text
/// header                                                    12
/// A       somfy-<mac>.local                        26 + 10 + 4  = 40
/// PTR     _services._dns-sd._udp.local -> _http._tcp.local
///                                          29 + 10 + 17        = 56
/// PTR     _http._tcp.local -> somfy-<mac>._http._tcp.local
///                                          17 + 10 + 45        = 72
/// SRV     somfy-<mac>._http._tcp.local             45 + 10 + 32 = 87
/// TXT     somfy-<mac>._http._tcp.local             45 + 10 + 10 = 65
///                                                              ----
///                                                               332
/// ```
///
/// 1,024 is three times that. The margin is deliberately generous rather than
/// tight because the thing it protects against is not a byte count anyone can
/// re-derive from the source — `domain` compresses names, so the real message is
/// smaller than the table, and a future record type would grow it in a way this
/// comment would not notice.
const RESPONSE_BYTES: usize = 1_024;

/// Bytes reserved for one incoming mDNS message.
///
/// Smaller than [`RESPONSE_BYTES`] because overrunning it is *not* fatal: a
/// datagram that does not fit is dropped by the socket, and a message that
/// arrives truncated fails to parse and is skipped with a warning, after which
/// the responder carries on.
///
/// 512 holds any question anyone asks about this device — a query for one name
/// is well under 100 bytes. What it can drop is a large query carrying
/// known-answer suppression records (§7.1), which happens on a busy network
/// during a `_services._dns-sd._udp` enumeration. The cost of dropping one is
/// that this device does not appear in that enumeration until the next query;
/// the cost of sizing for it is 1 KB of DRAM on the chip that has the least.
const REQUEST_BYTES: usize = 512;

/// Start the mDNS responder, and never fail in a way that matters.
///
/// Returns a `SpawnError` and nothing else; the caller prints it and carries on,
/// exactly as it does for Wi-Fi, the web server and the broker. A board with no
/// mDNS is a board whose UI is reached by address, which is where it was before
/// this module existed.
pub fn start(spawner: Spawner, stack: Stack<'static>) -> Result<(), SpawnError> {
    let hostname = crate::identity::hostname();
    let token = responder(stack)?;
    spawner.spawn(token);
    crate::logln!(
        "mdns: answering for '{}.local' — the UI is at http://{}.local",
        hostname,
        hostname,
    );
    Ok(())
}

/// Answer mDNS queries for as long as this device has an address.
///
/// The loop is deliberately dull, and its shape is the one [`crate::net`]'s
/// `address_watch` uses: wait for an address, serve, wait for it to go away,
/// repeat. It is written that way because the responder's whole answer — the A
/// record — is the address, so an address change has to rebuild it rather than
/// patch it.
#[embassy_executor::task]
async fn responder(stack: Stack<'static>) -> ! {
    let hostname = crate::identity::hostname();
    // `_http._tcp` instance name. The same string as the host, which is what
    // makes a browser's service list read as one device rather than two.
    let instance = hostname.clone();

    loop {
        stack.wait_config_up().await;
        let Some(config) = stack.config_v4() else {
            // Unreachable in practice — `wait_config_up` returned — and a panic
            // here would take the radio off the air over a name.
            continue;
        };
        let address = config.address.address();

        if let Err(error) = serve(stack, &hostname, &instance, address).await {
            crate::logln!(
                "mdns: the responder stopped ({:?}) — retrying in {} ms. The UI is still \
                 reachable at http://{}/",
                error,
                RESTART_DELAY_MS,
                address,
            );
            Timer::after(Duration::from_millis(RESTART_DELAY_MS)).await;
        }
    }
}

/// One address's worth of responding.
///
/// Returns `Ok(())` when the address went away, which is not a failure, and an
/// error when the responder itself stopped, which is.
async fn serve(
    stack: Stack<'static>,
    hostname: &str,
    instance: &str,
    address: Ipv4Addr,
) -> Result<(), MdnsError> {
    // One socket, and the pool is sized to say so. `edge-nal-embassy` hands out
    // sockets from a pool because its callers generally want several; this
    // caller wants exactly one, for the life of one DHCP lease.
    let buffers: UdpBuffers<1, RESPONSE_BYTES, REQUEST_BYTES, 4> = UdpBuffers::new();
    let udp = Udp::new(stack, &buffers);

    // **`IPV4_DEFAULT_SOCKET`, not `DEFAULT_SOCKET`.** The latter is an alias for
    // the IPv6 wildcard and `edge-mdns`'s own docs say "don't use in production
    // code"; this stack has no IPv6 at all, so binding it would fail at run time
    // in a build that compiles perfectly.
    //
    // The third argument is what joins 224.0.0.251, and `UNSPECIFIED` means "on
    // whichever interface has a route" rather than "on address 0.0.0.0".
    let mut socket =
        edge_mdns::io::bind(&udp, IPV4_DEFAULT_SOCKET, Some(Ipv4Addr::UNSPECIFIED), None)
            .await
            .map_err(|_| MdnsError::Bind)?;
    let (recv, send) = socket.split();

    let request = VecBufAccess::<NoopRawMutex, REQUEST_BYTES>::new();
    let response = VecBufAccess::<NoopRawMutex, RESPONSE_BYTES>::new();

    let host = Host {
        hostname,
        ipv4: address,
        // No AAAA record: `edge-mdns` reads `UNSPECIFIED` as "do not answer
        // AAAA queries", which is the truth — this stack is IPv4-only.
        ipv6: Ipv6Addr::UNSPECIFIED,
        ttl: Ttl::from_secs(TTL_SECONDS),
    };
    let service = Service {
        name: instance,
        // Priority and weight are SRV's server-selection fields and mean nothing
        // with one instance. Zero is the conventional "no preference expressed".
        priority: 0,
        weight: 0,
        service: "_http",
        protocol: "_tcp",
        port: crate::api::PORT,
        service_subtypes: &[],
        // `path=/` is the key Home Assistant, Avahi's browser and most service
        // browsers look for to build a clickable URL. Without it a discovered
        // service is an address and a port rather than a link.
        txt_kvs: &[("path", "/")],
    };

    // The announcement trigger. Nothing else in this firmware can reach it,
    // which is what makes the "no periodic multicast" claim above checkable
    // rather than a promise.
    let announce = Signal::<NoopRawMutex, ()>::new();

    let responder = Mdns::<_, _, _, _, _, NoopRawMutex>::new(
        Some(Ipv4Addr::UNSPECIFIED),
        None,
        recv,
        send,
        &request,
        &response,
        ChipRng(esp_hal::rng::Rng::new()),
        &announce,
    );

    let handler = HostAnswersMdnsHandler::new(ServiceAnswers::new(&host, &service));

    // Announce, then hold until the address goes away — one future, so the
    // responder is the only thing being selected against.
    let lifetime = async {
        announce_burst(&announce).await;
        stack.wait_config_down().await;
    };

    match select(responder.run(handler), lifetime).await {
        embassy_futures::select::Either::First(outcome) => {
            outcome.map_err(|_| MdnsError::Responder)
        }
        embassy_futures::select::Either::Second(()) => Ok(()),
    }
}

/// Raise the announcement signal on the RFC 6762 §8.3 schedule.
///
/// One fewer than [`ANNOUNCEMENTS`], because the responder's first announcement
/// is unprompted — it broadcasts on entry and only then waits on the signal.
async fn announce_burst(announce: &Signal<NoopRawMutex, ()>) {
    let mut gap = FIRST_ANNOUNCE_GAP_MS;
    for _ in 1..ANNOUNCEMENTS {
        Timer::after(Duration::from_millis(gap)).await;
        announce.signal(());
        // "provided that the interval between unsolicited responses increases by
        // at least a factor of two with each response sent".
        gap *= 2;
    }
}

/// Why the responder is not running.
///
/// Reported and then retried; nothing here reaches the radio. The two variants
/// are kept apart because they mean different things to whoever reads the line:
/// a bind failure is a stack that has no room or no address, and a responder
/// failure is a message this device could not build.
#[derive(Debug)]
enum MdnsError {
    /// The UDP socket could not be bound, or the multicast group not joined.
    Bind,
    /// The responder itself stopped. See [`RESPONSE_BYTES`] for the likely one.
    Responder,
}

/// The chip's hardware RNG, as the trait `edge-mdns` asks for.
///
/// It wants randomness for one thing: the 20-120 ms jitter RFC 6762 §6 puts
/// before a broadcast reply, so that every responder on a network does not
/// answer the same query in the same microsecond. Nothing here is
/// security-relevant, and the true RNG is running anyway because the radio is.
///
/// Four methods rather than a crate, because there is no `rand_core` 0.10
/// adapter for `esp-hal`'s RNG in the esp-rs stack — the `rand_core` version
/// changed the trait to a fallible `TryRng` with a blanket `Rng` impl, and the
/// adapters that exist target 0.6 or 0.9.
struct ChipRng(esp_hal::rng::Rng);

impl rand_core::TryRng for ChipRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        Ok(self.0.random())
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        Ok((u64::from(self.0.random()) << 32) | u64::from(self.0.random()))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        // Chunked rather than byte-at-a-time: each `random()` is a peripheral
        // read, and the caller asks for one byte at a time today anyway.
        for chunk in dst.chunks_mut(4) {
            let word = self.0.random().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
        Ok(())
    }
}
