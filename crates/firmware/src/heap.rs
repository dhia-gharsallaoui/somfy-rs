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
//! ## It is taken out of the same DRAM as the main stack, and it is per chip
//!
//! `esp_alloc::heap_allocator!` declares a static array, and esp-hal's linker
//! script gives the main stack whatever DRAM is left once the statics are
//! placed. So every byte here is a byte `main::check_stack_headroom` no longer
//! has — and that check is what turns an over-large heap into a refusal to
//! boot carrying a number, instead of into a corrupted pulse train.
//!
//! That trade is settled once, in one direction: [`STACK_BUDGET_BYTES`] fixes
//! what the stack keeps and [`RADIO_HEAP_BYTES`] takes the rest of whatever DRAM
//! the chip being built happens to have. **It is one rule over three inputs
//! rather than one number**, and the reason is that the chips are not the same
//! size: a single shared figure has to fit the smallest of them, which is how an
//! ESP32-S3 with 233,700 bytes to divide came to run its heap at 96% full next
//! to 176,356 bytes of stack it had no use for.
//!
//! The objection to per-chip figures is that it makes the chip nobody can test
//! the one running a configuration nobody has measured. That is true, and it was
//! equally true of the shared constant — no ESP32 and no ESP32-C3 had ever run
//! that either. What changes it is that the rule is now identical on every chip
//! and only its input differs, and that both quantities it is built from are
//! measurements rather than arguments: `main::REQUIRED_STACK_BYTES` against a
//! painted stack, and [`WIFI_WORKING_SET_BYTES`] against a live broker.

/// The main stack every chip is left, before the heap takes the rest.
///
/// **This is the design's one free variable, and everything else follows from
/// it.** `esp_alloc::heap_allocator!` declares a static array and esp-hal's
/// linker script gives the main stack whatever DRAM is left once the statics
/// are placed, so the heap and the stack are two shares of one fixed quantity —
/// measurably so, and checked rather than assumed: the heap moved by +4,096 on
/// the ESP32, +109,568 on the ESP32-S3 and +96,256 on the ESP32-C3 when this
/// constant was introduced, and each chip's stack fell by exactly that, to the
/// byte, in the relinked ELF. There is no third option and no slack between
/// them; choosing one chooses the other.
///
/// So this crate chooses the *stack*, once, for all chips, and
/// [`RADIO_HEAP_BYTES`] is the arithmetic that follows. The other order — pick a
/// heap and hope the stack survives — is what produced a 56 KB heap sized by the
/// smallest chip that was then in the matrix, and an ESP32-S3 running that heap
/// 96% full with 2,724 bytes to spare at the announcement peak.
///
/// ### The two terms
///
/// **49,592 bytes are required.** `main::REQUIRED_STACK_BYTES` derives that
/// frame by frame from the linked ELF and checks it on the ESP32-S3 against a
/// painted stack on real hardware, where the computed figure and the measured
/// one agree exactly. It is the boot path, not the frame path: the deepest thing
/// this firmware does is move the state task's 14 KB future into place,
/// underneath `start`'s own 13.7 KB frame, and `RmtTx::transmit_frame`'s
/// celebrated 6.5 KB never comes close.
///
/// **16,688 bytes are the margin, and it is measured rather than rounded.** What
/// the derivation cannot see is what an interrupt handler *calls*: those paths
/// run into `esp-radio`'s closed driver and into masked ROM, and neither carries
/// stack-size metadata, so no sum over them exists to be taken. The margin has
/// to cover being wrong about that, so it is set to **the largest single stack
/// frame emitted anywhere in any of the three images** — one more frame of the
/// worst size this compiler has actually produced here is the smallest unit in
/// which this call graph can grow.
///
/// That worst frame is **not** the same one on every chip, and getting this
/// wrong is easy: on the ESP32 and the ESP32-S3 the largest frame is
/// `UninitCell::write` at 14,320 bytes, but on the ESP32-C3 it is `entry`'s own
/// body — where `start` is inlined into it — at **16,688**. A margin of 14,320
/// would therefore have been smaller than one worst-case frame on one of the
/// three chips actually shipped, which is exactly the property the margin exists
/// to buy. The maximum is taken across all three:
///
/// ```text
/// RUSTFLAGS="-Zemit-stack-sizes -C link-arg=-Tlinkall.x" \
///   cargo build --release --features chip-c3 --target riscv32imc-unknown-none-elf --bin firmware
/// readelf -x .stack_sizes target/riscv32imc-unknown-none-elf/release/firmware
/// ```
///
/// — each entry in that section is a 4-byte address followed by a ULEB128 frame
/// size, and the largest of them is this figure. Read on 2026-08-17: ESP32
/// 14,320; ESP32-S3 14,320; ESP32-C3 16,688.
///
/// 49,592 + 16,688 = **66,280**, and the boot check fires at 49,592 — so the
/// margin is the distance between "this still works" and "this stops booting and
/// says why", rather than a number nobody would notice being spent.
pub const STACK_BUDGET_BYTES: usize = 49_592 + 16_688;

/// DRAM this chip has to divide between the main stack and the heap.
///
/// Measured, not looked up: build the chip, subtract the two linker symbols that
/// bound the stack, and add back whatever heap that build carried.
///
/// ```text
/// cargo build --release --features chip-s3 --target xtensa-esp32s3-none-elf --bin firmware
/// nm target/xtensa-esp32s3-none-elf/release/firmware | grep _stack_.*_cpu0
/// ```
///
/// `_stack_start_cpu0 - _stack_end_cpu0` is the stack, and it is the same
/// subtraction `main::check_stack_headroom` prints at boot, so these can be
/// cross-checked against a serial console rather than trusted. Read on
/// 2026-08-17 from release ELFs carrying a 57,344-byte heap, whose stacks were
/// ESP32 71,004, ESP32-S3 176,356 and ESP32-C3 163,112.
///
/// **This is the row that goes stale.** It moves every time a static is added
/// anywhere in the image, silently and in the direction that costs stack, which
/// is why the command that regenerates it sits next to it.
///
/// It **had** gone stale, by 3,520 bytes on every chip, and the symptom was
/// visible on a serial console for anyone who compared two numbers: the boot
/// line read `stack: 63796 bytes available` where these constants implied
/// 66,788. The margin was therefore 14,324 rather than the 16,688 the budget
/// above buys, and nothing was going to say so. Re-measured 2026-08-17 against
/// the images below.
///
/// # Measured per chip *and* per configuration
///
/// Each figure is taken with every feature that chip supports enabled, which is
/// the largest the statics get. A smaller build simply leaves the residue on
/// the stack, which is the safe direction and the same reason the division
/// rounds down.
///
/// | chip | features measured with | DRAM |
/// |---|---|---|
/// | ESP32 | `mqtt` — see [`crate::api`]; `http` does not fit | 125,116 |
/// | ESP32-S3 | `mqtt`, `ui` (and so `http`) | 159,908 |
/// | ESP32-C3 | `mqtt`, `ui` (and so `http`) | 146,672 |
///
/// The web server costs about **69,000 bytes on the ESP32, 70,000 on the
/// ESP32-S3 and 70,000 on the ESP32-C3** — three measurements of one thing, and the spread is
/// the per-architecture difference in what a generator lays out. Four connection
/// tasks are 52,384 of it (`api::HTTP_TASKS` × a 13,096-byte future, which is
/// `picoserve`'s router recursion) and their buffers are 14,336.
#[cfg(feature = "chip-esp32")]
const DRAM_FOR_STACK_AND_HEAP: usize = 125_116;
/// See the `chip-esp32` definition above.
#[cfg(feature = "chip-s3")]
const DRAM_FOR_STACK_AND_HEAP: usize = 159_908;
/// See the `chip-esp32` definition above.
#[cfg(feature = "chip-c3")]
const DRAM_FOR_STACK_AND_HEAP: usize = 146_672;

// **The ESP32 cannot carry the web server, and this says so at compile time
// rather than at link time.**
//
// Measured 2026-08-17: with `http` enabled the DRAM left for the stack and the
// heap falls to 54,556 bytes against a stack budget of 66,280 — the image does
// not link at all, and what `ld` says about it is `stack.x:11 cannot move
// location counter backwards`, which names neither the feature nor the chip.
//
// It is not close and it is not a tuning problem. Even at one connection task
// instead of four the server costs about 16,500 bytes, which would leave the
// Wi-Fi driver a heap of roughly 42 KB against a resting working set of 47,464
// — so the board would link and then fail to associate.
//
// The ESP32-S3 and the ESP32-C3 both have the DRAM for it, with 38,760 and
// 25,448 bytes of heap to spare over the worst peak yet measured — against the
// 3,944 the ESP32 has left with only a broker in it, which is itself inside
// this heap's known ~4,216-byte boot-to-boot noise and is the figure to watch
// on that chip.
#[cfg(all(feature = "chip-esp32", feature = "http"))]
compile_error!(
    "the ESP32 does not have the DRAM for the web server: with `http` enabled it has 54,556 \
     bytes for a stack budget of 66,280, so the image cannot link. Build it with \
     `--no-default-features --features chip-esp32,mqtt`, or use an ESP32-S3 or ESP32-C3. \
     See `heap::DRAM_FOR_STACK_AND_HEAP` for the measurement."
);

/// Bytes of heap installed for the Wi-Fi driver. **Per chip, from that chip's
/// own DRAM.**
///
/// One rule produces all three, and it is written here as arithmetic rather than
/// as three hand-computed constants so that it cannot drift from the prose:
///
/// > heap = [`DRAM_FOR_STACK_AND_HEAP`] − [`STACK_BUDGET_BYTES`], rounded
/// > **down** to a whole KiB.
///
/// The rounding goes down so the residue lands on the stack, which is the side
/// that fails silently — a heap that is a kilobyte small panics and says so,
/// while a stack that is a kilobyte small corrupts whatever it grows into.
///
/// | chip | DRAM to divide | heap | stack left | was, at 56 KB |
/// |---|---|---|---|---|
/// | ESP32 | 128,348 | 60 KiB = 61,440 | 66,908 | 57,344 heap / 71,004 stack |
/// | ESP32-S3 | 233,700 | **163 KiB = 166,912** | 66,788 | 57,344 / 176,356 |
/// | ESP32-C3 | 220,456 | 150 KiB = 153,600 | 66,856 | 57,344 / 163,112 |
///
/// Nothing in that table is chosen; it is what the rule returns.
///
/// ### What the ESP32-S3 gained, measured rather than asserted
///
/// The figure read off `heap: session announced`, which `crate::mqtt` prints one
/// line after the burst of retained discovery configs that *is* the peak — the
/// two older `report` call sites both run before that burst, so neither ever
/// showed the number this constant is chosen from:
///
/// | | 56 KB heap | 163 KiB heap |
/// |---|---|---|
/// | heap total | 57,344 | 166,912 |
/// | steady use | 47,464 – 47,608 | 47,464 – 47,608 |
/// | worst peak seen | 54,620 | 54,424 |
/// | **free at that peak** | **2,724** | **112,488** |
///
/// The steady figure does not move, which is the point: the driver's resting
/// working set was never the problem. The old margin was 2,724 bytes against a
/// peak that itself varied by about 2,000 bytes between boots of one unchanged
/// image — a margin inside its own noise, which is a coincidence with a good
/// track record rather than a design.
///
/// ### Three questions this retires
///
/// All three were open against the 56 KB heap and none survives the arithmetic,
/// which is the point of the exercise rather than a side effect:
///
/// - **The announcement burst.** It is the peak, it costs about 7,000 bytes over
///   the steady figure, and it now lands inside a heap with more than a hundred
///   kilobytes spare.
/// - **How many shades can be announced.** The burst scales with the shade count
///   and the old headroom did not cover doubling it. This one does.
/// - **Whether frame aggregation should be turned off.** It was measured at 316
///   bytes of steady use — a real, repeatable saving that was never worth what
///   reaching the setter costs (`esp-radio/unstable`, and on `chip-esp32` an ADC2
///   claim in `esp_radio`'s `init` that panics if esp-hal holds it). At 163 KiB
///   it is not worth discussing. Recorded so it is not investigated a fourth
///   time; `docs/provenance.md` carries the per-boot figures.
#[allow(
    dead_code,
    reason = "not used by `tx-check`, which includes this file by path and \
              installs only the scheduler's heap"
)]
pub const RADIO_HEAP_BYTES: usize = (DRAM_FOR_STACK_AND_HEAP - STACK_BUDGET_BYTES) / 1024 * 1024;

/// The Wi-Fi driver's measured resting working set, on an ESP32-S3.
///
/// Not a budget and not a peak — the figure `current_usage` settles on and then
/// holds for the life of a boot, 47,464 bytes at the bottom of its range across
/// sixteen boots against a real access point and a real broker (2026-08-17). It
/// is here so that [`warn_if_undersized`] has something to compare against that
/// was read off a serial line rather than argued for.
///
/// It is one chip's number used for all three, which is the honest limit of it:
/// the ESP32-C3 has never been measured, and the ESP32's driver is a different
/// blob again.
#[allow(dead_code, reason = "see the allow on `RADIO_HEAP_BYTES`")]
pub const WIFI_WORKING_SET_BYTES: usize = 47_464;

/// Say at boot when this chip's heap cannot hold the driver's working set.
///
/// **No chip in the matrix trips this today**, and it is here anyway, because
/// [`RADIO_HEAP_BYTES`] is now a *subtraction* rather than a number somebody
/// chose. Every static added to this image comes out of
/// [`DRAM_FOR_STACK_AND_HEAP`], and the day that constant is re-measured after a
/// Plan's worth of new buffers, the heap shrinks by exactly as much — quietly,
/// and with no diff to review, because the arithmetic is what changed rather
/// than the literal.
///
/// A heap that cannot fit the Wi-Fi driver does not degrade: `esp-alloc` returns
/// null, which reaches `handle_alloc_error`, which panics, and `main`'s handler
/// resets rather than halting. The visible symptom is a board that reboots a few
/// seconds into every boot, which is indistinguishable from a bad access point,
/// a failing supply or a broker that hangs up. One line naming the two numbers
/// turns a week of that into a sentence.
///
/// It is a run-time check rather than a `const` assertion so that a chip in this
/// state still **builds** — a compile-time refusal would take it out of the lint
/// and build matrix at the moment the matrix is what would catch the problem.
#[allow(dead_code, reason = "see the allow on `RADIO_HEAP_BYTES`")]
pub fn warn_if_undersized() {
    if RADIO_HEAP_BYTES < WIFI_WORKING_SET_BYTES {
        esp_println::println!(
            "heap: {} bytes is below the {} the Wi-Fi driver was measured to \
             hold at rest — this chip has too little DRAM for the radio and a \
             bootable stack at once, and association is expected to end in a \
             heap-exhaustion panic. See crates/firmware/src/heap.rs.",
            RADIO_HEAP_BYTES,
            WIFI_WORKING_SET_BYTES,
        );
    }
}
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
/// [`RADIO_HEAP_BYTES`] is derived from DRAM rather than from this figure, but
/// this is what says the derivation left enough: the high-water mark against the
/// size, on a running board. So it is the measurement rather than decoration — a
/// boot that prints it is a boot that can be quoted. Nowhere near the frame
/// path.
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
    reason = "not called by every binary that includes this file by path"
)]
pub fn free_bytes() -> usize {
    let stats = esp_alloc::HEAP.stats();
    stats.size.saturating_sub(stats.current_usage)
}

/// The largest the heap has been since boot.
///
/// **This is the figure [`RADIO_HEAP_BYTES`] is checked against**, and it is
/// worth publishing rather than only printing, for two reasons the measurement
/// behind that constant established. It is reached within a second of the
/// broker's CONNACK and never moves again, so catching it on a serial cable
/// means catching one second of a boot; and it varies by about two kilobytes
/// from boot to boot on one unchanged image, so one reading is a sample rather
/// than the answer. Over MQTT it becomes something an operator watches across
/// days and across reboots, which is the only shape in which this number means
/// much.
#[allow(
    dead_code,
    reason = "not called by every binary that includes this file by path"
)]
pub fn peak_bytes() -> usize {
    esp_alloc::HEAP.stats().max_usage
}
