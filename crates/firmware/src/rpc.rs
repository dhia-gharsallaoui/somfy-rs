//! The seam between the web server and the state task.
//!
//! # Why a request/response seam and not a shared registry
//!
//! For exactly the reason [`crate::inventory`] and [`crate::edits`] give: the
//! registry belongs to the state task, and nothing may reach across that
//! boundary. A registry behind a mutex would mean an HTTP handler holding a
//! lock the state task needs to plan an arrival stop, and an arrival stop that
//! arrives late is a shade that overshoots. The rule that keeps a network
//! service from being able to affect radio control is that there is no shared
//! mutable state to contend for, and this module does not create any.
//!
//! What it adds over the edit channel is an **answer**. `POST /api/v1/shades`
//! owes its client `201` with the id the registry assigned and the address this
//! controller allocated, and nothing outside the state task can produce either.
//! A fire-and-forget queue has nowhere to put that, so HTTP gets a seam that
//! does — while the *work* stays exactly where it was:
//! [`crate::tasks::apply_edit`] and `somfy_tasks::StateMachine::apply`, the same
//! two functions the MQTT path reaches.
//!
//! # How one request at a time is enough
//!
//! [`Rpc::gate`] admits one caller at a time, so a second waits rather than
//! racing. That is not a throughput compromise worth optimising: every reply
//! here is assembled in microseconds from memory the state task already owns,
//! and the alternative — several requests in flight against one registry —
//! would need per-request correlation for no gain. It also bounds the seam's
//! cost, which matters more: however many HTTP connections exist, the state
//! task sees one extra wake-up at a time.
//!
//! **A `FairSemaphore` and not an async `Mutex`**, and the difference is not
//! stylistic. `embassy_sync::mutex::Mutex` holds a *single* `WakerRegistration`,
//! and its own documentation says what two waiters do to it: they "wake each
//! other in a loop fighting over this WakerRegistration", which wastes CPU
//! until the holder releases. That is not a corner case here — the UI's
//! dashboard opens with three parallel `GET`s, and each list walk takes the
//! gate once per entity, so an ordinary page load is sustained three-way
//! contention. A `FairSemaphore` keeps a FIFO queue instead, and its capacity
//! is [`crate::api::HTTP_TASKS`] because that is exactly how many callers can
//! exist — so the `WaitQueueFull` it can return is unreachable, and it is
//! reported rather than ignored anyway.
//!
//! Serialising is also what makes the two signals a *rendezvous* rather than a
//! race. `Signal` holds one value and overwrites, so with several callers in
//! flight one request could replace another's before the state task read it.
//! With the gate there is at most one outstanding exchange, and the ordering
//! that makes it safe is written out at [`Rpc::call`]: clear the reply, then
//! signal the request, then wait — so an answer left behind by a caller whose
//! future was dropped cannot be mistaken for this one's. No borrow is held
//! across an await, so there is no `RefCell` to panic and no `unsafe` to
//! justify.
//!
//! # Lists are walked, not snapshotted
//!
//! [`Request::ShadeFrom`] asks for *the next shade at or after this slot*, so a
//! list of three shades costs four round trips rather than one 2.5 KB static
//! holding thirty-two DTOs. On a device whose heap is sized by subtracting its
//! statics from its DRAM (see [`crate::heap`]), that buffer would have been
//! paid for in Wi-Fi driver headroom on every boot, including the boots where
//! nobody opens the UI.
//!
//! The cost is that a list is not atomic: a shade added while one is being
//! walked may or may not appear. That is honest rather than merely tolerable —
//! ids are registry slots and never move, so the walk cannot skip or duplicate
//! an untouched shade; the worst case is a list that reflects the table as of
//! partway through, which is what any client polling a live device gets anyway.

use embassy_sync::semaphore::{FairSemaphore, Semaphore as _};
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration};
use somfy_api::{ApiErrorDto, CalibrationStepDto, GroupDto, RoomDto, ShadeDto};
#[cfg(feature = "http")]
use somfy_api::{MqttSettingsDto, MqttUpdateDto, WifiSettingsDto, WifiUpdateDto};
#[cfg(feature = "http")]
use somfy_config::WifiCredentials;
use somfy_domain::ShadeId;
use somfy_tasks::ControlCommand;

use crate::edits::ShadeEdit;
use crate::tasks::Mutex;

#[allow(
    dead_code,
    reason = "read by `Rpc::call`, whose caller is the web server"
)]
/// How long a caller waits for the state task before giving up.
///
/// A **policy figure, not a measurement.** The state task cannot fail to answer
/// — it never returns, it never blocks on anything unbounded, and its longest
/// single action is a flash sector erase, tens of milliseconds typical with a
/// datasheet worst case in the hundreds. Five seconds is two orders of
/// magnitude past that, so this cannot fire for any reason except a fault.
///
/// It exists anyway because the alternative to a bound is an HTTP task wedged
/// for the life of the boot, holding [`GATE`] and taking every later request
/// down with it. A degradable service must degrade, and "answer 503" is a
/// degradation where "never answer again" is not.
const REPLY_TIMEOUT_S: u64 = 5;

/// What the web server asks the state task for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "constructed by the web server; the state task answers every \
              variant either way"
)]
pub enum Request {
    /// The first shade in a slot at or after this one, if any.
    ShadeFrom(u8),
    /// The first group in a slot at or after this one, if any.
    GroupFrom(u8),
    /// The first room in a slot at or after this one, if any.
    RoomFrom(u8),
    /// One shade by id, for `GET /api/v1/shades/{id}`.
    Shade(ShadeId),
    /// Move something. Carries the *same* type the MQTT path puts on the
    /// command channel, and is applied by the same function.
    Command(ControlCommand),
    /// Transmit a pairing burst at one shade.
    ///
    /// Separate from [`Request::Command`] rather than folded into it, because
    /// it carries one rule a movement does not: a shade whose address came from
    /// another controller must be refused, and only the state task can see the
    /// address to judge that.
    Pair(ShadeId),
    /// One step of a guided travel-time calibration.
    ///
    /// Separate from [`Request::Command`] because only two of its four steps
    /// transmit anything, and because what it changes is the shade's stored
    /// *configuration* rather than its position — a finished run has to be
    /// written to flash the same way an edit is.
    Calibrate(ShadeId, CalibrationStepDto),
    /// Change the table.
    Edit(ShadeEdit),

    // -----------------------------------------------------------------------
    // Settings
    //
    // These reach the *configuration* region rather than the shade table, and
    // they come here for the same reason everything else does: the flash
    // peripheral has one owner and it is this task. See `crate::config`.
    //
    // **The resolution of a write-only secret happens on the far side of this
    // seam, not on this one.** A `SecretDto::Keep` is resolved against the
    // stored value by the state task, which is the only thing that can read it,
    // so a passphrase is never carried in this direction — the web server sends
    // the *instruction*, not the secret it stands in for.
    //
    // **These four are the one part of this seam that is `#[cfg]`-gated**, and
    // the reason is measured rather than tidy. This enum lives in a `Signal`,
    // which is a static sized to its largest variant, and `SaveMqtt` is the
    // largest by a wide margin — six fields with doubled inbound capacity. On a
    // build with no web server nothing can construct one, so an ungated variant
    // would be several hundred bytes of DRAM taken out of the Wi-Fi driver's
    // heap on every boot of a board that has no settings screen. The ESP32 is
    // exactly that board — it cannot link the web server at all — and it is the
    // chip with the least heap headroom to spare.
    // -----------------------------------------------------------------------
    /// What the device is provisioned with, minus every secret.
    #[cfg(feature = "http")]
    Settings,
    /// Validate a candidate credential against what is stored, without applying
    /// or storing anything.
    ///
    /// Separate from [`Request::SaveWifi`] because it runs *before* the radio is
    /// touched: an SSID one byte too long must cost no connection at all. It is
    /// also where a `psk` of "keep what you have" becomes the stored
    /// passphrase, which nothing outside the state task can do.
    #[cfg(feature = "http")]
    PrepareWifi(WifiUpdateDto),
    /// Store a credential a trial has proved. See `crate::trial`.
    #[cfg(feature = "http")]
    SaveWifi(WifiCredentials),
    /// Validate and store broker settings, resolving the password against what
    /// is stored.
    #[cfg(feature = "http")]
    SaveMqtt(MqttUpdateDto),
    /// Run without a broker. Not an absence — a device with no broker still
    /// receives, decodes and tracks — so it is its own request rather than an
    /// empty [`Request::SaveMqtt`].
    #[cfg(feature = "http")]
    ClearMqtt,

    // -----------------------------------------------------------------------
    // Firmware updates
    //
    // These reach flash for the same reason the settings do — the peripheral
    // has one owner and it is the state task — but they are unusual in one
    // way worth naming: an upload sends one of them **per page**, a few
    // thousand times for one image. That is affordable precisely because this
    // seam is a rendezvous rather than a queue: each round trip is two
    // executor polls against a flash write measured in milliseconds.
    //
    // The bytes do *not* travel in this enum. It lives in a `Signal`, which is
    // a static sized to its largest variant, so a page-carrying variant would
    // be that page of DRAM taken out of the Wi-Fi driver's heap on every boot
    // of every board. The page travels through `crate::ota`'s zero-copy
    // channel and this carries only its length.
    //
    // All four are `http`-gated because only a web server can produce one.
    // The boot-side self-test — which exists on every chip that has an
    // `otadata` region, including the one that cannot link a web server —
    // deliberately does **not** come through here: it runs on the state task's
    // own ticker and writes `otadata` directly, because that task already owns
    // the flash and a round trip through this seam would have been the task
    // asking itself. `crate::ota::tick_self_test` carries what that saved.
    // -----------------------------------------------------------------------
    /// Start writing the inactive slot, for an image of this many bytes.
    #[cfg(feature = "http")]
    OtaBegin {
        /// The request's `Content-Length`.
        declared: u32,
    },
    /// The page channel holds this many bytes; check them and write them.
    #[cfg(feature = "http")]
    OtaPage {
        /// How much of the lent page is the image's.
        len: u16,
    },
    /// Every byte has arrived. Finish the checks and mark the slot bootable.
    #[cfg(feature = "http")]
    OtaFinish,
    /// Give up on an upload, leaving the running image untouched.
    #[cfg(feature = "http")]
    OtaAbort,

    // -----------------------------------------------------------------------
    // Backup and restore
    //
    // The same shape as the four above and for the same reason — the flash has
    // one owner and it is the state task — with one difference worth naming:
    // **an export travels *back* through this seam**, which the update path
    // never needed. [`Reply::BackupChunk`] is therefore the one reply variant
    // that carries bulk, and it is sixty-four bytes because that is what sizes
    // the `Signal` static every reply shares. `crate::restore::EXPORT_CHUNK_BYTES`
    // carries that arithmetic and the structural reason for the same figure.
    //
    // A *restore* travels the other way and carries nothing here: its pages go
    // through `crate::ota`'s zero-copy channel, because an upload is an upload
    // and a second channel would be a second page buffer out of the DRAM the
    // Wi-Fi driver's heap is carved from.
    // -----------------------------------------------------------------------
    /// The next sixty-four bytes of this device's own backup, from `at`.
    ///
    /// Must be asked for in order: the checksum is accumulated as the bytes go
    /// past, so a gap would make it quietly wrong — and a backup whose checksum
    /// is wrong is one that is refused on the way back in with nothing saying
    /// why.
    #[cfg(feature = "http")]
    BackupChunk {
        /// How far into the container.
        at: u32,
    },
    /// Start staging a backup of this many bytes.
    #[cfg(feature = "http")]
    RestoreBegin {
        /// The request's `Content-Length`.
        declared: u32,
    },
    /// The page channel holds this many bytes; write them.
    #[cfg(feature = "http")]
    RestorePage {
        /// How much of the lent page is the file's.
        len: u16,
    },
    /// Every byte has arrived. Mark the restore staged for the next boot.
    #[cfg(feature = "http")]
    RestoreFinish,
    /// Give up on an upload, leaving nothing staged.
    #[cfg(feature = "http")]
    RestoreAbort,
}

/// What the state task answers.
///
/// Each read variant carries `Option` rather than a separate "not found",
/// because an empty registry slot and a shade are the same question asked of
/// the same array — and the walk above needs "nothing here" to mean "keep
/// going" rather than "fail".
#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "read by the web server; the state task produces every variant \
              either way"
)]
pub enum Reply {
    /// A shade, or nothing at or after the slot asked for.
    Shade(Option<ShadeDto>),
    /// A group, or nothing at or after the slot asked for.
    Group(Option<GroupDto>),
    /// A room, or nothing at or after the slot asked for.
    Room(Option<RoomDto>),
    /// A shade was created, and this is the id it was given.
    Created(ShadeId),
    /// It was done, and there is nothing to say about it.
    Done,
    /// It was refused, in the vocabulary the UI translates — with the settings
    /// field it is about, when it is about one.
    Refused(ApiErrorDto),
    /// What the device is provisioned with. The trial half of
    /// [`somfy_api::SettingsDto`] is not here: it belongs to `crate::trial`,
    /// which the web server reads directly, because it is not in flash and this
    /// task has no business knowing about the radio.
    ///
    /// Gated for the same measured reason as [`Request::Settings`].
    #[cfg(feature = "http")]
    Settings(Option<WifiSettingsDto>, Option<MqttSettingsDto>),
    /// A candidate credential, validated and with its passphrase resolved.
    ///
    /// **Carries a passphrase**, and it is the one reply here that does. It has
    /// to: the web server hands it to `crate::trial`, which hands it to the
    /// Wi-Fi task, which puts it on the radio — the same passphrase the driver
    /// already holds. What it does not do is reach a socket; nothing in
    /// [`somfy_api::SettingsDto`] has a field it could be written into.
    #[cfg(feature = "http")]
    WifiCandidate(WifiCredentials),
    /// Bytes of a backup, and how many of them are real.
    ///
    /// A short chunk is the last one; `len == 0` is the end. **The only reply
    /// here that carries bulk**, and the reason [`Rpc`]'s reply `Signal` is
    /// sixty-four bytes wider than it would otherwise be — see
    /// [`Request::BackupChunk`].
    #[cfg(feature = "http")]
    BackupChunk {
        /// How many of [`bytes`](Reply::BackupChunk::bytes) are the file's.
        len: u8,
        /// The bytes.
        bytes: [u8; crate::restore::EXPORT_CHUNK_BYTES],
    },
}

/// The one seam, as a single static.
///
/// Its producer is the web server, so a build without the `http` feature
/// compiles all of this and calls none of it — the same honest state the edit
/// channel was in before there was anything to produce an edit. Kept
/// unconditional because the seam belongs to the *state task*, which offers it;
/// a `#[cfg]` here would be transport knowledge inside the core.
///
/// A struct rather than three loose statics so that the invariant tying them
/// together — the slot is only touched by the state task while a gate-holding
/// caller is parked — has somewhere to be written down.
#[allow(
    dead_code,
    reason = "the producer is the web server; a build without `http` answers no \
              requests and this is the seam it would answer them on"
)]
pub struct Rpc {
    /// Serialises callers, FIFO. Held across the whole exchange.
    gate: FairSemaphore<Mutex, GATE_WAITERS>,
    /// Raised by a caller, awaited by the state task.
    request: Signal<Mutex, Request>,
    /// Raised by the state task, awaited by the caller.
    reply: Signal<Mutex, Reply>,
}

/// Callers the gate can queue.
///
/// One per thing that can be inside [`Rpc::call`] at once, which is one per
/// connection task. Stated here rather than read from `crate::api::HTTP_TASKS`
/// because this module is compiled whether or not there is a web server — the
/// seam belongs to the state task, which offers it — and `crate::api` asserts
/// that its own pool fits, so the two cannot drift without the build saying so.
pub const GATE_WAITERS: usize = 8;

/// The seam itself.
pub static RPC: Rpc = Rpc::new();

impl Rpc {
    const fn new() -> Rpc {
        Rpc {
            gate: FairSemaphore::new(1),
            request: Signal::new(),
            reply: Signal::new(),
        }
    }

    /// Ask the state task something, and wait for the answer.
    ///
    #[allow(dead_code, reason = "the caller is the web server, which `http` gates")]
    /// `None` means the state task did not answer inside [`REPLY_TIMEOUT_S`],
    /// which cannot happen without a fault — see that constant. The caller
    /// turns it into a `503`.
    pub async fn call(&'static self, request: Request) -> Option<Reply> {
        // Held for the whole exchange, which is what makes the signals below a
        // rendezvous rather than a race. Released by `Drop` on every path,
        // including the timeout below and a caller whose future is dropped.
        let _held = match self.gate.acquire(1).await {
            Ok(held) => held,
            Err(_) => {
                // Unreachable: the queue is as deep as the number of tasks that
                // can ask. Reported rather than `expect`ed, because a panic
                // here reboots the board over one request.
                crate::logln!("api: the request queue is full, which should be impossible");
                return None;
            }
        };
        // **Before signalling, not after.** A previous caller whose future was
        // dropped between signalling and waiting — an HTTP task whose socket
        // died — leaves an answer nobody read. Clearing it here means this
        // caller cannot mistake that answer for its own; clearing it after
        // would race the state task.
        self.reply.reset();
        self.request.signal(request);
        match with_timeout(Duration::from_secs(REPLY_TIMEOUT_S), self.reply.wait()).await {
            Ok(reply) => Some(reply),
            Err(_) => {
                crate::logln!(
                    "api: the state task did not answer in {}s — reporting the request \
                     unavailable rather than waiting for it",
                    REPLY_TIMEOUT_S,
                );
                None
            }
        }
    }

    /// The state task's end: wait for something to answer.
    pub async fn next(&'static self) -> Request {
        self.request.wait().await
    }

    /// The state task's end: answer it.
    ///
    /// Infallible and non-blocking by construction — `Signal::signal` overwrites
    /// rather than parks — so no reply can make the state task wait on an HTTP
    /// client. That is the property that keeps the web server unable to affect
    /// radio control, and it is the reason this is a `Signal` rather than a
    /// channel.
    pub fn answer(&'static self, reply: Reply) {
        self.reply.signal(reply);
    }
}
