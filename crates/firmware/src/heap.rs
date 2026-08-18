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
//! measurements rather than arguments: [`REQUIRED_STACK_BYTES`] out of the
//! linked ELF, and [`WIFI_WORKING_SET_BYTES`] against a live broker.
//!
//! ## The stack half was measured once and then left
//!
//! It is worth being exact about how that failed, because the shape of it is
//! general. `REQUIRED_STACK_BYTES` was read off a linked image and was right
//! when it was read. Three of the four frames on the chain it names then grew,
//! the web server added a fourth branch beneath them, and the constant did not
//! move — so `crate::check_stack_headroom` compared a real 66,724 against a
//! fictional 49,592, passed, and the board wrote through its stack guard about a
//! second later. **A check that passes and then the device dies is worse than no
//! check**, because it launders an overflow as a green light.
//!
//! Two things are different now. [`REQUIRED_STACK_BYTES`] is a `max` over the
//! chains this build actually contains rather than one figure, so a
//! configuration cannot silently need more than the constant admits; and the
//! `assert!` beneath [`STACK_MARGIN_FLOOR_BYTES`] turns "it no longer fits" into
//! a build failure. Neither of those can notice the constant itself going stale
//! — nothing written in this file can — which is what `crate::stack_used` is
//! for: it paints the unused stack at boot and reads back how far down the paint
//! was destroyed, so every boot prints a *measurement* next to the claim.

// How every stack figure below is read off a linked image. The commands are
// here because these are the rows that go stale — one of them already did, and
// the cost was a boot loop on the only hardware that exists.
//
//     RUSTFLAGS="-Zemit-stack-sizes -C link-arg=-Tlinkall.x" \
//       cargo build --release --features chip-s3 \
//         --target xtensa-esp32s3-none-elf --bin firmware
//     # frame sizes: a 4-byte address then a ULEB128 size, one entry per function
//     readelf -x .stack_sizes target/xtensa-esp32s3-none-elf/release/firmware
//     # the chain they sit on: who calls whom
//     xtensa-esp32s3-elf-objdump -d -C target/xtensa-esp32s3-none-elf/release/firmware
//     # what the linker actually left, which is the boot line's own figure
//     readelf -S target/xtensa-esp32s3-none-elf/release/firmware | grep '\.stack '
//
// A chain has to be *walked*, not guessed at, and on Xtensa that is less obvious
// than it sounds: a far call is an `l32r` of the target into a register followed
// by `callx8`, so the callee's name appears in the listing as a resolved literal
// rather than as a branch target. A reader grepping for `call8` finds almost
// nothing and concludes the function calls nobody.

/// The deepest chain this firmware runs, and where it runs.
///
/// **This is the term that was wrong**, and the shape of being wrong is worth
/// keeping: [`REQUIRED_STACK_BYTES`] named the right chain and had not been
/// re-read since three of that chain's four frames grew. It said 49,592. The
/// image that boot-looped needed **73,280** — short by 23,688 — and the image
/// with the inlining fixed needed 54,720, so it was short by 5,128 even after
/// the bug it hid was gone. **And it went stale again**, by a further 1,072,
/// which is what the re-measurement below found: the failure is not a one-off,
/// it is what a hand-read constant does.
///
/// Nothing checked it, because nothing written here can: a stack requirement is
/// a property of what the compiler emitted. `crate::stack_used` is the answer to
/// that, and it is why this constant now has a measurement standing next to it
/// on every boot instead of a date.
///
/// One straight line, no branch and no recursion, from the executor into the
/// spawn of the state task. Re-measured 2026-08-18 on this commit:
///
/// | | ESP32 | ESP32-S3 | ESP32-C3 |
/// |---|---|---|---|
/// | `main`, `Executor::run`, `run_inner` | 144 | 144 | 112 |
/// | `TaskStorage<__embassy_main_task>::poll` | 3,856 | 3,856 | 3,840 |
/// | [`crate::start`] | 20,080 | 20,064 | not re-measured |
/// | [`crate::tasks::state`], building the task token | 14,992 | 14,992 | not re-measured |
/// | `UninitCell::write_in_place`, moving the future into its static | 15,008 | 15,008 | not re-measured |
/// | **total** | **54,080** | **54,064** | see below |
///
/// **The figure this replaced was stale by 1,072 bytes, and had been before the
/// change that prompted the re-measurement.** It said 53,008; the ESP32 needed
/// 54,080. That is not a small discrepancy in context: the whole allowance for
/// [`INTERRUPT_FRAMES_BYTES`] is 1,712, so a boot that reported "72 bytes of the
/// requirement unspent" was really reporting that a nested interrupt had almost
/// nowhere to land. Nothing in this file can notice that — a stack requirement
/// is a property of what the compiler emitted — which is exactly why
/// `crate::stack_used` prints a measurement beside the claim on every boot, and
/// why the claim has to be re-read whenever the state machine changes shape.
///
/// **The ESP32-C3 was not re-measured.** It is RISC-V, so its frames are
/// `addi sp, sp, -N` rather than Xtensa's `entry a1, N`, and reading them needs
/// different tooling than the commands at the top of this file. The three
/// figures above are within 16 bytes of each other and the C3 sat 144 *below*
/// the ESP32 when all three were last read together, so taking the ESP32's is
/// still taking the maximum — but that is an inference, and it is the row to
/// re-read first if the C3 ever reports a stale requirement.
///
/// The last row is the leaf: it calls nothing but `memcpy`. It is the state
/// task's 14 KB future being materialised and then copied into the static
/// `#[embassy_executor::task]` declares for it, and it lands *below*
/// [`crate::start`]'s own frame rather than after it has been given back.
///
/// The worst chip's figure is taken, as [`INTERRUPT_FRAMES_BYTES`] is, because a
/// boot check that differs per chip is three numbers to keep true instead of
/// one.
///
/// **What this chain no longer contains, and the bug that put it there.**
/// `crate::start_network` is the *other* branch out of that same `poll` — it
/// runs after [`crate::start`] has returned — and it used to be inlined into it.
/// Inlining does not make two sequential calls share a peak; it makes the
/// callee's slots part of the caller's frame, live for as long as the caller is,
/// including while a 48,992-byte call runs underneath. That put the web server's
/// bring-up **beneath** the deepest thing this firmware does: the ESP32-S3's
/// `poll` frame was 22,432 bytes instead of 3,856 and the chain came to 71,568
/// against 66,724 of stack. `#[inline(never)]` on `crate::start_network` is what
/// separates them — 18,576 bytes, and the whole of the boot loop — and
/// [`NETWORK_CHAIN_BYTES`] is what that branch costs once separated.
/// **Re-measured 2026-08-18 for the settings screen, and deliberately not
/// lowered.** The chain is shallower than this now, because the compiler moved
/// its inlining: `TaskStorage<__embassy_main_task>::poll` fell from 3,856 bytes
/// to 48 as `crate::start`'s body moved into the task closure, which itself grew
/// from 20,064 to 21,424 (the configuration store is kept past boot now, so
/// `report_config` returns four things instead of three). Walked again:
///
/// | | ESP32-S3, this commit |
/// |---|---|
/// | `main`, `Executor::run`, `run_inner` | 96 |
/// | `TaskStorage<__embassy_main_task>::poll` | 48 |
/// | `__embassy_main_task_inner_function::{closure#0}`, which is [`crate::start`] | 21,424 |
/// | [`crate::tasks::state`], building the task token | 15,024 |
/// | `UninitCell::write_in_place`, moving the future into its static | 15,040 |
/// | **total** | **51,632** |
///
/// It is left at 54,080 rather than lowered to that, and the direction is the
/// whole argument: this constant is an **upper bound** the board refuses to boot
/// below, so a figure that is too high costs a little unusable stack while a
/// figure that is too low is the boot loop this file was rewritten for. Lowering
/// it would buy 2,448 bytes of stack that nothing needs — `STACK_BUDGET_BYTES`
/// already clears it by more than the margin floor — in exchange for pinning a
/// number that moves with the optimiser's inlining decisions rather than with
/// this firmware's code. The measurement is recorded so the next reader knows it
/// was taken.
const BOOT_CHAIN_BYTES: usize = 54_080;

/// The chain that brings up Wi-Fi, the web server and the broker session.
///
/// A sibling of [`BOOT_CHAIN_BYTES`] rather than a part of it — same `poll`,
/// different branch — and **the only one of the three that depends on which
/// transports are compiled in**, which is why it is here rather than folded into
/// a single figure. Measured 2026-08-17 on this commit, whole chain including
/// the 144 bytes above `poll` and `poll`'s own 3,856:
///
/// | build | `crate::start_network` | chain |
/// |---|---|---|
/// | ESP32-S3, `mqtt` + `ui` | 18,784 | 23,280 |
/// | ESP32-S3, `mqtt` | 9,568 | 13,936 |
/// | ESP32, `mqtt` | 9,568 | 14,416 |
/// | ESP32-C3, `mqtt` + `ui` | 8,176 | 12,352 |
///
/// The web server adds 9,216 bytes to `crate::start_network`'s own frame, and it
/// is dominated by one line: `api::start` hands `BUFFERS.init` an
/// `[Buffers; HTTP_TASKS]` **by value**, which is `HTTP_TASKS` × 3,584 = 14,336
/// bytes materialised on this stack on its way into a static. It is affordable
/// here and would not be under [`BOOT_CHAIN_BYTES`]; that is the whole reason the
/// two are kept apart. A `static_cell::ConstStaticCell` would take it to zero and
/// is the first thing to reach for if this branch ever needs to shrink.
#[cfg(feature = "http")]
const NETWORK_CHAIN_BYTES: usize = 23_280;
/// See the `http` definition above.
#[cfg(not(feature = "http"))]
const NETWORK_CHAIN_BYTES: usize = 14_416;

/// The deepest chain a *request* runs, once the device is up.
///
/// `picoserve`'s router is a type per route wrapping the previous one, so a
/// request walks a nest of monomorphised frames rather than a loop. Measured
/// 2026-08-17 on the ESP32-S3 with `mqtt` + `ui`, from the executor down through
/// `TaskStorage<connection>::poll` (2,064), the connection's `select` (7,600 +
/// 7,552), the path-parameter route for `/api/v1/shades/:id` (8,720) and the
/// response writers beneath it: **33,504 bytes**, about 19 KB clear of
/// [`BOOT_CHAIN_BYTES`].
///
/// It is a term in [`REQUIRED_STACK_BYTES`] rather than a comment because that
/// clearance is the thing that could stop being true: a route added to
/// `api::routes` deepens this and nothing else, and the `max` below is what
/// notices.
///
/// **Re-measured when `/confirm-pairing` was added**, because that is exactly
/// the change the paragraph above warns about. The router flattens far more
/// than its type suggests: of nineteen `picoserve::routing::Route`
/// monomorphisations in the image only three have frames of their own — the
/// `/api/v1/shades/:id` route at **8,720**, the WebSocket route at 7,456, and
/// **one** frame at 2,512 for the whole `(&str, ParsePathSegment<u8>, &str)`
/// family, into which `/pair`, `/command` and now `/confirm-pairing` are all
/// inlined. So the 8,720 this figure was walked through is byte-identical, the
/// new route added no frame of its own, and the clearance is unchanged.
///
/// Zero without the web server, which is not a rounding — there is no connection
/// task in that image at all.
#[cfg(feature = "http")]
/// **Re-measured 2026-08-18, when four settings routes were added** — which is
/// exactly the change the paragraph above warns about. Every one of them is a
/// plain literal path, the same `&str` family `/api/v1/shades`, `/api/v1/groups`
/// and `/api/v1/rooms` were already in, so none adds a route shape. The frames
/// moved anyway, and in opposite directions: the connection's `select` merged
/// into one 15,840-byte frame where it was two of 7,600 and 7,552, while the
/// `Route<&str, MethodRouter<…>>` frame fell from 8,720 to 6,544. Net about
/// −1,500, so the clearance below [`BOOT_CHAIN_BYTES`] is wider than it was and
/// this figure stays as the upper bound it is.
///
/// **What the routes did cost is DRAM, not stack** — 10,664 bytes of it, in the
/// four connection task futures. See [`DRAM_FOR_STACK_AND_HEAP`], which is where
/// that shows up and where it was paid for.
const REQUEST_CHAIN_BYTES: usize = 33_504;
/// See the `http` definition above.
#[cfg(not(feature = "http"))]
const REQUEST_CHAIN_BYTES: usize = 0;

/// What an interrupt costs on top of whatever chain it lands on.
///
/// An interrupt lands on whatever stack was running, and on this firmware that
/// is the main one — **observed, not assumed**: the boot loop this file was
/// rewritten for died at `0x40378954`, the first instruction of
/// `xtensa_lx_rt`'s `__default_naked_exception`, whose second and third
/// instructions drop the stack pointer by 256 and store through it. That store
/// is what reached the guard word.
///
/// `xtensa-lx-rt` allocates `XT_STK_FRMSZ` = 256 bytes per entry
/// (`xtensa-lx-rt-0.22.0/src/exception/asm.rs:81`, and visible as
/// `addmi a1, a1, 0xffffff00` at the top of every vector) and then calls a
/// handler with its own frame; the five entries that can nest —
/// `__user_exception`, `__level_1`, `__level_2`, `__level_3` and
/// `__default_double_exception` — cost 5 × 256 plus 432 of handler frames on the
/// worst chip. All five stacked at once is not a scenario anyone has seen; it is
/// the bound.
///
/// **It does not cover the bodies those handlers dispatch into.**
/// `esp_radio`'s `Handler::dispatch` calls straight into the closed Wi-Fi driver
/// from interrupt context (`esp-radio-0.18.0/src/interrupt_dispatch.rs:24`), and
/// neither that blob nor masked ROM carries stack-size metadata, so no sum over
/// them is available. [`STACK_MARGIN_FLOOR_BYTES`] is what stands behind that,
/// and [`crate::stack_used`] is what will eventually price it.
const INTERRUPT_FRAMES_BYTES: usize = 1_712;

/// Main stack this firmware refuses to start without.
///
/// The largest chain in this build plus what an interrupt adds to it. It is a
/// `max` rather than a sum because the three chains are alternatives — the
/// executor is in exactly one of them at a time — and it is a `max` rather than
/// a single figure because **which one is largest is a property of the enabled
/// features**, and the day that changes this arithmetic changes with it instead
/// of being re-derived by hand.
///
/// Today, on every configuration in the matrix, [`BOOT_CHAIN_BYTES`] wins:
/// 54,080 + 1,712 = **55,792**.
///
/// Checked at run time by `crate::check_stack_headroom` rather than asserted at
/// compile time, because the quantity it is checked *against* cannot be a
/// constant: esp-hal's linker script gives the stack whatever DRAM is left after
/// the statics, so it moves every time a static is added — and since
/// [`RADIO_HEAP_BYTES`] is the largest static in the image, every time that
/// changes too. What *is* asserted at compile time is that this fits the
/// division below.
pub const REQUIRED_STACK_BYTES: usize = larger(
    larger(
        larger(BOOT_CHAIN_BYTES, NETWORK_CHAIN_BYTES),
        REQUEST_CHAIN_BYTES,
    ),
    SERVICE_CHAIN_BYTES,
) + INTERRUPT_FRAMES_BYTES;

/// The deepest chain through the mDNS responder and the SNTP client, measured
/// the same way as the others — 5,456 bytes, far below [`BOOT_CHAIN_BYTES`], so
/// it does not set the requirement today.
///
/// It is a term in [`REQUIRED_STACK_BYTES`] rather than a comment for the reason
/// [`REQUEST_CHAIN_BYTES`] is: an mDNS record type added to `crate::mdns`'s
/// `Service`, or a deeper path through `sntpc`, deepens this and nothing else,
/// and the `max` above is what notices. **This term survived a merge in which
/// the boot chain was re-derived on a branch that did not know these services
/// existed** — recorded because a `max` whose smaller terms quietly disappear
/// still produces the right number, and stops producing it the moment one of
/// them grows.
///
/// Zero when neither service is compiled in, which is not a rounding — there is
/// no such task in that image.
#[cfg(any(feature = "mdns", feature = "sntp"))]
const SERVICE_CHAIN_BYTES: usize = 5_456;
/// See the `mdns`/`sntp` definition above.
#[cfg(not(any(feature = "mdns", feature = "sntp")))]
const SERVICE_CHAIN_BYTES: usize = 0;

/// `usize::max`, which is not a `const fn`.
const fn larger(left: usize, right: usize) -> usize {
    if left > right {
        left
    } else {
        right
    }
}

/// The main stack every chip is left, before the heap takes the rest.
///
/// **This is the division, and it is one number for all three chips.**
/// `esp_alloc::heap_allocator!` declares a static array and esp-hal's linker
/// script gives the main stack whatever DRAM is left once the statics are
/// placed, so the heap and the stack are two shares of one fixed quantity —
/// measurably so, and checked rather than assumed: the heap moved by +4,096 on
/// the ESP32, +109,568 on the ESP32-S3 and +96,256 on the ESP32-C3 when this
/// constant was introduced, and each chip's stack fell by exactly that, to the
/// byte, in the relinked ELF. There is no third option and no slack between
/// them; choosing one chooses the other.
///
/// ### Why it is 66,280 and why it must not rise
///
/// It used to be written as `49_592 + 16_688` — a requirement plus a margin —
/// and **both halves of that were wrong while their sum was right.** The
/// requirement is 55,792, so the margin this division actually buys is
/// 66,280 − 55,792 = 10,488 rather than 16,688. Nothing available then was
/// unavailable now; only the account of it was wrong, which is why the sum is
/// kept unchanged and every heap figure measured against it stays valid.
///
/// The figure is fixed **by the ESP32's heap**, which is the binding constraint
/// on the whole design and the reason a "pick the stack first" rule cannot be
/// followed all the way down:
///
/// ```text
/// ESP32 DRAM for stack and heap                                125,116
///   − this budget                                               66,280
///   = 58,836, rounded down to a whole KiB                        58,368   heap
///   − the largest heap high-water yet measured                   54,424
///   = what the ESP32 has left at the announcement burst           3,944
/// ```
///
/// **3,944 bytes is the entire slack in this design**, and it is inside the
/// heap's own ~4,216-byte boot-to-boot noise, which is why that figure is the
/// one to watch on that chip. Every byte added to this budget comes out of it.
/// Raising the budget by 4 KiB does not cost the ESP32 a margin; it costs it the
/// ability to finish announcing.
///
/// The two chips with DRAM to spare are not the constraint and do not get to set
/// it: at this budget the ESP32-S3 runs a 93,184-byte heap against the same
/// 54,424 peak, and the ESP32-C3 79,872.
///
/// ### What the difference buys
///
/// 66,280 − [`REQUIRED_STACK_BYTES`] = 10,488 bytes, and
/// [`STACK_MARGIN_FLOOR_BYTES`] is the least of it this division may leave.
pub const STACK_BUDGET_BYTES: usize = 66_280;

/// The least the division may leave over the measured requirement.
///
/// **A policy figure, and said so rather than dressed up.** What it stands
/// behind is the one thing [`REQUIRED_STACK_BYTES`] cannot see: the bodies
/// `esp-radio`'s interrupt handlers dispatch into, which are a closed blob and
/// masked ROM with no stack-size metadata to sum over. There is no derivation
/// available for a quantity nobody can read, so this is a reserve rather than a
/// measurement, and 8 KiB is chosen as roughly five times the entry cost
/// [`INTERRUPT_FRAMES_BYTES`] does account for.
///
/// The actual margin today is 10,488, so this floor is 2,296 bytes of slack
/// before the build stops. That is deliberate: it is a *gate*, not a target, and
/// it exists so that the failure of a growing call graph is a build error naming
/// two numbers rather than a device that passes its own boot check and then
/// writes through its stack guard.
///
/// The one measurement that ever priced the unseen part found it at zero: a
/// painted stack on an ESP32-S3, associated with a real access point and
/// announcing to a real broker, read back exactly the chain the ELF computed and
/// not one byte more. `crate::stack_used` makes that measurement permanent, so
/// this figure can eventually be replaced by one.
const STACK_MARGIN_FLOOR_BYTES: usize = 8 * 1024;

// **The gate, and it is the one thing here that can stop a build.**
//
// A configuration whose deepest chain has grown past what the division leaves
// fails to compile, naming both numbers — which is what the ESP32 + `http`
// refusal below already does for a different reason, generalised to the reason
// that actually bit.
//
// It is deliberately *not* satisfied by construction: `STACK_BUDGET_BYTES` is
// the DRAM division, fixed by the ESP32's heap, and `REQUIRED_STACK_BYTES` is
// what the compiler emitted. Neither is defined in terms of the other, so the
// comparison is a real one.
//
// The two ways out when it fires are both real work and neither is editing this
// line: make the chain shallower — `crate::start_network`'s `#[inline(never)]`
// is what that looks like, and it recovered 18,576 bytes — or move the division
// and pay for it out of the ESP32's 3,944-byte heap slack, which needs hardware.
const _: () = assert!(
    STACK_BUDGET_BYTES >= REQUIRED_STACK_BYTES + STACK_MARGIN_FLOOR_BYTES,
    "the deepest stack chain in this configuration no longer fits the DRAM \
     division: see heap::REQUIRED_STACK_BYTES for what it needs, \
     heap::STACK_BUDGET_BYTES for what the division leaves, and \
     heap::STACK_MARGIN_FLOOR_BYTES for the reserve that must survive between \
     them. Re-read the chains from a linked ELF before changing any of the \
     three — the commands are in this file.",
);

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
/// It **had** gone stale once, by 3,520 bytes on every chip, and the symptom was
/// visible on a serial console for anyone who compared two numbers: the boot
/// line read `stack: 63796 bytes available` where these constants implied
/// 66,788, so the stack was 3,520 bytes shorter than the budget claimed and
/// nothing was going to say so. Re-measured 2026-08-17 against the images below.
///
/// **Re-checked again 2026-08-17** while the stack requirement was being
/// re-derived, and all three are still exact. The check is a subtraction that
/// needs no serial console: `readelf -S | grep '\.stack '` gives the region the
/// linker left, and it must equal `DRAM_FOR_STACK_AND_HEAP - RADIO_HEAP_BYTES`.
/// It read 66,748 on the ESP32 (125,116 − 58,368), 66,724 on the ESP32-S3
/// (159,908 − 93,184) and 66,800 on the ESP32-C3 (146,672 − 79,872) — and the
/// ESP32-S3 figure is the same 66,724 the boot loop printed, which is what says
/// this row was not the one at fault.
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
/// | ESP32 | `mqtt` — see [`crate::api`]; `http` does not fit | 123,996 |
/// | ESP32-S3 | `mqtt`, `ui` (and so `http`) | 136,020 |
/// | ESP32-C3 | `mqtt`, `ui` (and so `http`) | 122,816 |
///
/// The web server costs about **69,000 bytes on the ESP32, 70,000 on the
/// ESP32-S3 and 70,000 on the ESP32-C3** — three measurements of one thing, and the spread is
/// the per-architecture difference in what a generator lays out. Four connection
/// tasks are 52,384 of it (`api::HTTP_TASKS` × a 13,096-byte future, which is
/// `picoserve`'s router recursion) and their buffers are 14,336.
/// **Re-measured 2026-08-18 for the settings screen, and this one is a real
/// cost rather than a correction.** The four settings routes and their handlers
/// took **10,664 bytes** of DRAM on both the ESP32-S3 and the ESP32-C3, and
/// essentially all of it is the web server's connection tasks: `picoserve`'s
/// router is a type per route, so every path is a variant of the monomorphised
/// future each of the [`crate::api::HTTP_TASKS`] tasks holds statically.
/// `firmware::api::connection::POOL` went from 52,384 bytes to 67,840 — 13,096
/// per task to 16,960.
///
/// Two things were done about that before it was accepted, and both are
/// measured:
///
/// - **The two endings of a Wi-Fi trial became one route with the decision in
///   the body** (`somfy_api::TrialDecisionDto`), which recovered 1,440 bytes.
///   That is the same trade `/calibrate` made.
/// - **Everything the settings screen reaches is `#[cfg(feature = "http")]`** —
///   the `rpc::Request` variants, `tasks::Table::config`, the state task's arms.
///   The ESP32 cannot link the web server and has the least heap headroom of the
///   three, so it now pays 496 bytes rather than 1,632, and **its heap is
///   unchanged at 56 KiB**.
///
/// What was tried and rejected, so it is not tried again:
/// `picoserve::response::Json` streams instead of holding a buffer, but its
/// `JsonStream` keeps the value *and* a serializer state live across the write —
/// the connection future grew to 18,904 bytes per task, 7,776 across the four,
/// against the 2,688 the wider fixed buffer costs. See `api::routes::JsonBody`.
///
/// **The ESP32-C3 is the chip this bill lands on**, and the figure is recorded
/// here rather than left to be discovered: its heap falls from 65 KiB to 55 KiB,
/// which is 1,700 bytes above the worst announcement peak ever measured — on a
/// *different* chip, against a peak that varied by about 2,000 bytes between
/// boots of one unchanged image. [`warn_if_tight`] says so at boot. It has not
/// been run on hardware.
///
/// **Re-measured 2026-08-18 before that, and the reason is worth recording.** Two branches
/// were merged that each added statics — the mDNS/SNTP services and the
/// calibration state — and each had re-measured this row against a tree without
/// the other. Resolving the conflict by taking one side kept a figure that was
/// correct for neither, and the merged S3 image claimed 159,908 where the linker
/// gave 146,700. **The board refused to boot rather than overflowing**, printing
/// `StackTooSmall { available: 53516, required: 55792 }`, which is
/// `check_stack_headroom` doing precisely the job the stale-figure story above
/// describes — the first time it has caught a real one.
///
/// The lesson is about the merge, not the arithmetic: this row is a property of
/// *the whole image*, so two correct measurements of two different images do not
/// combine, and a conflict here can only be settled by measuring again. The
/// subtraction below was cross-checked against the serial console — the ELF gave
/// 53,516 for the S3 and the board reported `available: 53516`.
///
/// **Re-measured 2026-08-18, −16 bytes on every chip**, when the per-shade
/// transmit width landed. Nothing on any stack chain moved — every frame on the
/// boot chain is byte-identical, so [`REQUIRED_STACK_BYTES`] is unchanged — but
/// the image gained 16 bytes of statics, and this row is about the image rather
/// than about the chains. Measured the documented way, the same worktree built
/// twice with the change out and in:
///
/// | chip | `.stack` before | after |
/// |---|---|---|
/// | ESP32 (`mqtt`) | 67,164 | 67,148 |
/// | ESP32-S3 (all) | 66,828 | 66,812 |
/// | ESP32-C3 (all) | 66,952 | 66,936 |
///
/// Identical on all three, and on both instruction sets, so it is a data static
/// rather than anything a code generator chose. **[`RADIO_HEAP_BYTES`] does not
/// move**: the division rounds down to a whole KiB and 16 bytes does not cross a
/// boundary on any of the three, so the whole of it comes out of the stack —
/// which is the direction this row's rounding is chosen to fail in. The margin
/// over [`REQUIRED_STACK_BYTES`] is ~11 KB, so the change is recorded here
/// because the self-check above must stay exact, not because anything is tight.
#[cfg(feature = "chip-esp32")]
const DRAM_FOR_STACK_AND_HEAP: usize = 123_996;
/// See the `chip-esp32` definition above.
#[cfg(feature = "chip-s3")]
const DRAM_FOR_STACK_AND_HEAP: usize = 136_020;
/// See the `chip-esp32` definition above.
#[cfg(feature = "chip-c3")]
const DRAM_FOR_STACK_AND_HEAP: usize = 122_816;

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
/// | chip | DRAM to divide | heap | stack left | spare over [`REQUIRED_STACK_BYTES`] |
/// |---|---|---|---|---|
/// | ESP32 | 123,996 | 56 KiB = 57,344 | 66,652 | 10,860 |
/// | ESP32-S3 | 136,020 | 68 KiB = 69,632 | 66,388 | 10,596 |
/// | ESP32-C3 | 122,816 | 55 KiB = 56,320 | 66,496 | 10,704 |
///
/// Nothing in that table is chosen; it is what the rule returns. Every `stack
/// left` column was read back out of the linked ELF on 2026-08-18 — they are the
/// `.stack` section's own size.
///
/// **This table went stale twice before it was derived rather than
/// transcribed**, which is why every figure in it now comes from one of two
/// places: `readelf -S | grep '\.stack '` for the `stack left` column, and the
/// rule above for the heap.
///
/// **The table used to read 233,700 for the ESP32-S3 and a 163 KiB heap, and
/// that was a measurement of an image without the web server in it.** It went
/// stale in the same commit that added one, alongside the `DRAM` figures below,
/// which *were* updated. Two tables describing one division, only one of them
/// maintained — recorded here because the fix is that this one is now derived
/// from the same constants rather than transcribed beside them.
///
/// ### What the ESP32-S3 gained, measured rather than asserted
///
/// The figure read off `heap: session announced`, which `crate::mqtt` prints one
/// line after the burst of retained discovery configs that *is* the peak — the
/// two older `report` call sites both run before that burst, so neither ever
/// showed the number this constant is chosen from:
///
/// | | 56 KB heap | 163 KiB heap | today, 91 KiB |
/// |---|---|---|---|
/// | heap total | 57,344 | 166,912 | 93,184 |
/// | steady use | 47,464 – 47,608 | 47,464 – 47,608 | *unchanged* |
/// | worst peak seen | 54,620 | 54,424 | *not re-measured* |
/// | **free at that peak** | **2,724** | **112,488** | **38,760** |
///
/// The steady figure does not move, which is the point: the driver's resting
/// working set was never the problem. The old margin was 2,724 bytes against a
/// peak that itself varied by about 2,000 bytes between boots of one unchanged
/// image — a margin inside its own noise, which is a coincidence with a good
/// track record rather than a design.
///
/// **The third column is arithmetic, not a fresh measurement**, and the
/// difference matters: the web server took 73,728 bytes of this heap when it
/// took its DRAM, so what is left is 93,184 rather than 166,912. The peak it is
/// compared against is still the one read at 163 KiB, because the heap's
/// consumers — the Wi-Fi driver's packet buffers and the announcement burst —
/// are not what the web server changed. A boot that prints `heap: session
/// announced` on the current image is what would turn that reasoning into a
/// reading.
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

/// The worst heap the driver and one announcement burst have ever needed.
///
/// 54,620 bytes, read off `heap: session announced` — the line `crate::mqtt`
/// prints one step after the burst of retained discovery configs that *is* the
/// peak. Measured on an ESP32-S3 against a real broker with a real installation;
/// [`WIFI_WORKING_SET_BYTES`] is the resting figure the same boots settled on,
/// and the difference between the two is what one announcement costs.
///
/// **It is one chip's number and it varies.** Across boots of one unchanged
/// image the peak moved by about 2,000 bytes, and it scales with the shade
/// count, so it is a floor on what a bigger installation would need rather than
/// a ceiling on anything. That is precisely why [`warn_if_tight`] compares
/// against it and says so out loud instead of a `const` assertion pretending to
/// know.
#[allow(dead_code, reason = "see the allow on `RADIO_HEAP_BYTES`")]
pub const WIFI_PEAK_BYTES: usize = 54_620;

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
    warn_if_tight();
}

/// Say at boot when the heap clears the working set but not the peak by much.
///
/// # Why a second, softer line
///
/// [`warn_if_undersized`] catches a heap that cannot hold the driver *at rest*,
/// which is a board that reboots seconds into every boot. This catches the one
/// that holds the driver and then runs out during the announcement burst — a
/// board that associates, connects to the broker, and dies while publishing,
/// which looks like a broker problem and is not.
///
/// **The ESP32-C3 is the reason it exists.** The settings screen cost 10,664
/// bytes of DRAM in the web server's connection tasks, and on the C3 that takes
/// the heap from 65 KiB to 55 KiB — 1,700 bytes above [`WIFI_PEAK_BYTES`],
/// against a peak that itself moved 2,000 bytes between boots of one unchanged
/// image. A margin inside its own noise is a coincidence, not a design, and the
/// honest thing is to say which boot it was measured on and let the board report
/// what it actually sees.
///
/// Not a refusal, and not a `const` assertion, for the same reason
/// [`warn_if_undersized`] is neither: the peak is one chip's measurement and a
/// compile-time refusal would take the affected chip out of the matrix that
/// would catch the problem.
#[allow(dead_code, reason = "see the allow on `RADIO_HEAP_BYTES`")]
fn warn_if_tight() {
    // Only when the heap clears the resting set, so this never doubles up on
    // the harder line above.
    if RADIO_HEAP_BYTES >= WIFI_WORKING_SET_BYTES
        && RADIO_HEAP_BYTES < WIFI_PEAK_BYTES + PEAK_NOISE_BYTES
    {
        esp_println::println!(
            "heap: {} bytes leaves {} above the worst announcement peak ever measured \
             ({}), which is inside the {}-byte spread that peak showed between boots. \
             Watch `heap: session announced` on this board — if it lands near the \
             total, an announcement can exhaust the heap and reset it. See \
             crates/firmware/src/heap.rs.",
            RADIO_HEAP_BYTES,
            RADIO_HEAP_BYTES - WIFI_PEAK_BYTES,
            WIFI_PEAK_BYTES,
            PEAK_NOISE_BYTES,
        );
    }
}

/// How much [`WIFI_PEAK_BYTES`] moved between boots of one unchanged image.
///
/// About 2,000 bytes, from the sixteen boots the peak itself was read from. It
/// is the width of the band inside which "the heap is big enough" cannot be
/// distinguished from "it happened not to run out this time", which is the
/// judgement [`warn_if_tight`] exists to hand to whoever is watching the console.
#[allow(dead_code, reason = "see the allow on `RADIO_HEAP_BYTES`")]
const PEAK_NOISE_BYTES: usize = 2_000;
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
