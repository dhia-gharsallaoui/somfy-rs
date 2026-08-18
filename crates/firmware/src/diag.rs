//! What the device can say about itself after the fact: the log ring and the
//! last panic.
//!
//! # The problem this exists to solve
//!
//! Every hard failure this project has had was diagnosed over a serial cable —
//! a stack-guard panic that boot-looped the board, a `StackTooSmall` refusal, a
//! Wi-Fi association that never completed and looked like a dead device. A user
//! has no cable, and the difference between "it stopped working" and a sentence
//! they can act on is entirely a matter of whether anything survived to be
//! read.
//!
//! # Where it lives, and why that is not DRAM
//!
//! **RTC-fast memory**, through esp-hal's `#[ram(rtc_fast, persistent)]`. Two
//! properties make it the only sensible place, and they are both load-bearing:
//!
//! 1. **It is not DRAM.** The linker gives it its own 8 KiB region — `readelf`
//!    shows `.rtc_fast.persistent` at `0x600fe000` on the ESP32-S3 and
//!    `0x50000000` on the ESP32-C3, both outside `dram_seg`. So none of it comes
//!    out of [`crate::heap::DRAM_FOR_STACK_AND_HEAP`], which is the quantity the
//!    Wi-Fi driver's heap is a *subtraction* from. A 4 KiB ring in a `static`
//!    would have cost the ESP32-C3 4 KiB of a heap that clears its measured
//!    announcement peak by about five, which is most of the margin. Here it
//!    costs nothing.
//! 2. **It survives a software reset.** esp-hal zeroes `.rtc_fast.persistent`
//!    only when `reset_reason()` is `ChipPowerOn` or unknown
//!    (`esp_hal::soc::__init_persistent`), so a reset caused by the panic
//!    handler preserves it. That is the whole reason a panic can be reported at
//!    all: [`crate::panic`] resets rather than halting, deliberately, so
//!    anything to be shown afterwards has to be somewhere the reset does not
//!    clear.
//!
//! **A power cut erases both.** That is the honest limit and it is not worked
//! around: the alternative is a flash erase and program from inside a panic
//! handler, on a device that has just been established to be in an unknown
//! state, with `esp-storage`'s ROM routines and their interrupts-disabled
//! window. A lost record is recoverable — the failure will recur — and a
//! rolling-code region damaged by a write from a panicking board is not.
//!
//! # What `#[ram(persistent)]` does not promise
//!
//! esp-hal's own documentation says a system-level or lesser reset landing
//! before the zeroing "could skip initialization and start the application with
//! the static filled with random bytes", and recommends a checksum. Both
//! records carry one, over a header small enough that maintaining it costs
//! nothing: the ring's is over its five header words and the panic record's is
//! over its header and its text. A record that does not check out is discarded
//! rather than shown, which is the only safe direction — a diagnostics screen
//! displaying invented text is worse than one saying nothing.
//!
//! # What a log line costs, next to what one already cost
//!
//! `esp_println::println!` expands to `writeln!(Printer, …)`, and
//! `Printer::write_bytes` takes a **critical section per format fragment** —
//! `esp-println-0.13.1/src/lib.rs:88`, `with(|token| …)` inside `write_bytes`,
//! called once per `write_str`. So a line with four `{}` in it already disables
//! interrupts five or six times, each of them spinning on the UART FIFO.
//! [`crate::net`]'s module docs put the cost of one line at most of the ~5 ms
//! available to re-arm the receiver between a frame and its repeat.
//!
//! [`emit`] adds **one** more critical section to that, and it holds no I/O:
//! the line is formatted straight into the ring in RTC RAM, which is a `memcpy`
//! of a few tens of bytes. Against the UART's milliseconds that is noise, and
//! it is why this module does not attempt to be cleverer — a design that
//! formatted once into a stack buffer would have to put that buffer on the
//! caller's frame, and the deepest chain in this image has under a kilobyte of
//! headroom before [`crate::heap`]'s compile-time assertion fires.
//!
//! `format_args!` is evaluated **once** and the same `Arguments` is handed to
//! both consumers, so an argument with a side effect cannot run twice. The
//! formatting machinery does run twice, which is CPU and not stack.
//!
//! # Re-entering a critical section from a panic handler is sound here
//!
//! [`record_panic`] takes a critical section, and the panic it is reporting may
//! have happened inside one. That is safe with esp-hal's `critical_section`
//! implementation specifically: it is backed by `esp_sync`, whose `release`
//! checks `token.is_reentry()` and does nothing for a nested acquisition
//! (`esp-sync-0.2.1/src/lib.rs:262`). A nested `with` re-disables interrupts and
//! restores correctly rather than deadlocking. The existing panic handler
//! already depends on this, because `esp_println` takes one too.

// **`tx-check` includes this file by path** — it takes `logln!`, which every
// other module in that harness logs through — and reaches nothing else here. So
// in that binary the ring's readers, the panic record and everything built on
// them are genuinely unused, and rustc is right to say so.
//
// A module-wide allow rather than one per item, because the list is fifteen
// long and a per-item `reason` on each would say the same sentence fifteen
// times. It is the same situation `crate::heap` handles the same way, for the
// same binary.
#![allow(
    dead_code,
    reason = "tx-check includes this file by path and uses only the `logln!` macro"
)]

use core::fmt::Write as _;

use somfy_api::{LogDto, PanicDto, ResetReasonDto, MAX_PANIC_TEXT_LEN};

/// Bytes of log the ring holds.
///
/// **4,096, and the figure is a measurement waiting to happen rather than a
/// choice defended in advance.** What it has to hold is the output of one boot
/// plus whatever came before a failure, and this firmware has over 130
/// `logln!` sites — how many of them a given board reaches depends on how many
/// shades it has, whether it has a broker, and whether the network came up.
/// Nothing available on a host can settle that.
///
/// So the ring **counts what it drops** ([`LogDto::dropped`]), the boot line
/// below reports it, and the diagnostics screen shows it. A board whose first
/// `GET /api/v1/system` reports a non-zero `dropped` is a board whose boot
/// output does not fit, and this constant is what to change. That is the same
/// discipline `esp-alloc`'s `internal-heap-stats` buys for the heap and that
/// `crate::stack_used` buys for the stack: a number standing next to the claim.
///
/// The ceiling is RTC-fast memory itself, 8 KiB shared with `.rtc_fast.text`
/// and with [`crate::ota`]'s attempt counter. [`RTC_FAST_BYTES`] and the
/// assertion under it are what keep this honest; the region is also the reason
/// the figure is not simply "as much as possible", since an esp-hal release
/// that placed something of its own there would otherwise turn a dependency
/// bump into a link failure at the end of a long build.
pub const RING_BYTES: usize = 4_096;

/// RTC-fast memory on both chips in the matrix.
///
/// Read off the linker scripts rather than a datasheet:
/// `esp-hal-1.1.2/ld/esp32s3/memory.x` gives `rtc_fast_seg` `len = 8k` and
/// `ld/esp32c3/memory.x` gives `RTC_FAST` `LENGTH = 0x2000`. Both chips alias
/// `RTC_FAST_RWTEXT` and `RTC_FAST_RWDATA` onto the same region
/// (`ld/*/linkall.x`), so code placed there would come out of this too — this
/// firmware places none.
const RTC_FAST_BYTES: usize = 8 * 1024;

/// What this module places in RTC-fast memory, in bytes.
///
/// The ring and its header and the panic record. It does not include
/// [`crate::ota`]'s four-byte attempt counter, which lives in the same section
/// and is why the reserve below is not spent to the byte.
const RTC_FAST_USED_BYTES: usize = core::mem::size_of::<Ring>() + core::mem::size_of::<Panic>();

/// RTC-fast memory this module will not take.
///
/// **A policy figure, and said so rather than dressed up.** What it stands
/// behind is placements this crate does not control: `.rtc_fast.text` is empty
/// in this image today and `.rtc_fast.persistent` holds only [`crate::ota`]'s
/// four-byte attempt counter, but an esp-hal release that put a wake stub or a
/// driver's state there would otherwise turn a dependency bump into a linker
/// message about a region overflowing — at the end of a long build, naming
/// neither this module nor the figure to change.
///
/// 2 KiB is a quarter of the region and comfortably more than any single
/// placement esp-hal makes there today, which is zero.
const RTC_FAST_RESERVE_BYTES: usize = 2 * 1024;

// **The one thing here that can stop a build**, and it is a real comparison
// rather than one satisfied by construction: [`RING_BYTES`] is chosen and
// [`RTC_FAST_BYTES`] is read off a linker script.
//
// Today it holds with 1,836 bytes to spare beyond the reserve, which is the
// room [`RING_BYTES`] has to grow into if the `dropped` counter says it must.
const _: () = assert!(
    RTC_FAST_USED_BYTES + RTC_FAST_RESERVE_BYTES <= RTC_FAST_BYTES,
    "the log ring and the panic record no longer fit RTC-fast memory with its \
     reserve intact: see diag::RING_BYTES for the figure to lower, \
     diag::RTC_FAST_RESERVE_BYTES for what is deliberately left free, and \
     diag::RTC_FAST_BYTES for what the linker script gives",
);

/// The checksum, table-free.
///
/// `CRC_32_ISO_HDLC` because that is what all four flash records already use, so
/// a reader meets one algorithm in this codebase rather than two. `NoTable`
/// because these records are tens of bytes and a second 1 KiB lookup table in
/// the image would buy nothing measurable — `somfy_store::Record` already
/// carries the tabled one for the 256-byte records it checks on every commit.
const CRC: crc::Crc<u32, crc::NoTable> = crc::Crc::<u32, crc::NoTable>::new(&crc::CRC_32_ISO_HDLC);

/// Marks a ring that this build wrote.
///
/// Changed whenever the layout changes, so a record left by an older image is
/// discarded rather than misread — the same job the `RTSC`/`RTSW`/`RTSS`/`RTSE`
/// magics do in flash, and the same little-endian spelling.
const RING_MAGIC: u32 = u32::from_le_bytes(*b"RTSL");

/// Marks a panic record this build wrote. See [`RING_MAGIC`].
const PANIC_MAGIC: u32 = u32::from_le_bytes(*b"RTSP");

// ---------------------------------------------------------------------------
// The records
// ---------------------------------------------------------------------------

/// The log ring, as it sits in RTC-fast memory.
///
/// `repr(C)` so the layout is the one written down rather than one rustc chose,
/// which matters for exactly the reason it matters in flash: a hex dump of this
/// region should be readable against this file.
#[repr(C)]
struct Ring {
    /// [`RING_MAGIC`] when this build wrote it.
    magic: u32,
    /// Index of the oldest byte in [`bytes`](Ring::bytes).
    head: u32,
    /// Bytes in use, at most [`RING_BYTES`].
    len: u32,
    /// Complete lines held.
    lines: u32,
    /// Lines evicted to make room, since the ring was last empty.
    dropped: u32,
    /// CRC over the five words above.
    ///
    /// **Not over the text.** Maintaining a checksum over four kilobytes on
    /// every log line would make a log line cost more than the UART write it
    /// accompanies, and the text is not what a bad header would corrupt into
    /// something dangerous — an out-of-range `head` or `len` is. Garbage text
    /// inside a valid frame renders as garbage text, which is what a log
    /// carrying a failure looks like anyway.
    crc: u32,
    /// The lines, each terminated by `\n`.
    bytes: [u8; RING_BYTES],
}

/// The last panic, as it sits in RTC-fast memory.
#[repr(C)]
struct Panic {
    /// [`PANIC_MAGIC`] when this build wrote it.
    magic: u32,
    /// Uptime when the panic happened, in seconds.
    uptime_s: u32,
    /// Boots since. Incremented once per boot by [`boot`], so a fresh record
    /// reads zero.
    boots_since: u32,
    /// Bytes of [`text`](Panic::text) in use.
    len: u32,
    /// Whether the message was longer than [`MAX_PANIC_TEXT_LEN`].
    truncated: u32,
    /// CRC over the five words above and over `text[..len]`.
    ///
    /// Over the text as well as the header here, unlike the ring's, because
    /// this record is written **once** — a checksum over 160 bytes costs
    /// nothing on a path taken once per panic, and the text is the whole value
    /// of the record.
    crc: u32,
    /// The message, as printable ASCII. See [`push_sanitised`].
    text: [u8; MAX_PANIC_TEXT_LEN],
}

// SAFETY: esp-hal requires three things of a `#[ram(persistent)]` type and both
// records satisfy all three by construction. Each is **inhabited** — a `struct`
// with no uninhabited field. Each is **valid for every bit pattern of its
// backing memory**, because every field is a `u32` or a `[u8; N]` and both admit
// any bits; that is exactly the property a reset landing mid-write needs, and it
// is why the validity of a record is decided by a checksum at read time rather
// than by the type. And each **contains only `Persistable` fields and padding**:
// `u32` and `[u8; N]` are the only field types here, both of which esp-hal
// implements the trait for, and `repr(C)` with a `u32`-aligned tail leaves no
// padding to reason about.
unsafe impl esp_hal::Persistable for Ring {}
// SAFETY: as `Ring` above.
unsafe impl esp_hal::Persistable for Panic {}

// `#[ram(persistent)]` is a NOLOAD section: these initialisers are what the
// *compiler* sees, and the hardware never applies them except on a power-on
// reset, where esp-hal zero-fills the section instead. The values below are
// therefore only meaningful as "what a zero-filled region decodes to", which is
// a magic of zero — not [`RING_MAGIC`] — and so an empty ring.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut LOG: Ring = Ring {
    magic: 0,
    head: 0,
    len: 0,
    lines: 0,
    dropped: 0,
    crc: 0,
    bytes: [0; RING_BYTES],
};

/// See [`LOG`].
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut PANIC: Panic = Panic {
    magic: 0,
    uptime_s: 0,
    boots_since: 0,
    len: 0,
    truncated: 0,
    crc: 0,
    text: [0; MAX_PANIC_TEXT_LEN],
};

// ---------------------------------------------------------------------------
// Reading and writing the ring
// ---------------------------------------------------------------------------

/// Whether the ring's header checks out.
///
/// # Safety
///
/// The caller holds a critical section, which on this single-core firmware is
/// what makes it the only accessor.
unsafe fn ring_valid() -> bool {
    let ring = &raw const LOG;
    // SAFETY: `ring` points at a live static of the right type and the caller
    // holds the only access. Volatile because the static is in a NOLOAD section
    // whose declared initialiser is not what the hardware contains, and a
    // non-volatile read is a read the optimiser may answer from that
    // initialiser.
    unsafe {
        if core::ptr::read_volatile(&raw const (*ring).magic) != RING_MAGIC {
            return false;
        }
        let head = core::ptr::read_volatile(&raw const (*ring).head);
        let len = core::ptr::read_volatile(&raw const (*ring).len);
        if head as usize >= RING_BYTES || len as usize > RING_BYTES {
            return false;
        }
        core::ptr::read_volatile(&raw const (*ring).crc) == ring_crc()
    }
}

/// The checksum the ring's header should carry.
///
/// # Safety
///
/// As [`ring_valid`].
unsafe fn ring_crc() -> u32 {
    let ring = &raw const LOG;
    let mut digest = CRC.digest();
    // SAFETY: as `ring_valid`.
    unsafe {
        for word in [
            core::ptr::read_volatile(&raw const (*ring).magic),
            core::ptr::read_volatile(&raw const (*ring).head),
            core::ptr::read_volatile(&raw const (*ring).len),
            core::ptr::read_volatile(&raw const (*ring).lines),
            core::ptr::read_volatile(&raw const (*ring).dropped),
        ] {
            digest.update(&word.to_le_bytes());
        }
    }
    digest.finalize()
}

/// Start again with an empty ring.
///
/// # Safety
///
/// As [`ring_valid`].
unsafe fn ring_reset() {
    let ring = &raw mut LOG;
    // SAFETY: as `ring_valid`.
    unsafe {
        core::ptr::write_volatile(&raw mut (*ring).magic, RING_MAGIC);
        core::ptr::write_volatile(&raw mut (*ring).head, 0);
        core::ptr::write_volatile(&raw mut (*ring).len, 0);
        core::ptr::write_volatile(&raw mut (*ring).lines, 0);
        core::ptr::write_volatile(&raw mut (*ring).dropped, 0);
        core::ptr::write_volatile(&raw mut (*ring).crc, ring_crc());
    }
}

/// Drop the oldest line, whole.
///
/// **Whole lines and never part of one**, which is what stops the log ever
/// starting mid-sentence — and, since a line is valid UTF-8 by construction,
/// what stops the `text/plain` response ever starting mid-character.
///
/// # Safety
///
/// As [`ring_valid`].
unsafe fn drop_oldest_line() {
    let ring = &raw mut LOG;
    // SAFETY: as `ring_valid`.
    unsafe {
        let mut head = core::ptr::read_volatile(&raw const (*ring).head) as usize;
        let mut len = core::ptr::read_volatile(&raw const (*ring).len) as usize;
        while len > 0 {
            let byte = core::ptr::read_volatile((&raw const (*ring).bytes).cast::<u8>().add(head));
            head = (head + 1) % RING_BYTES;
            len -= 1;
            if byte == b'\n' {
                break;
            }
        }
        core::ptr::write_volatile(&raw mut (*ring).head, head as u32);
        core::ptr::write_volatile(&raw mut (*ring).len, len as u32);
        let lines = core::ptr::read_volatile(&raw const (*ring).lines);
        core::ptr::write_volatile(&raw mut (*ring).lines, lines.saturating_sub(1));
        let dropped = core::ptr::read_volatile(&raw const (*ring).dropped);
        core::ptr::write_volatile(&raw mut (*ring).dropped, dropped.saturating_add(1));
    }
}

/// Append one byte, evicting whole lines until it fits.
///
/// # Safety
///
/// As [`ring_valid`].
unsafe fn push_byte(byte: u8) {
    let ring = &raw mut LOG;
    // SAFETY: as `ring_valid`.
    unsafe {
        while core::ptr::read_volatile(&raw const (*ring).len) as usize >= RING_BYTES {
            drop_oldest_line();
        }
        let head = core::ptr::read_volatile(&raw const (*ring).head) as usize;
        let len = core::ptr::read_volatile(&raw const (*ring).len) as usize;
        let at = (head + len) % RING_BYTES;
        core::ptr::write_volatile((&raw mut (*ring).bytes).cast::<u8>().add(at), byte);
        core::ptr::write_volatile(&raw mut (*ring).len, (len + 1) as u32);
    }
}

/// Writes into the ring, one fragment at a time, as `core::fmt` produces them.
///
/// It cannot fail: a line that will not fit evicts older ones, which is what a
/// ring is for. `write_str` therefore always returns `Ok`, and the `Result`
/// exists only because the trait has one.
struct RingWriter;

impl core::fmt::Write for RingWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for &byte in text.as_bytes() {
            // A line is delimited by `\n`, so a newline *inside* a message
            // would split it into two entries — harmless for reading and
            // wrong for `lines`. There is exactly one source of them today
            // (`core::panic::PanicInfo`'s `Display`, which puts the message on
            // its own line) and it is the one place where the split is what a
            // reader wants anyway, so they are counted rather than escaped.
            // SAFETY: this function is only reached from `record`, which holds
            // a critical section for the whole of it.
            unsafe { push_byte(byte) };
        }
        Ok(())
    }
}

/// Put one line in the ring.
fn record(args: core::fmt::Arguments<'_>) {
    critical_section::with(|_| {
        // SAFETY: the critical section is what makes this the only accessor on
        // this single-core firmware, and every helper below documents the same
        // requirement.
        unsafe {
            if !ring_valid() {
                ring_reset();
            }
            let _ = RingWriter.write_fmt(args);
            push_byte(b'\n');
            let ring = &raw mut LOG;
            let lines = core::ptr::read_volatile(&raw const (*ring).lines);
            core::ptr::write_volatile(&raw mut (*ring).lines, lines.saturating_add(1));
            core::ptr::write_volatile(&raw mut (*ring).crc, ring_crc());
        }
    });
}

/// Print a line and keep it.
///
/// Called by [`crate::logln`], which is what the rest of the firmware uses.
/// The serial output is byte-for-byte what `esp_println::println!` produced
/// before this module existed — the boot lines that state a measurement beside
/// a claim are the model for what a log line is here, and routing them through
/// something lossier would have been the wrong trade.
pub fn emit(args: core::fmt::Arguments<'_>) {
    record(args);
    let mut printer = esp_println::Printer;
    let _ = printer.write_fmt(args);
    let _ = printer.write_str("\n");
}

// ---------------------------------------------------------------------------
// Reading it back
// ---------------------------------------------------------------------------

/// How full the ring is.
pub fn log_stats() -> LogDto {
    critical_section::with(|_| {
        // SAFETY: as `record`.
        unsafe {
            if !ring_valid() {
                return LogDto {
                    capacity: RING_BYTES as u32,
                    bytes: 0,
                    lines: 0,
                    dropped: 0,
                };
            }
            let ring = &raw const LOG;
            LogDto {
                capacity: RING_BYTES as u32,
                bytes: core::ptr::read_volatile(&raw const (*ring).len),
                lines: core::ptr::read_volatile(&raw const (*ring).lines),
                dropped: core::ptr::read_volatile(&raw const (*ring).dropped),
            }
        }
    })
}

/// Copy up to `out.len()` bytes of the log, starting `from` bytes in.
///
/// Returns how many were copied; zero means the end. The caller walks it in
/// chunks so that a critical section is held for a `memcpy` of a few hundred
/// bytes rather than for the length of a TCP write — see
/// [`crate::api::routes`]'s log route.
///
/// **A reader that starts a walk and a writer that appends during it do not
/// agree**, and that is deliberate rather than tolerated: bounding the write
/// side would mean a log line waiting on a socket, which is the one thing a
/// degradable service may never do. The visible effect is that a line may be
/// missed or repeated while the log is being read, on a device that is
/// logging heavily at the moment somebody opens the screen.
pub fn log_read(from: usize, out: &mut [u8]) -> usize {
    critical_section::with(|_| {
        // SAFETY: as `record`.
        unsafe {
            if !ring_valid() {
                return 0;
            }
            let ring = &raw const LOG;
            let head = core::ptr::read_volatile(&raw const (*ring).head) as usize;
            let len = core::ptr::read_volatile(&raw const (*ring).len) as usize;
            let available = len.saturating_sub(from);
            let take = available.min(out.len());
            for (index, slot) in out.iter_mut().take(take).enumerate() {
                let at = (head + from + index) % RING_BYTES;
                *slot = core::ptr::read_volatile((&raw const (*ring).bytes).cast::<u8>().add(at));
            }
            take
        }
    })
}

// ---------------------------------------------------------------------------
// The panic record
// ---------------------------------------------------------------------------

/// Append `text` to `record.text` from `at`, as characters JSON does not
/// lengthen.
///
/// Returns the new length. Anything outside `0x20..=0x7E`, and the two
/// printable characters JSON escapes — `\"` and `\\` — become `.`.
///
/// **The substitution is what sizes [`somfy_api::SYSTEM_JSON_MAX_BYTES`]**, and
/// that constant is a buffer held across a response write inside each of four
/// connection task futures, out of the DRAM the Wi-Fi driver's heap is a
/// subtraction from. Admitting control characters would make the worst case six
/// bytes per byte and admitting quotes two; refusing both makes it one, which
/// takes the bound from 1,760 to 640 and gives the ESP32-C3 back a kilobyte of
/// Wi-Fi heap it does not have to spare.
///
/// What it costs is that a panic message quoting a string — `assertion failed:
/// name == "x"` — reads with dots where the quotes were, and that an accented
/// shade name reads as dots. **The unabridged message is in the ring and on the
/// serial line**, so nothing is lost, only moved: this record is the durable
/// summary and the ring is the text.
///
/// # Safety
///
/// As [`ring_valid`], for [`PANIC`].
unsafe fn push_sanitised(at: usize, text: &str) -> usize {
    let record = &raw mut PANIC;
    let mut at = at;
    for &byte in text.as_bytes() {
        if at >= MAX_PANIC_TEXT_LEN {
            break;
        }
        let plain = match byte {
            b'"' | b'\\' => b'.',
            0x20..=0x7E => byte,
            _ => b'.',
        };
        // SAFETY: `at` is bounded by the check above, and the caller holds the
        // only access.
        unsafe { core::ptr::write_volatile((&raw mut (*record).text).cast::<u8>().add(at), plain) };
        at += 1;
    }
    at
}

/// Formats a panic message into the record, sanitising and truncating.
struct PanicWriter {
    at: usize,
    overflowed: bool,
}

impl core::fmt::Write for PanicWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        // SAFETY: only reached from `record_panic`, which holds a critical
        // section for the whole of it.
        let next = unsafe { push_sanitised(self.at, text) };
        // Fewer bytes taken than offered — including none at all — means the
        // record is full and the rest of the message is gone. One condition
        // rather than two, because "took zero of a non-empty fragment" is the
        // same fact as "took fewer than were offered".
        self.overflowed |= next - self.at < text.len();
        self.at = next;
        Ok(())
    }
}

/// Keep what a panic said, so the next boot can report it.
///
/// Called from [`crate::panic`] **before** the message reaches the serial line,
/// because the serial write is the slow half and a second fault during it would
/// otherwise lose the record as well as the output.
pub fn record_panic(info: &core::panic::PanicInfo<'_>) {
    let uptime_s = uptime_s();
    critical_section::with(|_| {
        // SAFETY: as `record`. Re-entering a critical section held by the code
        // that panicked is sound with esp-hal's implementation; see this
        // module's docs.
        unsafe {
            let record = &raw mut PANIC;
            let mut writer = PanicWriter {
                at: 0,
                overflowed: false,
            };
            let _ = writer.write_fmt(format_args!("{}", info));
            core::ptr::write_volatile(&raw mut (*record).magic, PANIC_MAGIC);
            core::ptr::write_volatile(&raw mut (*record).uptime_s, uptime_s);
            core::ptr::write_volatile(&raw mut (*record).boots_since, 0);
            core::ptr::write_volatile(&raw mut (*record).len, writer.at as u32);
            core::ptr::write_volatile(&raw mut (*record).truncated, u32::from(writer.overflowed));
            core::ptr::write_volatile(&raw mut (*record).crc, panic_crc());
        }
    });
}

/// The checksum the panic record should carry.
///
/// # Safety
///
/// As [`ring_valid`], for [`PANIC`].
unsafe fn panic_crc() -> u32 {
    let record = &raw const PANIC;
    let mut digest = CRC.digest();
    // SAFETY: as `ring_valid`.
    unsafe {
        let len = core::ptr::read_volatile(&raw const (*record).len);
        for word in [
            core::ptr::read_volatile(&raw const (*record).magic),
            core::ptr::read_volatile(&raw const (*record).uptime_s),
            core::ptr::read_volatile(&raw const (*record).boots_since),
            len,
            core::ptr::read_volatile(&raw const (*record).truncated),
        ] {
            digest.update(&word.to_le_bytes());
        }
        for index in 0..(len as usize).min(MAX_PANIC_TEXT_LEN) {
            let byte =
                core::ptr::read_volatile((&raw const (*record).text).cast::<u8>().add(index));
            digest.update(&[byte]);
        }
    }
    digest.finalize()
}

/// The last panic, if there is one that checks out.
pub fn last_panic() -> Option<PanicDto> {
    critical_section::with(|_| {
        // SAFETY: as `record`.
        unsafe {
            let record = &raw const PANIC;
            if core::ptr::read_volatile(&raw const (*record).magic) != PANIC_MAGIC {
                return None;
            }
            let len = core::ptr::read_volatile(&raw const (*record).len) as usize;
            if len > MAX_PANIC_TEXT_LEN {
                return None;
            }
            if core::ptr::read_volatile(&raw const (*record).crc) != panic_crc() {
                return None;
            }
            let mut text = heapless::String::new();
            for index in 0..len {
                let byte =
                    core::ptr::read_volatile((&raw const (*record).text).cast::<u8>().add(index));
                // Cannot fail: `push_sanitised` admits only printable ASCII,
                // and `len` is bounded by the capacity above.
                let _ = text.push(byte as char);
            }
            Some(PanicDto {
                text,
                truncated: core::ptr::read_volatile(&raw const (*record).truncated) != 0,
                uptime_s: core::ptr::read_volatile(&raw const (*record).uptime_s),
                boots_since: core::ptr::read_volatile(&raw const (*record).boots_since),
            })
        }
    })
}

/// Forget the last panic and empty the log.
///
/// `DELETE /api/v1/system` reaches this, once a person has read the record or
/// reported it. Without it the screen would show a months-old panic forever,
/// which trains an operator to ignore the one field on it that is always worth
/// reading.
///
/// **Both at once**, because they are one thing — what this device remembers
/// about its own past — and because the lines that led to a panic are of no use
/// once the panic they explain has been dismissed.
pub fn forget() {
    critical_section::with(|_| {
        // SAFETY: as `record`.
        unsafe {
            core::ptr::write_volatile(&raw mut PANIC.magic, 0);
            ring_reset();
        }
    });
}

/// Note that a boot has happened.
///
/// Called once from [`crate::start`]. Two jobs: it ages the panic record, so a
/// screen can say *how long ago*, and it puts a separator in the log so the
/// lines from before a reset are not read as this boot's.
pub fn boot(reason: ResetReasonDto) {
    critical_section::with(|_| {
        // SAFETY: as `record`.
        unsafe {
            let record = &raw mut PANIC;
            if core::ptr::read_volatile(&raw const (*record).magic) == PANIC_MAGIC {
                let boots = core::ptr::read_volatile(&raw const (*record).boots_since);
                core::ptr::write_volatile(&raw mut (*record).boots_since, boots.saturating_add(1));
                core::ptr::write_volatile(&raw mut (*record).crc, panic_crc());
            }
        }
    });
    let carried = critical_section::with(|_| {
        // SAFETY: as `record`.
        unsafe { ring_valid() && core::ptr::read_volatile(&raw const LOG.len) > 0 }
    });
    if carried {
        crate::logln!(
            "---- reset ({:?}) — everything above is from before it ----",
            reason
        );
    }
}

// ---------------------------------------------------------------------------
// The rest of what the screen reads
// ---------------------------------------------------------------------------

/// How long this board has been running, in seconds.
///
/// `esp_hal::time::Instant` rather than `embassy_time::Instant`, because this is
/// called from the panic handler and the embassy time driver is installed by
/// `esp_rtos::start`: a panic before that would be a panic inside a panic.
/// The hardware counter is running from `esp_hal::init`.
pub fn uptime_s() -> u32 {
    // Saturating rather than `as`: the counter wraps after more than seven
    // years, which is longer than the device, but a truncating cast would
    // report a fresh boot rather than a large number and that is the one
    // reading that would mislead.
    u32::try_from(
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_secs(),
    )
    .unwrap_or(u32::MAX)
}

/// Why this board started.
///
/// **Matched on the discriminant rather than the variant name**, because the
/// two chips spell the same causes differently — `CpuSw` on the ESP32-S3 against
/// `Cpu0Sw` on the ESP32-C3 — so a name-based match would need a `#[cfg]` per
/// arm. The numbers are ESP-IDF's own reset-reason codes and are identical
/// across both parts; `esp-hal-1.1.2/src/rtc_cntl/rtc/esp32s3.rs:15` and
/// `esp32c3.rs:185` are the two enums.
pub fn reset_reason() -> ResetReasonDto {
    let Some(reason) = esp_hal::system::reset_reason() else {
        return ResetReasonDto::Other;
    };
    match reason as u8 {
        // Power on. **Also brownout and super-watchdog**: ESP-IDF folds all
        // three onto 0x01 and esp-hal's own documentation says the distinction
        // is not expressible as a Rust enum. `ResetReasonDto::PowerOn` says so
        // in its own docs rather than claiming more than the silicon reports.
        0x01 => ResetReasonDto::PowerOn,
        // CoreSw and CpuSw / Cpu0Sw: the firmware reset itself. This is what a
        // panic looks like from here, which is why `last_panic` exists.
        0x03 | 0x0C => ResetReasonDto::Software,
        // Deep sleep. Nothing in this firmware sleeps, so it means an image
        // that did.
        0x05 => ResetReasonDto::Other,
        // The five watchdogs: main 0 and 1, RTC, and the super-watchdog.
        0x07 | 0x08 | 0x09 | 0x0B | 0x0D | 0x10 | 0x11 | 0x12 => ResetReasonDto::Watchdog,
        0x0F => ResetReasonDto::Brownout,
        // USB UART and USB JTAG: espflash entering the bootloader, and normal
        // while a cable is attached.
        0x15 | 0x16 => ResetReasonDto::Debugger,
        // Clock glitch, eFuse CRC, power glitch, and anything a future part
        // adds.
        _ => ResetReasonDto::Other,
    }
}

/// Print a line and keep it in the ring.
///
/// A drop-in replacement for `esp_println::println!` with identical serial
/// output — see [`emit`] for what it adds and what that costs. Every log site
/// in this firmware goes through it, so that the lines a diagnostics screen most
/// wants are the ones a serial console has always shown.
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        $crate::diag::emit(::core::format_args!($($arg)*))
    };
}
