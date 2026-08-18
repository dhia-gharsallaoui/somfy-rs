//! What the device knows about itself, and what a restore did to it.
//!
//! Design spec §7.2 promises a `system` resource and §8 promises the two
//! screens that read it: diagnostics (log buffer, last panic) and
//! backup/restore. This module is the contract for both.
//!
//! # Why a diagnostics screen is worth its bytes
//!
//! Every hard failure this project has had was diagnosed over a serial cable —
//! a stack-guard panic that boot-looped the board, a `StackTooSmall` refusal, a
//! Wi-Fi association that never completed. A user has no cable. The most
//! valuable thing this resource can do is turn "it stopped working" into a
//! sentence somebody can act on or paste into an issue, so it carries
//! **numbers beside the claims** rather than a health verdict: the stack
//! figures the boot line prints, the heap's high-water mark, and how much of
//! the log ring the boot filled.
//!
//! There is deliberately no `healthy: bool` anywhere here. This device cannot
//! know whether it is healthy; it can only say what it measured.
//!
//! # What is not JSON, and why
//!
//! **The log itself is `text/plain`, streamed** — `GET /api/v1/system/log`.
//! It is lines, it is up to `firmware::diag::RING_BYTES` of them, and a JSON
//! string would have to escape every one of those bytes into a buffer this
//! device pays for four times over in Wi-Fi driver headroom (see
//! `firmware::api::routes`'s `JsonBody`). A `<pre>` is what the UI does with it
//! either way.
//!
//! **A backup is `application/octet-stream`, streamed** —
//! `GET /api/v1/system/backup`. It is flash records, byte for byte, and the
//! whole point of the format is that the decoder that reads them back is the
//! same one the boot path already uses.
//!
//! # Secrets
//!
//! Nothing here carries one, by the same structural rule
//! [`crate::settings`] states: no outbound type in this crate has a field a
//! passphrase or a broker password could be written into. That rule is what
//! makes an unauthenticated `GET` an actuation risk rather than a
//! credential-disclosure one, and **an export that carried secrets would undo
//! it in one line** — `GET /api/v1/system/backup` is exactly the "read the
//! passphrase out over the LAN" the settings module exists to prevent. So the
//! backup carries the shade table, the estate and the rolling codes, and it
//! carries the *names* of the network settings so a person can retype them.
//! See [`BackupContentsDto`].

use serde::{Deserialize, Serialize};

/// Longest firmware version string carried on the wire.
///
/// Semver plus room for a pre-release tag. It is `CARGO_PKG_VERSION` of the
/// firmware crate, so it is whatever the manifest says and nothing chooses it
/// at run time.
pub const MAX_VERSION_LEN: usize = 16;

/// Longest host name carried on the wire.
///
/// `somfy-` plus a hex MAC, which is what `firmware::identity` derives and the
/// only name this device answers to. Stated here rather than imported because
/// this crate compiles on the host and knows nothing about eFuses.
pub const MAX_HOST_LEN: usize = 18;

/// Longest panic text this device keeps.
///
/// **160 bytes, and the figure is a division rather than a preference.** The
/// text is escaped into [`SYSTEM_JSON_MAX_BYTES`], which is a buffer held
/// across a response write inside each of the web server's connection task
/// futures — four copies, out of the DRAM the Wi-Fi driver's heap is carved
/// from. A wider record is paid for on every boot including the boots where
/// nobody opens the screen.
///
/// 160 is enough for `panicked at src/main.rs:1234:9:` and a sentence after it,
/// which is what a Rust panic message is. **And it is not the only copy**: the
/// full message goes to the serial line and into the log ring unabridged. What
/// this record buys over the ring is *durability* — the ring wraps, and months
/// later this still says what the last panic was.
pub const MAX_PANIC_TEXT_LEN: usize = 160;

/// Longest JSON a [`SystemDto`] serialises to, in bytes.
///
/// **Measured, not counted** — `tests/system.rs` builds the widest legal value
/// and asserts this bound from both sides, never under it and never more than
/// 128 bytes over. That is the discipline [`crate::SHADE_JSON_MAX_BYTES`] and
/// [`crate::SETTINGS_JSON_MAX_BYTES`] are held to, and it exists because the
/// first of those was hand-counted and was wrong by 160 bytes.
///
/// The panic text is the term that dominates it, and it grows by **exactly one
/// byte per byte**: `firmware::diag::push_sanitised` admits only
/// `0x20..=0x7E` and substitutes `.` for the two printable characters JSON
/// escapes, `"` and `\`, so there is nothing left for JSON to lengthen. Without
/// that substitution the worst case would be six bytes per byte — a control
/// character escapes to `\u00XX` — and this constant would be 1,760.
///
/// **The substitution and this figure have to move together.** `tests/system.rs`
/// asserts the bound against a text built from the widest character the
/// firmware admits, so relaxing the firmware's rule without widening this leaves
/// the test passing while the device overruns the buffer.
///
/// The measurement is **627**; this is that rounded up to the next 128, which is
/// the granularity the test's own ceiling check uses. An ordinary document — the
/// one a real board answers with, with no panic recorded — is 246.
///
/// **It was 896 before the substitution above admitted only escape-free
/// characters**, and the 256 bytes that change gave back are spent four times
/// over: it is a kilobyte of ESP32-C3 Wi-Fi heap, on the chip whose margin over
/// the measured announcement peak is the tightest in the matrix.
pub const SYSTEM_JSON_MAX_BYTES: usize = 640;

/// Longest JSON a [`RestoreReportDto`] serialises to, in bytes.
///
/// Measured the same way and by the same test: **337**, rounded up to the next
/// 128.
pub const RESTORE_JSON_MAX_BYTES: usize = 384;

// ---------------------------------------------------------------------------
// The device
// ---------------------------------------------------------------------------

/// Which part this firmware was built for.
///
/// On the wire so the update screen can offer the right binary: this project
/// publishes one image per chip and writing the wrong one produces a board that
/// takes the update, reboots and does not come back —
/// [`crate::ApiErrorCode::ImageForAnotherChip`] is the device catching that
/// after the fact, and this is what lets the UI avoid it beforehand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum ChipDto {
    /// ESP32-S3 — Xtensa, the author's own board.
    Esp32S3,
    /// ESP32-C3 — RISC-V, reached by IP rather than by name and with no wall
    /// clock. See `firmware::heap` for what it refuses and why.
    Esp32C3,
}

/// Why the device started.
///
/// **Coarser than the silicon's own reset reason, and deliberately.** The
/// ESP32-S3 and ESP32-C3 spell the same causes with different variant names —
/// `CpuSw` against `Cpu0Sw`, `CpuMwdt0` against `Cpu0Mwdt0` — so a faithful
/// mirror would be two enums and a `#[cfg]` on the wire. What a person needs is
/// which of six things happened, and every one of those six has an action
/// behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum ResetReasonDto {
    /// The board was powered on, or its reset button was pressed.
    ///
    /// **This is also what a brownout looks like** on these parts: ESP-IDF maps
    /// 0x01 to power-on, brownout and super-watchdog alike, and esp-hal's own
    /// documentation says so. A board that reports this without anybody having
    /// touched it is a board with a supply problem.
    PowerOn,
    /// The firmware reset itself.
    ///
    /// The ordinary cause is a settings change, a firmware upload or a staged
    /// restore, each of which restarts on purpose. It is **also** what a panic
    /// looks like from here, because this firmware's panic handler resets
    /// rather than halting — so [`SystemDto::last_panic`] is what tells the two
    /// apart, and it is the reason that field exists.
    Software,
    /// A watchdog fired: a task stopped feeding it, or the CPU stopped running.
    Watchdog,
    /// The supply voltage sagged far enough for the brownout detector to fire.
    Brownout,
    /// The USB serial or JTAG peripheral reset the core.
    ///
    /// Normal while a cable is attached — `espflash` does this to enter the
    /// bootloader — and meaningless otherwise.
    Debugger,
    /// Something else, or nothing the chip reported.
    ///
    /// Deep sleep, a clock or power glitch, an eFuse CRC failure, or a reset
    /// register this build does not model. It is one variant rather than six
    /// because none of them has a different action.
    Other,
}

/// The stack figures the boot line prints, as numbers.
///
/// **The same three the serial console shows** — `firmware::heap` derives the
/// requirement from measured call chains and the boot refuses to start below
/// it. They are here so that a person with no cable can read the same
/// arithmetic, and because the one thing this project has been bitten by three
/// times is this row going stale: a claim with a measurement beside it is what
/// catches that, and a screen is where somebody will notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct StackDto {
    /// What the linker left, in bytes: `_stack_start_cpu0 - _stack_end_cpu0`.
    pub available: u32,
    /// What the deepest chain in this build needs, plus what an interrupt adds
    /// to it. `firmware::heap::REQUIRED_STACK_BYTES`.
    pub required: u32,
    /// How deep this boot actually went, read back off a painted stack.
    ///
    /// `null` until the paint has been scanned, which happens once the
    /// controller is running. It is the measurement that keeps
    /// [`required`](StackDto::required) honest.
    pub used: Option<u32>,
}

/// The heap figures, as numbers.
///
/// The heap exists for the Wi-Fi driver and nothing else in this firmware
/// allocates, so [`peak`](HeapDto::peak) is a measurement of somebody else's
/// code — which is precisely why it is worth showing. A board that reboots a
/// few seconds into every boot with a peak close to its size is out of heap,
/// and that is indistinguishable from a bad access point until somebody sees
/// these two numbers next to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct HeapDto {
    /// The whole heap, in bytes. A compile-time division of this chip's DRAM.
    pub size: u32,
    /// What is allocated right now.
    pub used: u32,
    /// The high-water mark since boot, which is the figure that matters.
    pub peak: u32,
}

/// How full the log ring is.
///
/// Every field here exists to answer one question — *is the ring big enough?* —
/// which is a question this project answers by measuring rather than by
/// choosing. [`dropped`](LogDto::dropped) is the whole point: a non-zero value
/// on a board that has just booted means the boot output does not fit, and the
/// lines a diagnostics screen most wants are the first ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct LogDto {
    /// Bytes the ring holds, in total. A compile-time constant.
    pub capacity: u32,
    /// Bytes of it in use.
    pub bytes: u32,
    /// Lines in it.
    pub lines: u32,
    /// Lines evicted since the ring was last empty, because a newer line needed
    /// the room. Non-zero means the oldest output has been lost.
    pub dropped: u32,
}

/// What the device recorded about the last time it panicked.
///
/// **This survives the reset that follows a panic**, which is the whole reason
/// it can exist: `firmware`'s panic handler resets rather than halting, so
/// anything to be shown afterwards has to live somewhere the reset does not
/// clear. `firmware::diag` puts it in RTC-fast memory, which esp-hal preserves
/// across a software reset and zeroes on a power-on.
///
/// **A power cut therefore erases it.** That is a real limit and it is stated
/// rather than worked around: RTC memory is the only thing on these parts that
/// survives a reset without a flash write, and a flash write from a panic
/// handler would be an erase and a program on a device that has just been
/// established to be in an unknown state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct PanicDto {
    /// What the panic said, as `core::panic::PanicInfo` renders it: the source
    /// location and the message.
    ///
    /// Stored as printable ASCII with everything else substituted — see
    /// [`MAX_PANIC_TEXT_LEN`] — and truncated to that many bytes. The full text
    /// is in the log ring and on the serial line.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub text: heapless::String<MAX_PANIC_TEXT_LEN>,
    /// Whether [`text`](PanicDto::text) is the whole message or its first
    /// [`MAX_PANIC_TEXT_LEN`] bytes.
    pub truncated: bool,
    /// How long the board had been running when it panicked, in seconds.
    ///
    /// A panic seconds into every boot is a boot loop; a panic after four days
    /// is something else entirely, and no other field distinguishes them.
    pub uptime_s: u32,
    /// How many times the board has started since. Zero means this boot is the
    /// one the panic caused.
    ///
    /// It counts resets rather than time because a power cut clears the record
    /// entirely, so anything this can report happened without one.
    pub boots_since: u32,
}

/// Everything the diagnostics screen reads, in one response.
///
/// One document rather than five endpoints because they are read together and
/// polled together, and because the interesting facts are the *relations*
/// between them — a heap peak next to a heap size, a stack depth next to a
/// stack requirement, a panic next to an uptime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct SystemDto {
    /// The part this image was built for.
    pub chip: ChipDto,
    /// The firmware version, from the crate manifest.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub firmware: heapless::String<MAX_VERSION_LEN>,
    /// The name this device answers to, derived from its MAC.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub host: heapless::String<MAX_HOST_LEN>,
    /// How long it has been running, in seconds.
    ///
    /// Seconds and not milliseconds because a `u32` of milliseconds wraps after
    /// forty-nine days and this device is meant to run for months — a wrapped
    /// uptime is worse than a coarse one, because it reads as a reboot that did
    /// not happen.
    pub uptime_s: u32,
    /// Why it started.
    pub reset_reason: ResetReasonDto,
    /// The stack figures the boot line prints.
    pub stack: StackDto,
    /// The heap figures.
    pub heap: HeapDto,
    /// How full the log ring is.
    pub log: LogDto,
    /// The last panic, if the device has one recorded. `null` on a board that
    /// has been power-cycled since, which is most of them.
    pub last_panic: Option<PanicDto>,
}

// ---------------------------------------------------------------------------
// Backup and restore
// ---------------------------------------------------------------------------

/// Which kind of file a staged restore turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum BackupFormatDto {
    /// This firmware's own export: an `RTSB` container holding the flash
    /// records verbatim.
    SomfyRs,
    /// A configuration backup exported by a C++ ESPSomfy-RTS controller.
    EspSomfyRts,
}

/// What happened to the last backup that was uploaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub enum RestoreOutcomeDto {
    /// Nothing has been uploaded, or the staging region has been cleared.
    None,
    /// A backup is staged and will be applied on the next boot.
    ///
    /// A client sees this between the `202` and the restart. It is not a
    /// success: nothing has been read, nothing has been validated and nothing
    /// has been written.
    Staged,
    /// The staged backup was read, every record was accepted, and the shade
    /// table, the estate and the rolling codes were written.
    Applied,
    /// The staged backup was refused, and **nothing was written**.
    ///
    /// [`RestoreReportDto::error`] carries the code and
    /// [`RestoreReportDto::row`] the record it came from. The device is still
    /// running whatever it was running before the upload.
    Refused,
}

/// The non-secret half of a backup's contents, as the device read them back.
///
/// This is what a restore **cannot** put back and a person therefore has to
/// retype: the Wi-Fi passphrase and the broker password are not in the file at
/// all. What is here is everything needed to know *which* passphrase and
/// *which* password — the network's name, the broker's address — so the
/// retyping is a lookup rather than a guess.
///
/// It is `null` for a C++ backup, which carries neither: that format keeps
/// network credentials in NVS rather than in the file, and `somfy-migrate`'s
/// own documentation says an import can recover *where* to publish and never
/// *as whom*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct BackupContentsDto {
    /// The network the exporting device was joined to, if it had one.
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub ssid: Option<heapless::String<32>>,
    /// Whether that credential had a passphrase. `true` means one has to be
    /// retyped; `false` means the network is open and nothing is missing.
    pub psk_was_set: bool,
    /// The broker the exporting device published to, as a dotted quad, if it
    /// had one.
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub broker: Option<heapless::String<21>>,
    /// Whether that broker connection had a password.
    pub broker_password_was_set: bool,
}

/// What the last upload to `POST /api/v1/system/restore` did.
///
/// # Why this is read after a restart rather than answered by the upload
///
/// A restore is **staged, not applied**: the upload streams to a flash region
/// and the device restarts, and the boot path applies it. Three reasons, and
/// the first is the one that makes it not a choice.
///
/// **It does not fit anywhere else.** A C++ backup is about twelve kilobytes
/// and has to be parsed whole; `somfy_migrate::parse_backup` takes one
/// contiguous slice, and the decoded `MigrationData` is another five and a
/// half. On the ESP32-S3 the Wi-Fi driver's heap clears its own measured peak
/// by about eleven kilobytes and the state task's stack chain has under a
/// kilobyte of headroom before a compile-time assertion fires, so there is no
/// static, no future and no task stack with room for it. At **boot** there is:
/// the main stack is sixty-six kilobytes and nothing has been spawned yet.
///
/// **The boot path already knows how to do this.** It reads the same four flash
/// regions, seeds rolling codes through the same `somfy_store::seed_if_absent`,
/// and announces the result to Home Assistant. Applying a restore to a running
/// controller would mean a second code path that tears down a registry the
/// state task owns and a broker session mid-flight.
///
/// **There is precedent in this firmware for exactly this shape.** A firmware
/// upload answers `202` and restarts; a broker settings change answers `202`
/// and restarts, because the retained topics of the superseded namespaces must
/// be cleared by the boot path that already does it correctly.
///
/// What it costs is that a refusal arrives after a reboot rather than in the
/// response. The cheap half is not deferred — an upload that is not a backup at
/// all, or is too large, or does not checksum, is refused immediately with a
/// code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(
        export,
        export_to = "../../../ui/src/api/generated/",
        rename_all = "camelCase"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReportDto {
    /// What happened.
    pub outcome: RestoreOutcomeDto,
    /// Which format the file turned out to be. `null` when nothing is staged,
    /// or when the file was refused before it could be identified.
    pub format: Option<BackupFormatDto>,
    /// Shades applied, or that would be applied.
    pub shades: u8,
    /// Rooms applied.
    pub rooms: u8,
    /// Groups applied.
    pub groups: u8,
    /// Records that were accepted with a caveat — an unknown shade kind
    /// defaulted to a roller, a group whose rolling code the old controller
    /// never wrote to its backup, a member naming a shade that no longer
    /// exists.
    ///
    /// A count and not a list, deliberately: every one of them is written to
    /// the log as its own line, with the record and the reason, and
    /// `GET /api/v1/system/log` is where a person reads them. Carrying them
    /// here would be a second, narrower vocabulary for the same facts and a
    /// buffer this device pays for four times.
    pub warnings: u8,
    /// The refusal, when [`outcome`](RestoreReportDto::outcome) is
    /// [`Refused`](RestoreOutcomeDto::Refused).
    ///
    /// It is an ordinary [`crate::ApiErrorDto`] carrying an ordinary
    /// [`crate::ApiErrorCode`], because a backup is refused by the same rules a
    /// hand-typed shade is: a name over thirty-two bytes is
    /// [`NameTooLong`](crate::ApiErrorCode::NameTooLong) whether it was typed
    /// or imported.
    pub error: Option<crate::ApiErrorDto>,
    /// Which record in the file the refusal came from, counting shades from
    /// zero. `null` when the refusal is about the file rather than a record in
    /// it.
    pub row: Option<u8>,
    /// The non-secret settings the file carried, so a person knows what to
    /// retype. `null` unless a `somfy-rs` backup has been read.
    pub contents: Option<BackupContentsDto>,
}

impl RestoreReportDto {
    /// The report of a device with nothing staged, which is almost every
    /// device almost all of the time.
    pub const fn nothing() -> RestoreReportDto {
        RestoreReportDto {
            outcome: RestoreOutcomeDto::None,
            format: None,
            shades: 0,
            rooms: 0,
            groups: 0,
            warnings: 0,
            error: None,
            row: None,
            contents: None,
        }
    }
}
