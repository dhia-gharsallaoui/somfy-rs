//! The only heap in this firmware, and the reasons for its size.
//!
//! Every crate in this workspace is allocation-free, and the six carrying the
//! protocol, the domain, the RMT rendering, the CC1101 driver, the store and
//! the task bodies still are — CI proves it by building each of them for
//! `thumbv7em-none-eabihf`, a target with no allocator at all. **The heap
//! exists for `esp-radio` and for what `esp-radio` drags in with it**, and it
//! is confined here so that the claim stays checkable.
//!
//! Three things need it, and nothing else in the firmware does:
//!
//! - **The Wi-Fi driver's packet buffers**, which the closed blob allocates
//!   and frees as frames arrive and leave.
//! - **The driver's task stacks.** `esp-radio` creates its threads through
//!   `esp-rtos`, which allocates their stacks from here.
//! - **`esp-rtos`'s own main-task bookkeeping**, allocated by `esp_rtos::start`
//!   — which is why even `tx-check`, a bring-up binary with no network at all,
//!   installs [`install_scheduler_only`].
//!
//! ## Heap exhaustion mid-frame
//!
//! There is no garbage collector here and so no pause to worry about, but the
//! analogous failure is real: an allocation that fails while a frame is being
//! built or decoded. It cannot happen on this firmware's frame path, and the
//! reason is structural rather than careful — `somfy-rts`, `somfy-rmt`,
//! `somfy-cc1101`, `somfy-store`, `somfy-domain` and `somfy-tasks` contain no
//! allocation to fail, which is exactly what the bare-metal builds in CI check.
//!
//! What *can* exhaust the heap is Wi-Fi traffic — and it is worth being exact
//! about what that costs, because the obvious answer is wrong. `esp-alloc`
//! returns null on failure, which reaches `handle_alloc_error`, which panics.
//! So an exhausted heap is not "Wi-Fi degrades"; it is a panic, and the panic
//! handler is what decides whether the radio survives it. `main`'s does, by
//! resetting rather than halting — see the argument on it. That is the
//! mechanism that turns heap exhaustion back into a recoverable event, and it
//! is why the margin below is watched on every boot rather than assumed.
//!
//! ## It is taken out of the same DRAM as the main stack
//!
//! `esp_alloc::heap_allocator!` declares a static array, and esp-hal's linker
//! script gives the main stack whatever DRAM is left once the statics are
//! placed. So every byte here is a byte `main::check_stack_headroom` no longer
//! has — and that check is what turns an over-large heap into a refusal to
//! boot carrying a number, instead of into a corrupted pulse train.

/// Bytes of heap installed for the Wi-Fi driver.
///
/// **Two measurements bracket this number, and neither of them is a
/// derivation.** `docs/provenance.md` records both.
///
/// ### The floor: what the driver actually uses
///
/// **46,660 bytes**, the high-water mark printed by [`report`] on an ESP32-S3
/// with the Wi-Fi driver initialised, the station configured and association
/// attempts in progress (2026-08-16). Not a figure reasoned to — a figure read
/// off the serial line.
///
/// Reasoning to one was tried and abandoned as fiction. `esp-radio`'s
/// documented budget at `ControllerConfig::default()` is 10 static RX buffers
/// of about 1.6 KB each — 16 KB, held from Wi-Fi init to deinit — plus *up to*
/// 32 dynamic RX and 32 dynamic TX buffers sized by the frame and freed once
/// the stack has taken them, plus the driver's task stacks. Only the 16 KB is
/// a working set; the rest is a cap, and taking the cap literally gives about
/// 120 KB, which is more DRAM than the smaller chips in this matrix have at
/// all.
///
/// ### The ceiling: what the tightest chip can give
///
/// **The ESP32-S2 sets it.** Its usable `dram_seg` is 184 KB against the
/// ESP32-S3's, and `esp-radio`'s statics take most of that, so linking the
/// controller with a 32 KB heap leaves these main stacks (read from
/// `_stack_start_cpu0 - _stack_end_cpu0` in each ELF, 2026-08-16):
///
/// | chip | stack at a 32 KB heap |
/// |---|---|
/// | ESP32 | 113,340 |
/// | ESP32-S2 | **41,028** |
/// | ESP32-S3 | 218,692 |
/// | ESP32-C3 | 205,440 |
///
/// Every byte added here comes off those figures one for one. A 96 KB heap —
/// which is what `esp-radio`'s own example implies, and what this was first
/// written as — does not link for the ESP32-S2 at all: the linker reports
/// `cannot move location counter backwards`, the statics having overrun the
/// segment by 28,980 bytes.
///
/// ### The choice
///
/// 56 KB is the largest single figure that leaves the ESP32-S2 **16,324
/// bytes** of main stack — just under twice `main::REQUIRED_STACK_BYTES`, and well
/// clear of the ~6.5 KB `RmtTx::transmit_frame` needs — while staying 10,684
/// bytes (23%) above the measured high-water mark. One constant rather than
/// four, because a per-chip heap would mean the chip nobody can test is the
/// one running a configuration nobody has measured.
///
/// ### What is still unmeasured
///
/// The mark above was taken with association **failing**, so it covers driver
/// init, the task stacks and scan traffic, and not the dynamic RX/TX buffers a
/// working link fills. [`report`] prints the mark again every time the network
/// comes up for exactly that reason. **Plan 5 Task 3 must read it under real
/// MQTT traffic and revisit this number**; if it turns out the working set
/// exceeds what the ESP32-S2 can give, the honest outcome is to say the
/// ESP32-S2 is a compile target rather than to quietly under-size every chip.
#[allow(
    dead_code,
    reason = "not used by `tx-check`, which includes this file by path and \
              installs only the scheduler's heap"
)]
pub const RADIO_HEAP_BYTES: usize = 56 * 1024;

/// Bytes of heap for a binary that starts the scheduler and no radio.
///
/// `esp_rtos::start` allocates its main-task bookkeeping, so a binary that
/// calls it needs *a* heap even with no network anywhere in it. This is that
/// heap and nothing more: a few hundred bytes of task record, rounded up to a
/// figure that will not need revisiting, and small enough that `tx-check`'s
/// stack headroom is indistinguishable from what it was before.
pub const SCHEDULER_HEAP_BYTES: usize = 4 * 1024;

/// Install the full heap. For the controller, which runs a radio.
#[allow(dead_code, reason = "see the allow on `RADIO_HEAP_BYTES`")]
pub fn install_for_radio() {
    // A statement rather than an expression: the macro declares the backing
    // static in place.
    esp_alloc::heap_allocator!(size: RADIO_HEAP_BYTES);
}

/// Install the scheduler's heap only. For bring-up binaries with no network.
#[allow(
    dead_code,
    reason = "used by tx-check, which includes this file by path"
)]
pub fn install_scheduler_only() {
    esp_alloc::heap_allocator!(size: SCHEDULER_HEAP_BYTES);
}

/// Print the heap's size, its current use and its high-water mark.
///
/// The high-water mark is the number [`RADIO_HEAP_BYTES`] is chosen from, so
/// this is the measurement rather than decoration: a boot that prints it is a
/// boot that can be quoted. Nowhere near the frame path.
#[allow(
    dead_code,
    reason = "not called by every binary that includes this file"
)]
pub fn report(when: &str) {
    let stats = esp_alloc::HEAP.stats();
    esp_println::println!(
        "heap: {} — {} of {} bytes used, peak {}",
        when,
        stats.current_usage,
        stats.size,
        stats.max_usage,
    );
}
