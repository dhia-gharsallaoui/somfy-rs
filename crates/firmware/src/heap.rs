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
//! the chip being built happens to have. **It is one rule over two inputs
//! rather than one number**, and the reason is that the chips are not the same
//! size: a single shared figure has to fit the smallest of them, which is how an
//! ESP32-S3 with 233,700 bytes to divide came to run its heap at 96% full next
//! to 176,356 bytes of stack it had no use for.
//!
//! The objection to per-chip figures is that it makes the chip nobody can test
//! the one running a configuration nobody has measured. That is true, and it was
//! equally true of the shared constant — no ESP32-C3 had ever run that either.
//! What changes it is that the rule is now identical on every chip and only its
//! input differs, and that both quantities it is built from are measurements
//! rather than arguments: [`REQUIRED_STACK_BYTES`] out of the linked ELF, and
//! [`WIFI_WORKING_SET_BYTES`] against a live broker.
//!
//! ## Why the ESP32 is no longer one of them
//!
//! Dropped 2026-08-18, and it is the removal of an unverified claim rather than
//! a reduction in capability: **no ESP32 has ever booted this firmware.** It was
//! already excluded from the web server by a `compile_error!` here, and its one
//! buildable configuration — `mqtt` alone — measured 123,284 bytes of DRAM, so
//! its heap was 56,320 against a [`WIFI_PEAK_BYTES`] of 54,620. **+1,700, inside
//! that peak's own ~2,000-byte boot-to-boot spread**, with no smaller
//! configuration left to retreat to. A margin inside its own noise is a
//! coincidence with a good track record, not a fit, and the claim that the chip
//! was supported could not be backed. `docs/provenance.md` carries the
//! arithmetic next to the ESP32-S2's, which was dropped on 2026-08-17 for the
//! same reason.
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
//!
//! ## This file loses things in merges, and a partial repair is its own failure
//!
//! Three times, and the third is the one worth recording because it was silent.
//! Two branches each added statics to the image and each re-measured
//! [`DRAM_FOR_STACK_AND_HEAP`] against a tree without the other; resolving by
//! taking one side kept a figure correct for neither, and the board refused to
//! boot — caught, loudly, by `crate::check_stack_headroom`. A second resolution
//! dropped [`SERVICE_CHAIN_BYTES`] from the `max`, which was noticed and
//! restored in a commit that says so in its subject.
//!
//! **That same resolution also dropped a `compile_error!` — one refusing the
//! ESP32 the SNTP client — and nobody looked.** It was gone for weeks while a
//! comment in `.github/workflows/ci.yml` went on asserting that "both refusals
//! are `compile_error!`s in `heap.rs`". Nothing failed, because what was lost
//! was a *guard*: its whole job is to be silent, so losing it is
//! indistinguishable from it passing. The lesson is not "merge carefully" — it
//! is that **noticing one thing a bad resolution dropped is evidence there are
//! others, and the repair is to diff the whole file against both parents rather
//! than to fix the item that announced itself.**
//!
//! It is moot for the constant it guarded, since the ESP32 was dropped
//! (2026-08-18). It is not moot for the two refusals beside
//! [`DRAM_FOR_STACK_AND_HEAP`], which are now what keeps that row a maximum.

// How every stack figure below is read off a linked image. The commands are
// here because these are the rows that go stale — one of them already did, and
// the cost was a boot loop on the only hardware that exists.
//
//     RUSTFLAGS="-Zemit-stack-sizes -C link-arg=-Tlinkall.x" \
//       cargo build --release --features chip-s3 \
//         --target xtensa-esp32s3-none-elf --bin firmware
//     # frame sizes: a 4-byte address then a ULEB128 size, one entry per function
//     readelf -x .stack_sizes target/xtensa-esp32s3-none-elf/release/firmware
//     # ...which is a hex dump, so `stacksizes.py` beside this crate decodes it
//     # and sorts by frame, largest first:
//     python3 stacksizes.py \
//       target/xtensa-esp32s3-none-elf/release/firmware firmware::restore
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
/// **Re-read 2026-08-18 after Plan 6 Task 3, and it did not grow.** That task
/// added a fourth flash region and a `provision_estate` pass over it, so the
/// question was whether `crate::start`'s frame took any of it: on the ESP32 it
/// reads **0x4e60 = 20,064**, sixteen bytes *below* the figure in the table
/// above, and `crate::tasks::state` is unchanged at 0x3a90 = 14,992.
/// `firmware::provision_estate` has a frame of its own (0x9c0 = 2,496) and
/// returns before the state task is built, so it sits about 25 KB down a branch
/// that is not this one. The constant stays where it is, as an upper bound.
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
///
/// **Re-measured 2026-08-18 for over-the-air updates, and it had gone stale
/// again — on `main`, before that work, in the direction that boot-loops.**
/// Both Xtensa chips were walked with the commands at the top of this file, and
/// the same worktree was built twice with the change out and in so the two
/// causes could be separated:
///
/// | | ESP32, before | ESP32, after | ESP32-S3, before | ESP32-S3, after |
/// |---|---|---|---|---|
/// | `main`, `Executor::run`, `run_inner` | 144 | 144 | 144 | 144 |
/// | `TaskStorage<__embassy_main_task>::poll` | 3,856 | 3,856 | 48 | 3,856 |
/// | [`crate::start`] | 20,320 | 20,320 | 21,680 (inlined) | 20,096 |
/// | [`crate::tasks::state`] | 15,248 | 15,248 | 15,280 | 15,648 |
/// | `UninitCell::write_in_place` | 15,264 | 15,264 | 15,296 | 15,664 |
/// | **total** | **54,832** | **54,832** | **52,448** | **55,408** |
///
/// Two separate findings, and only one of them belongs to the change:
///
/// - **The ESP32 column does not move at all**, and it already read 54,832
///   against a constant of 54,080. So this row was **stale by 752 bytes on
///   `main`** — the shortfall existed before the update path was written and
///   nothing was going to say so. That is the third time, and it is the reason
///   `crate::stack_used` prints a measurement beside the claim on every boot.
/// - **The ESP32-S3 column moves by 2,960**, and most of it is the optimiser
///   rather than this firmware: [`crate::start`] stopped being inlined into the
///   task closure, which moved 21,680 bytes out of a `poll` frame that
///   correspondingly grew from 48 to 3,856. The part that is genuinely new is
///   **368 bytes** on each of the two state-task frames, which is
///   `crate::tasks::Table`'s new `ota` field — the upload session and its image
///   verifier, put on this stack deliberately rather than in a `static`,
///   because the `static` would have come out of the Wi-Fi driver's heap
///   instead. See [`DRAM_FOR_STACK_AND_HEAP`].
///
/// The constant takes the larger of the two chips, as it always has: **55,408**.
///
/// **Re-read 2026-08-18 for the diagnostics and backup screens, and it had gone
/// stale again — by 224 bytes, in the direction that boot-loops.** That is the
/// fourth time, and it was caught only because the same walk was being done for
/// [`RESTORE_CHAIN_BYTES`]; nothing else would have said so. Walked on the
/// ESP32-S3 with the commands at the top of this file:
///
/// | | before | after |
/// |---|---|---|
/// | `main`, `Executor::run`, `run_inner` | 144 | 144 |
/// | `TaskStorage<__embassy_main_task>::poll` | 3,856 | 3,856 |
/// | [`crate::start`] | 20,096 | 20,160 |
/// | [`crate::tasks::state`] | 15,648 | 15,728 |
/// | `UninitCell::write_in_place` | 15,664 | 15,744 |
/// | **total** | **55,408** | **55,632** |
///
/// The 64 bytes on `start` are the staging region and the early return that
/// carries it; the 80 on each state-task frame are `crate::tasks::Table`'s two
/// new fields — the staging region and an export's checksum. Both are small
/// because they were *made* small: the restore's real cost is on a chain of its
/// own ([`RESTORE_CHAIN_BYTES`]) and an export's whole state is a reference and
/// a `u32`.
///
/// It is raised rather than left, unlike the 2026-08-18 re-measurement above:
/// that one found the chain *shallower* than the constant, where this one finds
/// it deeper, and this constant is an upper bound the board refuses to boot
/// below.
/// The ESP32-C3 is still not walked — see the note above — and the inference
/// that it sits below the Xtensa figures is unchanged.
///
/// **The ESP32 was dropped on 2026-08-18 and this figure does not move**, which
/// is the one thing worth checking when a chip leaves: it was the *smaller* of
/// the two Xtensa columns above (54,832 against the ESP32-S3's 55,408), so the
/// `max` was never its. The tables above keep their ESP32 columns because they
/// are a record of measurements that were taken, not a claim about what is
/// built today — and because the repeated finding they carry is that this row
/// goes stale, which is about the reading rather than about the chip.
const BOOT_CHAIN_BYTES: usize = 55_632;

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
///
/// **Not re-derived for the `Origin`/`Host` extractor added 2026-08-18, and
/// here is the reasoning rather than a figure pretending to be a measurement.**
/// What it adds to this chain is one leaf frame — `FromThisDevice::
/// from_request_parts`, whose locals are an eighteen-byte hostname and two
/// header slices, with no buffer and no recursion — beneath a chain that has
/// **20,576 bytes of clearance** below [`BOOT_CHAIN_BYTES`]. A leaf that size
/// cannot move the `max`, so [`REQUIRED_STACK_BYTES`] is unchanged and the
/// figure stays the upper bound it already is. **What would make a real
/// derivation necessary:** an extractor that reads a body, buffers a header, or
/// calls into `crate::rpc` — any of which would put a real frame on this chain
/// rather than a leaf.
///
/// **Re-derived 2026-08-18 for the firmware-upload route, which is exactly the
/// change the paragraph above warns about — and this one did move the chain.**
/// `POST /api/v1/ota` is a `RequestHandlerService` rather than a handler
/// function, so it is inlined into the connection's `select` with a streaming
/// read and four `crate::rpc` calls inside it, and the select frame grew from
/// **10,000 to 24,672 bytes**. Walked again, same worktree built twice:
///
/// | | bytes |
/// |---|---|
/// | `main`, `Executor::run`, `run_inner` | 144 |
/// | `TaskStorage<connection>::poll` | 2,064 |
/// | the connection's `select` | 20,864 |
/// | the deepest `Route<&str, MethodRouter<…>>` | 7,648 |
/// | `ChunkedResponse<Collection>` | 3,696 |
/// | `Refusal::write_to` | 1,392 |
/// | **total** | **35,808** |
///
/// **20,864 rather than 24,672 because 3,808 of it was given back**, and how is
/// worth recording: the handler was written first with a `finalize().write_to()`
/// pair per outcome — four of them, which is what a service handler invites —
/// and each pair is inlined into the poll with its own response writer beneath
/// it. Collapsing them into the single `Result<_, Result<Refusal, Unavailable>>`
/// that every other handler in `api::routes` already returns recovered that much
/// with no change in behaviour. The remaining ~10,900 over the previous figure
/// is the streaming read and the request/reply values of four `crate::rpc`
/// round trips, and it is affordable because this chain sits **19,600 bytes
/// below** [`BOOT_CHAIN_BYTES`] — which is what the `max` in
/// [`REQUIRED_STACK_BYTES`] is for.
const REQUEST_CHAIN_BYTES: usize = 35_808;
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
/// 55,632 + 1,712 = **57,344**.
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
    larger(SERVICE_CHAIN_BYTES, RESTORE_CHAIN_BYTES),
) + INTERRUPT_FRAMES_BYTES;

/// The chain that applies a staged configuration restore.
///
/// **The deepest single thing this firmware does that is not the boot chain**,
/// and the only one whose depth is chosen rather than emergent: sixteen
/// kilobytes of it is `crate::restore::STAGE_MAX_BYTES`, the staged file, read
/// onto the stack because `somfy_migrate::parse_backup` takes one contiguous
/// slice and there is no resumable form of it.
///
/// Walked 2026-08-18 on the ESP32-S3 with the commands at the top of this file,
/// down the C++-backup branch, which is the deeper of the two:
///
/// | | bytes |
/// |---|---|
/// | `main`, `Executor::run`, `run_inner` | 144 |
/// | `TaskStorage<__embassy_main_task>::poll` | 3,856 |
/// | `crate::restore::apply` | 16,704 |
/// | `crate::restore::read_foreign` | 7,968 |
/// | `crate::restore::parse_foreign` | 11,408 |
/// | `crate::restore::map_migration` | 12,448 |
/// | **total** | **52,528** |
///
/// The other branch — this firmware's own `RTSB` container — is 46,416 through
/// `read_own`, `write_regions` and `crate::shades::ShadeStore::store`.
///
/// # Four things had to be true for this to fit, and each was measured
///
/// It began at **149,888 bytes** in one frame and came down in four steps, none
/// of which changed what the code does:
///
/// 1. **It is called from `crate::entry`, not from `crate::start`.** `start`'s
///    own frame is 20,144 and it is *live* while it calls anything, so a
///    restore under it was a 73 KB chain against 66 KB of stack. `start` returns
///    `Booted::Restore` instead, and `entry` applies it and resets — which also
///    means the boot that follows reads the new configuration through the
///    ordinary path rather than a second one.
/// 2. **Both format readers are `#[inline(never)]`**, so `apply` holds one at a
///    time rather than the sum.
/// 3. **The parse and the mapping are `#[inline(never)]` and separate.** A
///    `somfy_migrate::MigrationData` is ~5.6 KB and the importer's room-index
///    table is 2 KB more; composed in one frame they were live together for no
///    reason but the absence of a seam. Worth about twelve kilobytes.
/// 4. **The importer is called through its warning *sink*.**
///    `somfy_config::import::Import` is 36,976 bytes, of which 33,024 is a
///    `heapless::Vec<Warning, 688>`; `ImportedTable` is **3,952**, measured the
///    same way against `thumbv7em-none-eabihf`. The device logs each warning as
///    it is raised and reports a count, so it never needs the list.
///
/// It is a term in the `max` above rather than a comment for the reason
/// [`REQUEST_CHAIN_BYTES`] is: it is the one that moves when the backup format
/// or the importer changes, and the `max` is what notices. It sits **2,880
/// bytes below** [`BOOT_CHAIN_BYTES`], so it does not set the requirement today
/// — and if it ever does, `crate::restore::STAGE_MAX_BYTES` is the dial, at the
/// cost of refusing larger backups with a code that names the number.
///
/// Zero without a web server: nothing can stage a restore in that image, so
/// nothing can apply one.
#[cfg(feature = "http")]
const RESTORE_CHAIN_BYTES: usize = 52_528;
/// See the `http` definition above.
#[cfg(not(feature = "http"))]
const RESTORE_CHAIN_BYTES: usize = 0;

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
/// **This is the division, and it is one number for both chips.**
/// `esp_alloc::heap_allocator!` declares a static array and esp-hal's linker
/// script gives the main stack whatever DRAM is left once the statics are
/// placed, so the heap and the stack are two shares of one fixed quantity —
/// measurably so, and checked rather than assumed: the heap moved by +109,568 on
/// the ESP32-S3 and +96,256 on the ESP32-C3 when this constant was introduced,
/// and each chip's stack fell by exactly that, to the byte, in the relinked ELF.
/// There is no third option and no slack between them; choosing one chooses the
/// other.
///
/// ### Why it is 66,280 and why it must not rise
///
/// It used to be written as `49_592 + 16_688` — a requirement plus a margin —
/// and **both halves of that were wrong while their sum was right.** Nothing
/// available then was unavailable now; only the account of it was wrong, which
/// is why the sum is kept unchanged and every heap figure measured against it
/// stays valid. At today's [`REQUIRED_STACK_BYTES`] of 57,120 the margin this
/// division actually buys is 66,280 − 57,120 = **9,160**, not 16,688.
///
/// ### It was fixed by the ESP32, and the ESP32 is gone
///
/// The figure was chosen because it was the most the *ESP32* could give the
/// stack while still leaving the Wi-Fi driver a heap — that chip was the binding
/// constraint on the whole design, and the reason a "pick the stack first" rule
/// could not be followed all the way down. It was dropped on 2026-08-18 (see the
/// module docs), so the binding constraint is gone with it.
///
/// **The figure is kept unchanged anyway, and that is a judgement rather than an
/// oversight.** Lowering it is the only direction that would buy anything — more
/// heap on both remaining chips — and there is almost nothing there to buy:
/// [`REQUIRED_STACK_BYTES`] is 57,344 and [`STACK_MARGIN_FLOOR_BYTES`] is 8,192,
/// so the floor on this budget is 65,536 and the whole available move is **744
/// bytes**, which does not cross the whole-KiB boundary the division rounds on
/// for either chip. It would change no heap by a byte while pinning a number
/// that a growing call graph moves. Every heap figure ever measured against
/// 66,280 also stays valid, which is worth more than 968 bytes of nothing.
///
/// ### What the difference buys
///
/// 66,280 − [`REQUIRED_STACK_BYTES`] = 8,936 bytes, and
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
/// The actual margin today is 8,936, so this floor is 744 bytes of slack
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
// fails to compile, naming both numbers — which is what the `chip-c3` + `mdns` /
// `sntp` refusals below already do for a different reason, generalised to the
// reason that actually bit.
//
// It is deliberately *not* satisfied by construction: `STACK_BUDGET_BYTES` is
// the DRAM division and `REQUIRED_STACK_BYTES` is what the compiler emitted.
// Neither is defined in terms of the other, so the comparison is a real one.
//
// The two ways out when it fires are both real work and neither is editing this
// line: make the chain shallower — `crate::start_network`'s `#[inline(never)]`
// is what that looks like, and it recovered 18,576 bytes — or move the division
// and pay for it out of the ESP32-C3's 5,796-byte heap slack, which needs
// hardware nobody has.
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
/// # One figure per chip, and what makes that sound
///
/// **This is a per-chip constant describing a per-*configuration* quantity, and
/// the only thing that reconciles those is an invariant: each figure is measured
/// with the largest feature set that chip is *permitted* to build.** A smaller
/// build then leaves the residue on the stack, which is the safe direction and
/// the same reason the division rounds down. A *larger* build would take the
/// residue out of a stack the constant already promised away — silently, and in
/// the direction that overflows.
///
/// So "permitted" cannot be a convention. It is a `compile_error!`: the ESP32-C3
/// refuses `mdns` and `sntp` immediately below, and those refusals are what keep
/// its figure a maximum rather than a sample. **Deleting one of them without
/// re-measuring this row is the same defect as letting the row go stale**, which
/// it has done three times, once refusing to boot.
///
/// | chip | largest permitted set | DRAM |
/// |---|---|---|
/// | ESP32-S3 | `mqtt`, `ui`, `mdns`, `sntp` — everything | 132,260 |
/// | ESP32-C3 | `mqtt`, `ui` (and so `http`); `mdns` and `sntp` refused | 126,864 |
///
/// The **ESP32 was dropped on 2026-08-18** and its row with it; see the module
/// docs for the arithmetic. The historical notes below keep their ESP32 columns
/// because they record measurements that were taken.
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
///
/// **Re-measured 2026-08-18 for the `Origin`/`Host` check and the per-shade
/// command limiter, and this row is the whole reason the check has the shape it
/// has.** Measured the documented way, one worktree built twice:
///
/// | chip | before | after | delta |
/// |---|---|---|---|
/// | ESP32 (`mqtt`) | 123,996 | 123,740 | −256 |
/// | ESP32-S3 (all) | 136,020 | 135,068 | −952 |
/// | ESP32-C3 (all) | 122,816 | 121,856 | −960 |
///
/// The ESP32 has no web server, so its 256 bytes are the limiter alone — a
/// `[u32; 32]` table in the state task's future, which the generator lays out
/// twice. The other two pay a further ~700, spread across roughly ninety
/// eight-byte constant anchors: one per guarded handler's rejection path, with
/// nothing large enough to name.
///
/// **What this bought, and it is the measurement that chose the mechanism.**
/// The obvious way to check every request is `picoserve::Router::layer`, which
/// wraps the whole router and so cannot be forgotten by a route added later.
/// Measured on the ESP32-S3, `firmware::api::connection::POOL` went from 67,840
/// to 73,888 — **6,048 bytes**, of which 4,896 is an empty pass-through layer
/// that does nothing at all, because `call_layer` is an `async fn` holding the
/// entire inner router's future across its own await. The same check written as
/// a `picoserve::extract::FromRequestParts` extractor on each handler left that
/// figure at 67,840 exactly. 6,048 bytes would have taken the ESP32-C3's heap
/// to 48 KiB against a 54,424-byte announcement peak — a board that panics
/// part-way through publishing its discovery configs. `crate::api::origin`
/// carries the table and what the shape costs in exchange.
///
/// **The limiter's table is 128 bytes rather than 256 for this row's sake, and
/// it is worth one kilobyte on the chip that can least afford it.** At `u64`
/// milliseconds the ESP32 measured 66,140 bytes of `.stack` against a
/// 66,280-byte budget, which rounds its heap down to 55 KiB; at `u32` seconds it
/// measures 66,396 and keeps 56 KiB. See `somfy_tasks::CommandLimiter`.
///
/// **Re-measured 2026-08-18 for over-the-air updates, and this is the largest
/// single bill any change has presented to this row.** Measured the documented
/// way, one worktree built twice:
///
/// | chip | before | after | delta |
/// |---|---|---|---|
/// | ESP32 (`mqtt`) | 123,732 | 123,284 | −448 |
/// | ESP32-S3 (all) | 135,060 | 132,260 | −2,800 |
/// | ESP32-C3 (all) | 121,848 | 119,064 | −2,784 |
///
/// Attributed against the linked images rather than estimated:
///
/// - **1,440 bytes** in `firmware::api::connection::POOL`, 68,224 → 69,664 —
///   360 per connection task, for the `POST /api/v1/ota` route. `picoserve`'s
///   router is a type per route, so every path is a variant of the future each
///   of the four tasks holds statically. This is the unavoidable half.
/// - **264 bytes** for `firmware::ota::upload::PAGES`, the one page buffer the
///   megabyte crosses tasks in. Deliberately 256 bytes rather than 512 or 4,096
///   — see `crate::ota::PAGE_BYTES`, which argues the size against *this* row.
/// - **368 bytes** in `firmware::tasks::state::POOL`, 15,936 → 16,304, for the
///   upload session and its image verifier. That one is a choice: it could have
///   been a `static`, which would have cost the same bytes *here* instead of on
///   a stack that has room.
/// - The rest — about 720 on the two chips with a web server, and all 432 of
///   the ESP32's — is small statics and constant anchors, the same residue the
///   `Origin`/`Host` row above describes. The `ota` module's own statics total
///   **eleven bytes**, and its attempt counter is in RTC memory rather than
///   DRAM, so it costs this row nothing at all.
///
/// **864 bytes were given back before this was accepted**, and the shape of it
/// is worth keeping: the boot self-test was first written as an
/// `#[embassy_executor::task]`, whose future is a `static` sized whether or not
/// it is ever spawned. Almost all of it was the `crate::rpc::Request` its
/// confirm call held. Driving it from the state task's existing ticker instead
/// costs the executor's stack for the length of a call and nothing when it is
/// not running — see `crate::ota::tick_self_test`.
///
/// **Re-measured 2026-08-18 when the ESP32 was dropped, and the ESP32-C3's row
/// moved for a reason that is about its feature set rather than about the
/// chip.** Both remaining figures were read the documented way on that day, and
/// the ESP32-S3's is confirmed twice over: `readelf -S` gives `.stack` = 66,724,
/// and a live board printed `stack: 66724 bytes available` — the same number.
///
/// The C3 was measured across its whole feature space, because the reason its
/// full build did not fit had been misattributed to the web UI:
///
/// | ESP32-C3 configuration | `.stack` | DRAM | heap | vs [`WIFI_PEAK_BYTES`] |
/// |---|---|---|---|---|
/// | `mqtt`+`ui`+`mdns`+`sntp` | 66,840 | 119,064 | 52,224 | **−2,396** |
/// | `mqtt`+`mdns`+`sntp`, no `ui` | 67,080 | 119,304 | 52,224 | **−2,396** |
/// | `mqtt`+`ui`+`sntp`, no `mdns` | 71,752 | 123,976 | 57,344 | +2,724 |
/// | **`mqtt`+`ui`, no `mdns`/`sntp`** | 74,632 | 126,856 | **60,416** | **+5,796** |
/// | `mqtt` alone | 163,168 | 215,392 | 148,480 | +93,860 |
///
/// Those six rows were all read at the heap the *old* constant gave (52,224), so
/// they are directly comparable with each other, which is what the table is for.
/// **The chosen row then had to be re-measured against its own heap**, and it
/// moved by 8 bytes: at a 60,416-byte heap the C3's `.stack` links to 66,448
/// rather than 74,632 − 8,192 = 66,440, so the constant below is **126,864**.
/// The eight bytes are the linker's alignment response to a heap 8,192 bytes
/// larger, and they are worth a sentence because they are the whole reason this
/// row is *measured* rather than computed: `DRAM = .stack + heap`, and `heap` is
/// derived from `DRAM`, so the constant is a fixpoint and only a second build
/// proves you have reached it. 126,864 − 66,280 = 60,584, which still rounds
/// down to the same 60,416, so the image does not move again — checked.
///
/// **`ui` costs 240 bytes of DRAM. `mdns` costs 4,672 and `sntp` 2,880**, and
/// the two are additive to the byte (4,672 + 2,880 = 7,792 = 126,856 − 119,064).
/// The intuition that the UI is what does not fit is wrong twice over: the
/// connection tasks and `picoserve`'s monomorphised router come with **`http`**,
/// which every row above except the last still has, and `ui` adds only
/// `include_bytes!` assets, which are `.rodata` in flash rather than DRAM.
///
/// So the C3 ships the fourth row — the web UI, the REST API and the update
/// route, reached by IP rather than by name — and `mdns` and `sntp` are refused
/// below. 60,416 is 5,796 above the worst announcement peak ever measured, about
/// **2.9× that peak's own ~2,000-byte boot-to-boot spread**, where the full
/// build sat 2,396 *below* it.
///
/// **The number to distrust, if you are the first person to boot a C3:**
/// [`WIFI_PEAK_BYTES`] is an **ESP32-S3** measurement. No C3 has ever run this
/// firmware, so its own announcement peak has never been observed — a different
/// Wi-Fi blob on a different core could want more or less. +5,796 against
/// another chip's peak is a great deal better than −2,396 against it, and it is
/// not the same as knowing. Watch `heap: session announced` on that board before
/// trusting any of this.
///
/// **Re-measured 2026-08-18 for the image digest, and this is the first change
/// that moved the ESP32-S3 down a whole kilobyte for a few hundred bytes of
/// need.** `somfy_ota::image::Verifier` gained a `Sha256` and a thirty-two byte
/// delay line — about 150 bytes of struct — and that struct is a field of the
/// state task's future, which embassy sizes as a `static`. Measured the
/// documented way, one worktree built twice:
///
/// | chip | before | after | delta |
/// |---|---|---|---|
/// | ESP32-S3 (all) | 128,020 | 127,692 | −328 |
/// | ESP32-C3 (`mqtt`+`ui`) | 122,552 | 122,216 | −336 |
///
/// Attributed: **376 bytes** of it is `firmware::tasks::state::POOL`, 16,304 →
/// 16,680, read off the linked image with `nm --size-sort`. That is two and a
/// half bytes of DRAM per byte of struct, which is the future-layout tax this
/// file has priced once before, and the rest is a small give-back elsewhere.
///
/// **What it cost is not 328 bytes, and the difference is the point.** The heap
/// is a floor division by 1,024, so what matters is which side of a kilobyte
/// boundary the subtraction lands on:
///
/// | chip | heap before | heap after | vs [`WIFI_PEAK_BYTES`] | to the next cliff |
/// |---|---|---|---|---|
/// | ESP32-S3 | 61,440 | **60,416** | +5,796 | 996 bytes |
/// | ESP32-C3 | 55,296 | 55,296 | +676 | 640 bytes |
///
/// The ESP32-C3 does not move: 336 bytes came out of a band that had 976 of
/// slack. The ESP32-S3 had **300**, so 328 bytes cost it a kilobyte, and its
/// clearance over the worst announcement peak ever measured falls from 6,820 to
/// 5,796 — still **2.9× that peak's own ~2,000-byte spread**, and 8.6× what the
/// ESP32-C3 ships with today.
///
/// **Twenty-eight bytes would have kept the old band, and they were not taken.**
/// The obvious source is `Verifier`'s three length counters, which index
/// buffers of 112, 24 and 32 bytes and are `usize` for no reason. Narrowing
/// them to `u8` was written, and it needs fifteen `usize::from` casts through
/// the walk — a permanent cost in the most-read function in that crate, to sit
/// 32 bytes above a cliff that the next feature crosses anyway. The band was
/// bought instead: there are now 996 bytes of slack on the ESP32-S3, which is
/// where the next few hundred bytes should go before this is re-read.
#[cfg(feature = "chip-s3")]
const DRAM_FOR_STACK_AND_HEAP: usize = 127_660;
/// See the `chip-s3` definition above.
#[cfg(feature = "chip-c3")]
const DRAM_FOR_STACK_AND_HEAP: usize = 122_216;

// **The ESP32-C3 does not have the DRAM for the mDNS responder or the SNTP
// client on top of the web server, and these say so at compile time.**
//
// Measured 2026-08-18, in the table on `DRAM_FOR_STACK_AND_HEAP` above: with
// both on, that chip's heap is 52,224 against a 54,620-byte announcement peak.
// It clears the driver's resting working set by 4,760, so it would associate and
// connect — and then exhaust the heap part-way through publishing its retained
// discovery configs, which `esp-alloc` answers with a null, which reaches
// `handle_alloc_error`, which panics. A board that reboots while announcing
// looks like a broker fault and is not.
//
// **Why a refusal here when `warn_if_tight` argues for a warning.** That
// function's argument is against refusing a *chip*, because a compile-time
// refusal would take it out of the matrix at the moment the matrix is what would
// catch the problem. These refuse two *features* on one chip and leave it in the
// matrix for everything else, so that argument does not reach them. What does
// reach them is the invariant above: `DRAM_FOR_STACK_AND_HEAP` is one figure per
// chip, sound only because it is measured at the largest set that chip can
// build. Downgrade either of these to a warning and the C3 can build an image
// larger than its own constant was measured on — which is the unsafe direction,
// and the direction this row has failed in three times.
//
// The ESP32-S3 carries both, and with **5,796 bytes** of heap to spare over the
// same peak. That figure said 10,916 until 2026-08-18 and had been wrong for
// two revisions of `DRAM_FOR_STACK_AND_HEAP` — 10,916 is the spare of a 64 KiB
// heap, which this chip last had before the web server landed. It is corrected
// here rather than quietly because it is the exact failure this file's own
// "loses things in merges" section is about: a number in prose beside a
// constant, with nothing checking that the two still agree.
#[cfg(all(feature = "chip-c3", feature = "mdns"))]
compile_error!(
    "the ESP32-C3 does not have the DRAM for the mDNS responder as well as the web server: it \
     costs 4,672 bytes, which takes this chip's Wi-Fi heap from 60,416 to 55,296 against a \
     54,620-byte announcement peak — inside that peak's own ~2,000-byte spread. Build it with \
     `--no-default-features --features chip-c3,mqtt,ui` and reach the device by IP, or use an \
     ESP32-S3. See `heap::DRAM_FOR_STACK_AND_HEAP` for the measurement."
);
#[cfg(all(feature = "chip-c3", feature = "sntp"))]
compile_error!(
    "the ESP32-C3 does not have the DRAM for the SNTP client as well as the web server: it \
     costs 2,880 bytes of this chip's Wi-Fi heap, and with `mdns` as well the heap falls 2,396 \
     bytes BELOW the 54,620-byte announcement peak. Build it with `--no-default-features \
     --features chip-c3,mqtt,ui` — that image has no wall clock at all — or use an ESP32-S3. \
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
/// | chip | DRAM to divide | heap | stack left | spare over [`REQUIRED_STACK_BYTES`] | vs [`WIFI_PEAK_BYTES`] |
/// |---|---|---|---|---|---|
/// | ESP32-S3 | 129,324 | 61 KiB = 62,464 | 66,860 | 9,740 | +7,844 |
/// | ESP32-C3 | 123,872 | 56 KiB = 57,344 | 66,528 | 9,408 | +2,724 |
///
/// **Re-measured 2026-08-18 for the diagnostics and backup screens, and this is
/// the row going stale being caught rather than found later.** Both figures fell
/// — the ESP32-S3 by 2,936 and the ESP32-C3 by 2,992 — and the cause is entirely
/// the connection task futures: four new routes, two new response buffers, and
/// `crate::restore`'s share of the same futures. Nothing of the two screens'
/// own state is in DRAM at all: `crate::diag`'s log ring and panic record are
/// 4,308 bytes of **RTC-fast** memory, which the linker gives its own 8 KiB
/// region outside `dram_seg`, and the staged-restore buffer is boot stack.
///
/// **It would have been worse by 4,096 and the ESP32-C3 would not have shipped.**
/// `api::TCP_RX_BYTES` and `api::TCP_TX_BYTES` were halved to pay for the two
/// screens, which is where that 4,096 came from; without it the C3's heap lands
/// at 53,248, *below* the announcement peak. That constant carries the trade and
/// what it costs in round trips.
///
/// **The ESP32-C3 is now the tightest row this matrix has ever shipped**, at
/// +2,724 against a peak whose own boot-to-boot spread is about 2,000. Three
/// things are worth saying about that figure rather than one:
///
/// - It is **above** the spread, where the plain ESP32 was dropped at +1,700,
///   which is inside it.
/// - [`WIFI_PEAK_BYTES`] is an **ESP32-S3** measurement taken on a build with
///   `mdns` and `sntp` in it, and the C3 refuses both. Its own peak has never
///   been observed because no C3 has ever booted this firmware.
/// - [`warn_if_tight`] prints the comparison at boot, so the first person to
///   boot a C3 settles it in one line rather than in an argument.
///
/// **Both rows moved on 2026-08-18**, and for different reasons. The ESP32-S3's
/// is the same image it has always been, re-read after the ESP32 was dropped and
/// unchanged by that. The ESP32-C3's is a *different configuration*: `mdns` and
/// `sntp` are now refused on that chip, which is what takes its heap from 52,224
/// — 2,396 bytes below the announcement peak — to 60,416, which is 5,796 above
/// it. See [`DRAM_FOR_STACK_AND_HEAP`] for the whole feature-by-feature table
/// and for why the web UI was *not* the thing to cut.
///
/// **Neither chip trips [`warn_if_tight`] now**, which is the first time that
/// has been true since it was written; the C3 was the reason it exists. It is
/// kept, and the reason is in its own doc comment: the figure it compares
/// against is one chip's reading, and the C3's own peak is still unmeasured.
///
/// The available lever, recorded rather than taken because it spends the
/// operator's page-load time and that is not this change's to spend:
/// `api::TCP_TX_BYTES` at 512 instead of 1,024 returns 2,048 bytes of DRAM. Its
/// own documentation calls it the figure to raise if the UI feels slow, so this
/// is the trade in the other direction — roughly twenty extra round trips on a
/// 21 KB script, once per page load and never on a reload, since the assets
/// answer `304`.
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
///   reaching the setter costs (`esp-radio/unstable`, plus — on the ESP32, which
///   this firmware no longer supports — an ADC2 claim in `esp_radio`'s `init`
///   that panics if esp-hal holds it). At 163 KiB
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
/// It is one chip's number used for both, which is the honest limit of it: the
/// ESP32-C3 has never been booted, so its driver's resting set has never been
/// observed either.
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
        crate::logln!(
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
/// **The ESP32-C3 is the reason it exists**, and as of 2026-08-18 no chip in the
/// matrix trips it: the C3 refuses `mdns` and `sntp`, which takes its heap to
/// 60,416 — 5,796 above [`WIFI_PEAK_BYTES`], about 2.9× that peak's own spread —
/// and the ESP32, whose 1,700-byte margin was the other case, was dropped.
///
/// **It is kept anyway, and not as decoration.** [`RADIO_HEAP_BYTES`] is a
/// subtraction, not a chosen number: every static added anywhere in the image
/// comes out of [`DRAM_FOR_STACK_AND_HEAP`], and the day that row is re-measured
/// after a Plan's worth of new buffers the heap shrinks by exactly as much, with
/// no diff to review. This is the line that would say so. And the figure it
/// compares against is still an **ESP32-S3** reading — the C3's own announcement
/// peak has never been observed, so the margin above is against another chip's
/// number.
///
/// Not a refusal, and not a `const` assertion, for the same reason
/// [`warn_if_undersized`] is neither: the peak is one chip's measurement and a
/// compile-time refusal would take the affected chip out of the matrix that
/// would catch the problem. The two `compile_error!`s beside
/// [`DRAM_FOR_STACK_AND_HEAP`] are not a counter-example — they refuse two
/// *features* on one chip and leave it in the matrix for everything else, and
/// they exist to keep that constant a maximum rather than to price a heap.
#[allow(dead_code, reason = "see the allow on `RADIO_HEAP_BYTES`")]
fn warn_if_tight() {
    // Only when the heap clears the resting set, so this never doubles up on
    // the harder line above.
    if RADIO_HEAP_BYTES < WIFI_WORKING_SET_BYTES
        || RADIO_HEAP_BYTES >= WIFI_PEAK_BYTES + PEAK_NOISE_BYTES
    {
        return;
    }
    // **Two lines rather than one, because the subtraction changes sign.** A
    // heap below the peak used to be unrepresentable here and the message
    // computed `RADIO_HEAP_BYTES - WIFI_PEAK_BYTES` unguarded. On the ESP32-C3,
    // once the firmware-upload route landed, that became a `usize` underflow —
    // and both operands are `const`, so the compiler evaluated it and refused
    // the build outright with `attempt to compute 52224_usize - 54620_usize`.
    // The condition this line reports is exactly the one it would not compile
    // for, which is a good way round for it to have been found.
    //
    // `saturating_sub` on both branches rather than a plain `-` on the branch
    // that is provably safe: the guard above is the only thing making it safe,
    // and a later edit to the guard would put the same landmine back.
    if RADIO_HEAP_BYTES < WIFI_PEAK_BYTES {
        crate::logln!(
            "heap: {} bytes is {} BELOW the worst announcement peak ever measured ({}), \
             though still {} above the driver's resting working set. This board is expected \
             to associate and to connect to a broker; what is in doubt is the burst of \
             retained discovery configs, which is where the peak was measured. Watch \
             `heap: session announced` — if it approaches the total, an announcement can \
             exhaust the heap and reset the board. The peak is an ESP32-S3 measurement and \
             has never been taken on this chip. See crates/firmware/src/heap.rs.",
            RADIO_HEAP_BYTES,
            WIFI_PEAK_BYTES.saturating_sub(RADIO_HEAP_BYTES),
            WIFI_PEAK_BYTES,
            RADIO_HEAP_BYTES.saturating_sub(WIFI_WORKING_SET_BYTES),
        );
        return;
    }
    crate::logln!(
        "heap: {} bytes leaves {} above the worst announcement peak ever measured \
         ({}), which is inside the {}-byte spread that peak showed between boots. \
         Watch `heap: session announced` on this board — if it lands near the \
         total, an announcement can exhaust the heap and reset it. See \
         crates/firmware/src/heap.rs.",
        RADIO_HEAP_BYTES,
        RADIO_HEAP_BYTES.saturating_sub(WIFI_PEAK_BYTES),
        WIFI_PEAK_BYTES,
        PEAK_NOISE_BYTES,
    );
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
    crate::logln!(
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

/// The heap the allocator actually installed, in bytes.
///
/// [`RADIO_HEAP_BYTES`] is what this build asked for and this is what it got.
/// They agree today; reporting the measured one means a diagnostics screen
/// cannot disagree with a serial console, and means the day they stop agreeing
/// is visible rather than inferred.
#[allow(
    dead_code,
    reason = "not called by every binary that includes this file by path"
)]
pub fn size_bytes() -> usize {
    esp_alloc::HEAP.stats().size
}

/// Bytes of heap currently allocated.
///
/// The complement of [`free_bytes`], from the same counters, because a screen
/// reads better as "used of size" than as "free of size" and computing one from
/// the other at the call site is where an off-by-one lives.
#[allow(
    dead_code,
    reason = "not called by every binary that includes this file by path"
)]
pub fn used_bytes() -> usize {
    esp_alloc::HEAP.stats().current_usage
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
