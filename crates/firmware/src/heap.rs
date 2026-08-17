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
/// image that boot-looped needed **73,280** — short by 23,688 — and this image,
/// with the inlining fixed, needs 54,720, so it was short by 5,128 even after
/// the bug it hid was gone.
///
/// Nothing checked it, because nothing written here can: a stack requirement is
/// a property of what the compiler emitted. `crate::stack_used` is the answer to
/// that, and it is why this constant now has a measurement standing next to it
/// on every boot instead of a date.
///
/// One straight line, no branch and no recursion, from the executor into the
/// spawn of the state task. Measured 2026-08-17 on this commit:
///
/// | | ESP32 | ESP32-S3 | ESP32-C3 |
/// |---|---|---|---|
/// | `main`, `Executor::run`, `run_inner` | 144 | 144 | 112 |
/// | `TaskStorage<__embassy_main_task>::poll` | 3,856 | 3,856 | 3,840 |
/// | [`crate::start`] | 19,552 | 19,536 | 19,472 |
/// | [`crate::tasks::state`], building the task token | 14,720 | 14,720 | 14,704 |
/// | `UninitCell::write_in_place`, moving the future into its static | 14,736 | 14,736 | 14,736 |
/// | **total** | **53,008** | **52,992** | **52,864** |
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
const BOOT_CHAIN_BYTES: usize = 53_008;

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
const REQUEST_CHAIN_BYTES: usize = 33_504;
/// See the `http` definition above.
#[cfg(not(feature = "http"))]
const REQUEST_CHAIN_BYTES: usize = 0;

/// The deepest chain the two discovery-and-time services can run.
///
/// **A sum rather than a walk, and deliberately so.** Every other term here
/// names one chain and adds its frames; this one adds *every* frame in
/// `edge-mdns`, `domain`, `edge-nal`, `edge-nal-embassy`, `sntpc`,
/// `crate::mdns`, `crate::sntp` and `crate::identity` — 41 functions, 4,240
/// bytes — plus the `embassy-net` and `smoltcp` UDP and DNS frames beneath their
/// socket calls — 15 functions, 1,072 bytes — plus the 144 above `poll`. No
/// single path can exceed a sum over all of them, so 5,456 is an upper bound and
/// not an estimate.
///
/// It is written that way because the honest alternative was worse. Walking
/// these chains properly would mean following `domain`'s message builder through
/// a `visit`/`FnMut` indirection, and a mis-walk there would produce a *number*
/// rather than an obvious error. The two task frames that anchor them are small
/// enough that the crude bound settles the question outright: measured
/// 2026-08-17 on the ESP32-S3 with `mqtt` + `ui` + `mdns` + `sntp`,
/// `TaskStorage<mdns::responder>::poll` is **512** bytes and
/// `TaskStorage<sntp::client>::poll` is **880**, against [`BOOT_CHAIN_BYTES`]'s
/// 53,008.
///
/// It is a term in [`REQUIRED_STACK_BYTES`] rather than a comment for the reason
/// [`REQUEST_CHAIN_BYTES`] is: an mDNS record type added to `crate::mdns`'s
/// `Service`, or a deeper path through `sntpc`, deepens this and nothing else,
/// and the `max` below is what notices.
///
/// Zero when neither service is compiled in, which is not a rounding — there is
/// no such task in that image.
#[cfg(any(feature = "mdns", feature = "sntp"))]
const SERVICE_CHAIN_BYTES: usize = 5_456;
/// See the `mdns`/`sntp` definition above.
#[cfg(not(any(feature = "mdns", feature = "sntp")))]
const SERVICE_CHAIN_BYTES: usize = 0;

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
/// 53,008 + 1,712 = **54,720**. The runners-up are [`REQUEST_CHAIN_BYTES`] at
/// 33,504, [`NETWORK_CHAIN_BYTES`] at 23,280 and [`SERVICE_CHAIN_BYTES`] at
/// 5,456.
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
/// requirement is 54,720, so the margin this division actually buys was, and
/// still is, 66,280 − 54,720 = 11,560 rather than 16,688. Nothing available now
/// was unavailable then; only the account of it was wrong, which is why the sum
/// is kept unchanged and every heap figure measured against it stays valid.
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
/// 66,280 − [`REQUIRED_STACK_BYTES`] = 11,560 bytes, and
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
/// The actual margin today is 11,560, so this floor is 3,368 bytes of slack
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
/// | ESP32 | `mqtt` — see [`crate::api`]; neither `http` nor `sntp` fits | 125,116 |
/// | ESP32-S3 | `mqtt`, `ui` (and so `http`), `mdns`, `sntp` | 152,084 |
/// | ESP32-C3 | `mqtt`, `ui` (and so `http`), `mdns`, `sntp` | 138,888 |
///
/// **Re-measured 2026-08-17 for Plan 6 Task 7**, which is what moved the last
/// two rows. The discovery-and-time services cost the ESP32-S3 7,824 bytes
/// (159,908 → 152,084) and the ESP32-C3 7,784 (146,672 → 138,888), and the
/// ESP32 nothing, because it gets neither. Most of it is the mDNS responder's
/// four buffers and the resolver's four `dns::DnsQuery` slots — `embassy-net`
/// sizes the latter with a hard-coded `MAX_QUERIES = 4` that no feature reaches.
/// The ESP32 figure was re-read as well, on a `--features chip-esp32,mqtt` image,
/// and is unchanged to the byte.
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
const DRAM_FOR_STACK_AND_HEAP: usize = 152_084;
/// See the `chip-esp32` definition above.
#[cfg(feature = "chip-c3")]
const DRAM_FOR_STACK_AND_HEAP: usize = 138_888;

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

// **The ESP32 cannot carry the SNTP client either, and this refuses it for a
// different reason from the one above.**
//
// It is not a link failure: the image builds, boots and runs. It is that the
// heap left over is inside its own noise. Measured 2026-08-17, with
// `--features chip-esp32,mqtt,sntp`:
//
// ```text
// DRAM for stack and heap, with `sntp`                          122,220
//   − STACK_BUDGET_BYTES                                         66,280
//   = 55,940, rounded down to a whole KiB                        55,296   heap
//   − the largest heap high-water yet measured                   54,424
//   = what is left at the announcement burst                        872
// ```
//
// 872 bytes against a **~4,216-byte boot-to-boot spread**, and against a peak
// that was measured on an ESP32-S3 rather than on this chip. That is not a thin
// margin, it is no margin: `esp-alloc` answers exhaustion with a null,
// `handle_alloc_error` panics, and `main`'s handler resets — so the symptom
// would be a board that reboots a few seconds into some boots and not others.
// `warn_if_undersized` would not catch it, because the driver's *resting* set
// still fits; it is the announcement burst that does not.
//
// **And the loss is nothing this chip can currently use.** A wall clock has two
// consumers: log timestamps, and TLS certificate validity for the GitHub OTA
// path. The ESP32 has no web server, so it has no manual-upload OTA either, and
// Plan 6's own heap note says TLS may not fit on any chip. So this refusal costs
// that board a number it would print and nothing else.
//
// Two-thirds of the cost is the resolver rather than the SNTP exchange —
// `embassy-net` sizes its query storage with a hard-coded `MAX_QUERIES = 4` no
// feature reaches — so **what would change this** is a resolver-free time
// source: an NTP server address in the config record instead of a name, at which
// point `sntp` without `embassy-net/dns` is worth re-measuring on this chip.
#[cfg(all(feature = "chip-esp32", feature = "sntp"))]
compile_error!(
    "the ESP32 does not have the DRAM for the SNTP client and its resolver: with `sntp` \
     enabled it has 55,296 bytes of heap against a 54,424-byte measured peak, which is 872 \
     bytes of margin inside a ~4,216-byte boot-to-boot spread — the board would reboot on \
     some boots and not others. Build it with `--no-default-features --features \
     chip-esp32,mqtt`, or use an ESP32-S3 or ESP32-C3. See `heap::RADIO_HEAP_BYTES` for the \
     arithmetic."
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
/// | ESP32 | 125,116 | 57 KiB = 58,368 | 66,748 | 12,028 |
/// | ESP32-S3 | 152,084 | 83 KiB = 84,992 | 67,092 | 12,372 |
/// | ESP32-C3 | 138,888 | 70 KiB = 71,680 | 67,208 | 12,488 |
///
/// Nothing in that table is chosen; it is what the rule returns. Every `stack
/// left` column was read back out of the linked ELF — they are the `.stack`
/// section's own size.
///
/// **The last two rows moved on 2026-08-17 when the discovery-and-time services
/// landed**, and the direction is the one to watch: their statics came out of
/// [`DRAM_FOR_STACK_AND_HEAP`], so the heap fell by 8,192 bytes on the ESP32-S3
/// and 8,192 on the ESP32-C3 while each chip's stack *rose* slightly — the
/// rounding-down leaves the residue on the stack, which is the whole reason it
/// rounds that way. Free at the 54,424-byte peak is now 30,568 on the ESP32-S3
/// and 17,256 on the ESP32-C3. The ESP32 is unchanged because it carries
/// neither service; see the `compile_error!` above for the arithmetic that
/// refuses it.
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
