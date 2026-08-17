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
//! - **Whatever the ROM allocates through `esp-rtos`'s syscall table.**
//!   `esp_rtos::start` installs `__getreent`, `_malloc_r` and `_free_r` into
//!   it (`esp-rtos-0.3.0/src/syscall.rs::setup_syscalls`), so a ROM routine
//!   that allocates allocates *here*. That is why even `tx-check`, a bring-up
//!   binary with no network at all, installs [`install_scheduler_only`] —
//!   **not** because `esp_rtos::start` allocates, which it measurably does
//!   not. See [`SCHEDULER_HEAP_BYTES`], which used to say otherwise.
//!
//! ## An idle link is not an idle allocator
//!
//! [`report`] prints a figure that does not move, and that stillness is
//! misleading about how hard this heap is worked. Measured on an ESP32-S3
//! against a real access point and a real broker on 2026-08-17, with nothing
//! to do but hold the link and publish five diagnostics a minute:
//! `total_allocated` climbed by **about 30,000 bytes a second**, indefinitely,
//! while `current_usage` never changed by a byte. The Wi-Fi driver allocates
//! and frees a buffer per frame — beacons, ARP, the AP's own chatter — and the
//! net effect is exactly zero, which is why the steady figure looks calm.
//!
//! It is worth naming because of what each of those allocations costs.
//! `esp-alloc` takes a critical section per allocation and per free, and
//! `internal-heap-stats` (which this crate enables, and which is what makes
//! [`peak_bytes`] exist at all) adds counter updates inside it. So an
//! associated link with no application traffic masks interrupts several
//! hundred times a second. `crate::net` argues carefully about `esp_println`
//! holding a critical section for the length of one log line; this is the same
//! cost, unbudgeted, at a far higher rate, and the asymmetry between the two
//! arguments was an accident rather than a judgement.
//!
//! It has not been observed to matter — a reception is timestamped by the RMT
//! peripheral and a transmission is clocked out of RMT RAM, so neither is
//! stretched by a masked interrupt (see `crate::net`'s fourth structural
//! claim) — and it is recorded here so that the next person to time the frame
//! path knows the floor is not quiet.
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
/// **55,040 bytes**, the worst high-water mark seen across fourteen boots of an
/// ESP32-S3 on a real access point with a real broker (2026-08-17). Not a
/// figure reasoned to — a figure read off the serial line.
///
/// It replaces the 46,660 bytes this note carried from 2026-08-16, which were
/// taken with association *failing* and which the note itself flagged as owing
/// a re-measurement under real MQTT traffic. **That obligation is discharged,
/// and the answer is that the old figure understated the peak by 8,380 bytes.**
/// What the traffic actually looks like:
///
/// | | bytes |
/// |---|---|
/// | steady use, held for as long as the boot lasts | 47,464 |
/// | free at rest | 9,880 |
/// | worst peak in fourteen boots | 55,040 |
/// | free at that peak | 2,304 |
///
/// Three things about that table are worth more than the numbers.
///
/// **The steady figure does not move within a boot.** Sampled every five
/// seconds over runs of seven minutes: 47,464 at every one of them, unchanged
/// across association cycles and across the diagnostics tick. It settles on a
/// slightly different value from boot to boot — 47,416 on one run in seven —
/// and then holds it. So "the heap is stable" is a claim about a running
/// device, not about a constant.
///
/// **There is no leak, and that is checked rather than assumed.** With
/// `internal-heap-stats`' running totals printed alongside the usage,
/// `total_allocated - total_freed` equalled `current_usage` **exactly**, at
/// every sample, through 6.4 MB of allocation. A leak of one byte a minute
/// would show in that subtraction; nothing does.
///
/// **The peak is transient and it is the session announcement.** It is reached
/// within a second of the broker's CONNACK — the burst of retained discovery
/// configs — and never again: [`report`] at "network up", a moment before,
/// reads about 49,000. So the honest picture is 9,880 bytes free at rest with a
/// momentary trough of 2,304, not a heap that sits 96% full.
///
/// The peak is also **noisy**, which the single measurement it replaces could
/// not have shown. Fourteen boots of one unchanged image spanned 50,824 to
/// 55,040 — a 4,216-byte spread — depending on nothing more than how many of
/// the driver's dynamic RX and TX buffers happened to be in flight at once. Any
/// future comparison of two heap configurations has to survive that spread
/// before it means anything; see the AMPDU note below, which did not.
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
/// controller leaves these main stacks. **How to re-read them**, because every
/// figure below has already drifted once: build the binary for each target and
/// subtract two linker symbols out of the ELF —
///
/// ```text
/// cargo build --release --features chip-s2 --target xtensa-esp32s2-none-elf --bin firmware
/// nm target/xtensa-esp32s2-none-elf/release/firmware | grep _stack_.*_cpu0
/// ```
///
/// — and the stack is `_stack_start_cpu0 - _stack_end_cpu0`. It is the same
/// subtraction `main::check_stack_headroom` does at boot, so the ESP32-S3 row
/// can be checked against the `stack:` line on the serial console; on
/// 2026-08-17 both said 176,388.
///
/// | chip | stack at this crate's 56 KB heap | at a 32 KB heap |
/// |---|---|---|
/// | ESP32 | 71,036 | 95,612 |
/// | ESP32-S2 | **14,588** | 39,164 |
/// | ESP32-S3 | 176,388 | 200,964 |
/// | ESP32-C3 | 163,144 | 187,720 |
///
/// Measured 2026-08-17. The 32 KB column this replaces was measured on
/// 2026-08-16 and read ESP32 113,340, ESP32-S2 41,028, ESP32-S3 218,692,
/// ESP32-C3 205,440 — stale by 17,728 bytes on the three chips that build the
/// broker session and by 1,864 on the ESP32-S2, which does not. The difference
/// is Task 3 onward: a table of figures like this goes wrong silently every
/// time a static is added, so it carries the command to regenerate it rather
/// than only the result.
///
/// Every byte added here comes off those figures one for one. A 96 KB heap —
/// which is what `esp-radio`'s own example implies, and what this was first
/// written as — does not link for the ESP32-S2 at all: the linker reports
/// `cannot move location counter backwards`, the statics having overrun the
/// segment by 28,980 bytes.
///
/// ### The choice
///
/// 56 KB is the largest single figure that leaves the ESP32-S2 **14,588
/// bytes** of main stack — 1.78 times `main::REQUIRED_STACK_BYTES`, and well
/// clear of the ~6.5 KB `RmtTx::transmit_frame` needs — while staying 2,304
/// bytes above the worst peak measured under real traffic. One constant rather
/// than four, because a per-chip heap would mean the chip nobody can test is
/// the one running a configuration nobody has measured.
///
/// **That margin is 4%, not the 23% this note used to claim**, and the change
/// is entirely in the measurement rather than in the constant: the old figure
/// compared 56 KB against a mark taken with association failing. Both halves
/// have since moved against each other — the peak up by 8,380 bytes and the
/// ESP32-S2's stack down by 1,736 — so the arithmetic is worth re-doing rather
/// than trusting, and the two commands above are what re-does it.
///
/// **What that costs, said plainly.** 2,304 bytes of headroom against a peak
/// that varies by 4,216 bytes between boots is not a comfortable margin: it
/// says a session announcement roughly twice as expensive as the worst one
/// observed would exhaust this heap. The consequence is bounded rather than
/// catastrophic — an exhausted heap is a panic, and `main`'s handler resets
/// rather than halting, so the device reboots and rejoins (see "Heap exhaustion
/// mid-frame" above) — but "bounded" means a reboot loop at the moment the
/// broker is announced, which is a plausible way for this device to become
/// useless while looking merely flaky.
///
/// Growing the heap is **not** the answer and the ESP32-S2 is why: every byte
/// added comes off its 14,588, and it boots into `StackTooSmall` below 8,192.
/// The margin is what it is. Two things follow. The peak is published over MQTT
/// precisely so it can be watched across many more than fourteen boots, and the
/// announcement burst — not the steady link — is where to look first if it is
/// ever seen to climb.
///
/// ### What frame aggregation is worth here, since it was measured
///
/// `esp-radio`'s `ControllerConfig::default()` enables AMPDU in both directions
/// with a six-frame Block-Ack window, and Block-Ack reorder state was the
/// leading suspect for the transient peak above. **It is not.** Fourteen boots
/// with aggregation on against seventeen with it off, same image otherwise,
/// same access point and broker (2026-08-17):
///
/// | | AMPDU on | AMPDU off |
/// |---|---|---|
/// | steady use | 47,464 | 47,148 |
/// | worst peak | 55,040 | 54,676 |
/// | best peak | 50,824 | 50,556 |
/// | allocator churn | ~29,800 B/s | ~27,500 B/s |
///
/// Turning it off is worth **316 bytes of steady use** — a real and repeatable
/// saving, visible on every boot — and **364 bytes of worst case**, which is
/// inside the 4,216-byte boot-to-boot spread and therefore not a result at all.
/// The transient peak survives with aggregation disabled, on most boots, which
/// is what rules Block-Ack state out as its cause.
///
/// So aggregation stays on, and the reason is the *cost* of turning it off
/// rather than the benefit: the `with_ampdu_*` setters are
/// `#[builder_lite(unstable)]`, which the derive expands to
/// `#[cfg(feature = "unstable")]`, so reaching them means enabling
/// `esp-radio/unstable` — a wider API surface and, on `chip-esp32` only, an
/// ADC2 claim in `esp_radio`'s `init` that panics if esp-hal holds it. That is
/// not a trade worth 316 bytes. Recorded so it is not investigated a third
/// time; `docs/provenance.md` carries the per-boot figures.
///
/// ### The ESP32-S2 answered the other half of that question
///
/// The note this replaces asked what to do if the working set did not fit the
/// ESP32-S2. It does not, and the shortfall is not in the heap: it is in what
/// is left of `dram_seg` after it. The broker session's task future is 14,816
/// bytes, the boot check needs 8,192 of stack, and at 56 KB the ESP32-S2 has
/// 14,588 bytes of DRAM after the statics — so the image does not link, by
/// **5,748 bytes**, before any stack is carved at all (`cannot move location
/// counter backwards (from 3ffdf674 to 3ffde000)`, 2026-08-17).
///
/// The answer taken is the one this note asked for: **say so.** `chip-s2`
/// builds without the broker session and prints that at boot; every other chip
/// is unaffected and the heap stays one constant. Shrinking it to fit would put
/// the figure below the 55,040-byte peak above, which trades a link error for a
/// heap-exhaustion panic under traffic that has now been measured rather than
/// under traffic nobody had seen. `crates/firmware/Cargo.toml`'s `mqtt` feature
/// carries the arithmetic.
#[allow(
    dead_code,
    reason = "not used by `tx-check`, which includes this file by path and \
              installs only the scheduler's heap"
)]
pub const RADIO_HEAP_BYTES: usize = 56 * 1024;

/// Bytes of heap for a binary that starts the scheduler and no radio.
///
/// **The reason this constant used to give was false, and the constant is kept
/// anyway.** Both halves of that are deliberate.
///
/// ### What it used to say, and why it is wrong
///
/// It said `esp_rtos::start` allocates its main-task bookkeeping, so a binary
/// that calls it needs *a* heap. Measured on an ESP32-S3 on 2026-08-17, with
/// [`report`] called at three points in `main::start`: the heap is at **0 of
/// 57,344 bytes used, peak 0** immediately after `esp_rtos::start`, still 0
/// after both radio tasks are spawned, and still 0 on entry to `net::start`.
/// The first byte is taken by `esp_radio::wifi::new`, which `tx-check` never
/// calls.
///
/// The source says the same thing. `esp_rtos::start` ends in
/// `task::allocate_main_task`, which — despite the name — allocates nothing: it
/// writes into the fields of `SCHEDULER.per_cpu[cpu].main_task`, part of a
/// static, and adopts the existing main stack as a slice of
/// `_stack_end_cpu0 .. _stack_start_cpu0`
/// (`esp-rtos-0.3.0/src/lib.rs:321-334`). `Task::drop` frees a stack only when
/// `heap_allocated` is set, and the only thing that sets it is
/// `SchedulerState::create_task` (`src/scheduler.rs:181`), which the main task
/// does not go through.
///
/// ### What it is actually for
///
/// `esp_rtos::start` also calls `syscall::setup_syscalls`, which installs
/// `__getreent`, `_malloc_r` and `_free_r` into the ROM syscall table
/// (`esp-rtos-0.3.0/src/syscall.rs:111`). Those route ROM-side allocation into
/// **this** heap, lazily: `_getreent` allocates a `_reent` for a task the first
/// time ROM code asks one of it. So the true statement is not "`start`
/// allocates" but "`start` makes the ROM able to allocate here", and a binary
/// that started the scheduler with no heap installed would answer the first
/// such call with a null, which reaches `handle_alloc_error`, which panics.
///
/// ### Why 4 KiB and not less
///
/// Because the binary that needs it is this module's one untestable
/// consumer. `tx-check` keys the transmitter — it is the only image here that
/// puts a frame on the air by itself — so "run it and read the high-water
/// mark", which is how every other figure in this file was arrived at, is not
/// available. With the premise corrected the honest options were to shrink this
/// on an argument or to keep it on one, and 4 KiB is kept: it is a reserve
/// against a path that has not fired rather than a measured requirement, it is
/// 7% of what the controller installs, and `tx-check`'s stack headroom is
/// indistinguishable from what it would be at zero.
///
/// **What would make this measurable:** a binary that starts the scheduler,
/// drives the same peripherals `tx-check` drives, and never keys the
/// transmitter. `store-check` and `config-check` are the closest things to it
/// and neither qualifies — they start no scheduler and install no heap — so
/// this stays a reserve until one exists.
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

/// Bytes of heap not currently allocated.
///
/// Read from the same `esp-alloc` counters [`report`] prints, so a figure
/// published over MQTT and a figure read off the serial line are the same
/// measurement rather than two that might disagree.
#[allow(
    dead_code,
    reason = "read only by the broker session, which the ESP32-S2 build omits"
)]
pub fn free_bytes() -> usize {
    let stats = esp_alloc::HEAP.stats();
    stats.size.saturating_sub(stats.current_usage)
}

/// The largest the heap has been since boot.
///
/// **This is the figure [`RADIO_HEAP_BYTES`] is chosen from**, and it is worth
/// publishing rather than only printing, for two reasons the measurement behind
/// that constant established. It is reached within a second of the broker's
/// CONNACK and never moves again, so catching it on a serial cable means
/// catching one second of a boot; and it varies by four kilobytes from boot to
/// boot, so one reading is a sample rather than the answer. Over MQTT it
/// becomes something an operator watches across days and across reboots, which
/// is the only shape in which this number means much.
#[allow(
    dead_code,
    reason = "read only by the broker session, which the ESP32-S2 build omits"
)]
pub fn peak_bytes() -> usize {
    esp_alloc::HEAP.stats().max_usage
}
