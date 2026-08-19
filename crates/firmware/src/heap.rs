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
//! One pool, two shares, and there is no third option: esp-hal's linker script
//! gives the main stack whatever DRAM is left once the statics are placed, so a
//! byte spent here is a byte `main::check_stack_headroom` no longer has.
//!
//! **The division is made by the linker, not by a constant in this file, and
//! that changed on 2026-08-19.** [`STACK_BUDGET_BYTES`] fixes what the stack
//! keeps; `crates/firmware/build.rs` reserves exactly that at the top of DRAM
//! and gives the heap a `.heap` output section running from the end of the
//! statics up to it; [`heap_region`] reads that section's two bounds at boot.
//! What this file used to carry instead — `DRAM_FOR_STACK_AND_HEAP`, one
//! hand-measured figure per chip — needed re-measuring after six consecutive
//! merges, was twice wrong in the direction that refuses to boot, and once
//! actually stopped a board starting. [`heap_region`] carries the full account
//! of why vigilance was not going to fix that.
//!
//! ## Why the ESP32 and the ESP32-C3 are no longer among them
//!
//! The ESP32 was dropped 2026-08-18 and the ESP32-C3 on 2026-08-19, and in both
//! cases it is the removal of an unverified claim rather than a reduction in
//! capability: **neither chip had ever booted this firmware.** The ESP32 was
//! already excluded from the web server by a `compile_error!` here, and its one
//! buildable configuration — `mqtt` alone — measured 123,284 bytes of DRAM, so
//! its heap was 56,320 against a [`WIFI_PEAK_BYTES`] of 54,620: **+1,700, inside
//! that peak's own boot-to-boot spread**, with no smaller configuration left to
//! retreat to. The ESP32-C3 was the same judgement one step later. It had
//! already needed three accommodations to stay — two `compile_error!`s refusing
//! it `mdns` and `sntp`, and halved TCP buffers — and its shipping build cleared
//! the same peak by **676 bytes**, against a peak measured on a *different*
//! chip. A margin inside its own noise is a coincidence with a good track
//! record, not a fit. `docs/provenance.md` carries both sets of arithmetic next
//! to the ESP32-S2's, dropped on 2026-08-17 for the same reason.
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
//! Four times now. Two branches each added statics to the image and each
//! re-measured `DRAM_FOR_STACK_AND_HEAP` against a tree without the other;
//! resolving by taking one side kept a figure correct for neither, and the board
//! refused to boot — caught, loudly, by `crate::check_stack_headroom`. A second
//! resolution dropped [`SERVICE_CHAIN_BYTES`] from the `max`, which was noticed
//! and restored in a commit that says so in its subject.
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
//! **The fourth is the largest and it was on `main`.** The merge at `1add252`
//! resolved this file by taking one parent's copy whole, and with it went
//! everything the other parent had added: [`RESTORE_CHAIN_BYTES`], an entire
//! term of the `max` in [`REQUIRED_STACK_BYTES`]; 224 bytes of
//! [`BOOT_CHAIN_BYTES`], reverted from 55,632 to 55,408 in the direction that
//! boot-loops; four `crate::logln!` call sites reverted to `esp_println`, so
//! that the heap lines stopped reaching the diagnostics ring; and
//! [`size_bytes`] and [`used_bytes`]. **Only the last of those five stopped the
//! crate compiling** — `api::routes` calls them — which is exactly why the other
//! four went unnoticed, and it is the same lesson one size larger. All five are
//! restored here, by the prescribed method: this file diffed in full against
//! both parents of the merge.
//!
//! **And it is the last time this particular failure can happen to the DRAM
//! figure**, because there is no longer a figure. See [`heap_region`].

// How every stack figure below is read off a linked image. The commands are
// here because these are the rows that go stale — one of them already did, and
// the cost was a boot loop on the only hardware that exists.
//
//     # **No `-Tlinkall.x`.** This recipe used to carry one and it now fails the
//     # link outright — "redefinition of memory region alias `ROTEXT`" — because
//     # `build.rs` passes `-Tsomfy-link.x`, which includes linkall.x's lines
//     # itself. `.cargo/config.toml` says the same thing from the other side.
//     # RUSTFLAGS replaces `build.rustflags`, not the link args a build script
//     # emits, so nothing else needs restating here.
//     RUSTFLAGS="-Zemit-stack-sizes" \
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
///   instead. See [`heap_region`].
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
///
/// **Both of the paragraphs above were lost in a merge and are restored here**
/// (2026-08-19), along with [`RESTORE_CHAIN_BYTES`] — the resolution took the
/// other parent's copy of this file whole. The constant was reverted to 55,408
/// with it, which is 224 bytes short of what the image needs. See the module
/// docs.
///
/// **The ESP32 was dropped on 2026-08-18 and this figure does not move**, which
/// is the one thing worth checking when a chip leaves: it was the *smaller* of
/// the two Xtensa columns above (54,832 against the ESP32-S3's 55,408), so the
/// `max` was never its. The tables above keep their ESP32 columns because they
/// are a record of measurements that were taken, not a claim about what is
/// built today — and because the repeated finding they carry is that this row
/// goes stale, which is about the reading rather than about the chip.
///
/// **Re-read 2026-08-19 while adding the calibration entity, and it had gone
/// stale a fifth time — by 720 bytes, again in the direction that boot-loops.**
/// Walked on the ESP32-S3 with the commands at the top of this file (whose
/// recipe was itself stale and is corrected there — the `-Tlinkall.x` it carried
/// now fails the link outright):
///
/// | | recorded | measured |
/// |---|---|---|
/// | `main`, `Executor::run`, `run_inner` | 144 | 144 |
/// | `TaskStorage<__embassy_main_task>::poll` | 3,856 | 3,856 |
/// | [`crate::start`] | 20,160 | 20,304 |
/// | [`crate::tasks::state`] | 15,728 | 16,016 |
/// | `UninitCell::write_in_place` | 15,744 | 16,032 |
/// | **total** | **55,632** | **56,352** |
///
/// **None of it is the calibration entity's**, and that was checked rather than
/// assumed: the same walk was run on the parent commit with this branch's
/// changes stashed, and all five frames read *identically*. What the entity does
/// add is 16 bytes on `Inventory::snapshot` (1,376 → 1,392) for the extra field
/// per shade, and `snapshot` is a **sibling** of the state task under
/// [`crate::start`] rather than a frame beneath it — 144 + 3,856 + 20,304 +
/// 1,392 = 25,696, about 30 KB clear of this chain.
///
/// So this is the fifth consecutive reading to find the constant short, and the
/// fourth to find it short by a number larger than the whole
/// [`INTERRUPT_FRAMES_BYTES`] allowance. The live board's own
/// `crate::stack_used` corroborates it exactly: it reports a high-water of
/// **56,344**, which is eight bytes under the chain measured here and 712 bytes
/// *over* the figure this constant claimed — so the "1,000 bytes of the
/// requirement unspent" that boot line prints was measured against a number that
/// was already wrong, and the true slack was −720 into the interrupt allowance.
///
/// **What it costs to be honest about it.** [`REQUIRED_STACK_BYTES`] becomes
/// 56,352 + 1,712 = **58,064**, and the compile-time gate below wants
/// `STACK_BUDGET_BYTES` ≥ 58,064 + [`STACK_MARGIN_FLOOR_BYTES`] = 66,256 against
/// a budget of 66,280. **It fits by 24 bytes.** That is not comfortable and it
/// should not be read as comfortable: the next thing to deepen this chain fails
/// the build, and the two ways out are the ones named on the gate — make the
/// chain shallower, or lower `STACK_BUDGET_BYTES` and give the difference to the
/// heap, which has room now that the ESP32 and the ESP32-C3 are gone.
const BOOT_CHAIN_BYTES: usize = 56_352;

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
/// four connection task futures. See [`heap_region`], which is where that shows
/// up and where it was paid for.
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
/// Checked at run time by `crate::check_stack_headroom` as well as at compile
/// time, and both are worth keeping now that they no longer say the same thing.
/// The compile-time gate below compares this against [`STACK_BUDGET_BYTES`],
/// which since 2026-08-19 is what the linker *reserves*; the boot check compares
/// it against `_stack_start_cpu0 - _stack_end_cpu0`, which is what the linker
/// actually left. Those agree unless the memory map is not the one this crate
/// builds against — which is the case `crate::stack_region` exists to notice
/// rather than assume.
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
/// or the importer changes, and the `max` is what notices. It sits **3,104
/// bytes below** [`BOOT_CHAIN_BYTES`], so it does not set the requirement today
/// — and if it ever does, `crate::restore::STAGE_MAX_BYTES` is the dial, at the
/// cost of refusing larger backups with a code that names the number.
///
/// **This constant was dropped whole by a merge and is restored here**
/// (2026-08-19). Nothing failed while it was gone, which is what a lost term of
/// a `max` looks like: the `max` goes on producing the right answer until one of
/// the terms it no longer has grows. See the module docs.
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

// The main stack's budget, from the one file that defines it. `build.rs`
// includes the same file to emit the linker fragment that reserves exactly
// these bytes; this includes it for the compile-time gate below. Two copies of
// the figure — one in Rust and one in a linker script — would be two numbers
// that can disagree, which is the failure this whole file was rewritten for.
include!("../stack_budget.rs");

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

// **The first of the two gates that can stop a build, and the one about code.**
//
// A configuration whose deepest chain has grown past what the division leaves
// fails to compile, naming both numbers.
//
// It is deliberately *not* satisfied by construction: `STACK_BUDGET_BYTES` is
// what the linker reserves and `REQUIRED_STACK_BYTES` is what the compiler
// emitted. Neither is defined in terms of the other, so the comparison is real.
//
// **The second gate is in `build.rs` and is about size**: an image whose statics
// no longer leave `STACK_BUDGET_BYTES` fails the `ASSERT` in the linker fragment
// that reserves it. Between them they cover the two ways this division stops
// working — the chain got deeper, or the image got fatter — and neither is a
// number anybody maintains.
//
// The two ways out when this one fires are both real work and neither is editing
// this line: make the chain shallower — `crate::start_network`'s
// `#[inline(never)]` is what that looks like, and it recovered 18,576 bytes — or
// lower the budget, which spends heap the announcement burst is measured against
// and which `stack_budget.rs` argues is worth 744 bytes at most.
const _: () = assert!(
    STACK_BUDGET_BYTES >= REQUIRED_STACK_BYTES + STACK_MARGIN_FLOOR_BYTES,
    "the deepest stack chain in this configuration no longer fits the DRAM \
     division: see heap::REQUIRED_STACK_BYTES for what it needs, \
     heap::STACK_BUDGET_BYTES for what the division leaves, and \
     heap::STACK_MARGIN_FLOOR_BYTES for the reserve that must survive between \
     them. Re-read the chains from a linked ELF before changing any of the \
     three — the commands are in this file.",
);

/// Where the linker left the heap, and how big it is.
///
/// # This replaced a constant, and the constant is the point
///
/// Until 2026-08-19 this file carried `DRAM_FOR_STACK_AND_HEAP`, one
/// hand-measured figure per chip for "the DRAM this chip has to divide", from
/// which the heap was a subtraction. **It needed re-measuring after six
/// consecutive merges.** Twice it was wrong in the direction that refuses to
/// boot; once a board actually refused, printing
/// `StackTooSmall { available: 53516, required: 55792 }`; and one of the merges
/// that broke it also silently dropped a `compile_error!` from this same file,
/// which nobody noticed for days because a guard's whole job is to be quiet.
///
/// Three properties made it un-maintainable, and none of them is fixed by
/// vigilance:
///
/// - **It was a property of the whole linked image** — total DRAM minus every
///   static — so a change anywhere moved it.
/// - **It was circular.** It decided the heap's size, and the heap was the
///   largest static in the image, so changing it changed the thing being
///   measured. It was documented as a fixpoint needing a second build.
/// - **Two branches could each measure it correctly and produce a merge for
///   which neither figure was right.** A conflict there could only be settled
///   by measuring again, which no merge tool will do.
///
/// # What is there instead
///
/// The linker already knows the answer, and always did:
/// `esp-hal-1.1.2/ld/sections/stack.x` gives `.stack` everything left in
/// `RWDATA` once the statics are placed. `crates/firmware/build.rs` now emits
/// one more output section immediately before it — `.heap`, running from the
/// end of the statics to exactly [`STACK_BUDGET_BYTES`] below the top of DRAM —
/// so the division is made by the linker, on the real image, on every build.
/// This function reads the two symbols that section defines.
///
/// **There is no number left to go stale, and a merge cannot produce a wrong
/// one, because there is no number to merge.** What is hand-written is
/// [`STACK_BUDGET_BYTES`], and that is a *policy* figure — what the stack
/// keeps — rather than a measurement of an image, so it does not move when the
/// image does.
///
/// # What it also fixed, which was not the goal
///
/// The heap used to be a `static` array placed among the other statics, so
/// `_stack_end_cpu0` sat *below* it and everything that reads those symbols was
/// measuring a region part of which the allocator owned: esp-hal's stack-guard
/// word and its hardware watchpoint (60 bytes above `_stack_end_cpu0`, and
/// re-armed at that address by `esp-rtos`'s `hw-task-overflow-detection` on
/// every switch back to the main task), esp-hal's `ensure_stack_pointer_in_range`,
/// esp-rtos's own `stack-pointer-range-check`, and [`crate::stack_region`]. They
/// now all agree, and the watchpoint sits 60 bytes above the true floor rather
/// than 60 bytes above a heap.
///
/// # And it moved the DRAM ceiling to link time
///
/// An image whose statics grow past what is left after the budget no longer
/// links: `build.rs`'s fragment ends in an `ASSERT` naming both numbers. That
/// is the failure the old constant used to express as a board that would not
/// start.
fn heap_region() -> (usize, usize) {
    unsafe extern "C" {
        static _somfy_heap_start: u8;
        static _somfy_heap_end: u8;
    }
    // Neither is dereferenced — only the addresses are taken, which is what
    // makes this safe and why no `unsafe` block is needed for it.
    (
        (&raw const _somfy_heap_start) as usize,
        (&raw const _somfy_heap_end) as usize,
    )
}

/// Bytes of DRAM this image has for the Wi-Fi driver's heap.
///
/// Whatever [`STACK_BUDGET_BYTES`] and this image's statics left, read out of
/// the linker's own symbols. A function rather than a constant precisely
/// because nothing in this file may claim to know it — see [`heap_region`].
///
/// **It is not rounded**, unlike the constant it replaced. That rounding went
/// *down* to a whole KiB so the residue landed on the stack, "the side that
/// fails silently"; it exists to protect a stack whose size was a subtraction.
/// The stack's size is now reserved by the linker before the heap is given
/// anything, so the residue has nowhere to fail and the heap keeps it — worth
/// somewhere under a kilobyte, and worth saying rather than leaving as a
/// difference somebody spots between two boot lines.
///
/// # What each configuration actually gets
///
/// Read on 2026-08-19 off the five linked release images the CI matrix builds,
/// with `readelf -S | grep '\.heap '`. `.stack` is **66,280 in every one of
/// them** — that is the point of the rewrite — and the heap absorbs the whole
/// difference:
///
/// | build | heap |
/// |---|---|
/// | `mqtt` + `ui` + `mdns` + `sntp` — what a board runs | **61,336** |
/// | `http` alone | 90,904 |
/// | `mdns` alone (pulls `http`) | 85,976 |
/// | `sntp` alone (pulls nothing) | 178,936 |
/// | radio only | 181,832 |
///
/// **The constant this replaced could not express that table.** It was one
/// figure per chip, measured at the largest feature set that chip was permitted
/// to build, and a smaller build left the residue *unused on the stack* — the
/// safe direction, and a wasteful one.
///
/// # Where the shipping figure goes
///
/// Measured the way the header of this file describes — one worktree built
/// twice per change, not by subtracting two rows of the table above, which
/// conflates features:
///
/// - **The web server costs about 70,000 bytes.** Four connection tasks are the
///   bulk of it (`api::HTTP_TASKS` × a 16,960-byte future, which is
///   `picoserve`'s router recursion) and their buffers are 14,336.
/// - **`ui` costs 240 bytes**, which is the intuition-defeating one: the
///   connection tasks and the monomorphised router come with **`http`**, and
///   `ui` adds only `include_bytes!` assets, which are `.rodata` in flash.
/// - **`mdns` costs 4,672 and `sntp` 2,880**, additive to the byte.
///
/// What was tried and rejected, so it is not tried again:
/// `picoserve::response::Json` streams instead of holding a buffer, but its
/// `JsonStream` keeps the value *and* a serializer state live across the
/// write — the connection future grew to 18,904 bytes per task, 7,776 across
/// the four, against the 2,688 the wider fixed buffer costs. See
/// `api::routes::JsonBody`.
///
/// The lever that is already pulled: `api::TCP_RX_BYTES` and `TCP_TX_BYTES` are
/// 512 rather than 1,024, worth 4,096 bytes, and that module argues why they
/// stay there now that the chip which forced it is gone. The one still on the
/// shelf is `TOPIC_CAPACITY` at 160 rather than 256, since two collected plans
/// hold twenty `Step`s between them.
#[allow(
    dead_code,
    reason = "not used by `tx-check`, which includes this file by path and \
              installs only the scheduler's heap"
)]
pub fn radio_heap_bytes() -> usize {
    let (start, end) = heap_region();
    end.saturating_sub(start)
}

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
#[allow(dead_code, reason = "see the allow on `radio_heap_bytes`")]
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
#[allow(dead_code, reason = "see the allow on `radio_heap_bytes`")]
pub const WIFI_PEAK_BYTES: usize = 54_620;

/// Say at boot when this image's heap cannot hold the driver's working set.
///
/// **The ESP32-S3 does not trip this today**, and it is here anyway, and since
/// 2026-08-19 the reason is sharper rather than weaker. The heap is now
/// *whatever the linker had left* — see [`heap_region`] — so every static added
/// anywhere in the image takes a byte off it, silently, with no diff to review
/// and no constant to re-measure. There is no longer a moment at which somebody
/// looks at the figure. This line is the moment.
///
/// It reports what the allocator actually holds rather than what
/// [`radio_heap_bytes`] planned, because after the rewrite those are two things
/// and a screen that reads the first while a warning reads the second is how a
/// disagreement hides.
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
#[allow(dead_code, reason = "see the allow on `radio_heap_bytes`")]
pub fn warn_if_undersized() {
    let installed = size_bytes();
    if installed < WIFI_WORKING_SET_BYTES {
        crate::logln!(
            "heap: {} bytes is below the {} the Wi-Fi driver was measured to \
             hold at rest — this image has too little DRAM left for the radio \
             and a bootable stack at once, and association is expected to end \
             in a heap-exhaustion panic. See crates/firmware/src/heap.rs.",
            installed,
            WIFI_WORKING_SET_BYTES,
        );
    }
    warn_if_tight(installed);
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
/// **The ESP32-C3 was the reason it exists**, and that chip was dropped on
/// 2026-08-19 along with the ESP32 before it, whose 1,700-byte margin was the
/// other case. The ESP32-S3 clears the peak comfortably and does not trip this.
///
/// **It is kept anyway, and not as decoration** — the argument for it got
/// stronger on 2026-08-19, not weaker. The heap is no longer a subtraction from
/// a constant somebody re-measured; it is [`heap_region`], whatever the linker
/// had left, so every static added anywhere in the image takes a byte off it
/// with no diff to review and **no moment at which a person looks at the
/// figure**. This is that moment. And what it compares against is a *measured*
/// peak from a real broker with a real installation, which is a thing no
/// arithmetic in this file can predict.
///
/// Not a refusal, and not a `const` assertion — it could not be one now even if
/// that were wanted, since the quantity is not a constant — for the reason
/// [`warn_if_undersized`] gives: a build in this state must stay in the matrix,
/// because the matrix is what would catch the problem.
#[allow(dead_code, reason = "see the allow on `radio_heap_bytes`")]
fn warn_if_tight(installed: usize) {
    // Only when the heap clears the resting set, so this never doubles up on
    // the harder line above.
    if !(WIFI_WORKING_SET_BYTES..WIFI_PEAK_BYTES + PEAK_NOISE_BYTES).contains(&installed) {
        return;
    }
    // **Two lines rather than one, because the subtraction changes sign.** A
    // heap below the peak used to be unrepresentable here and the message
    // computed `RADIO_HEAP_BYTES - WIFI_PEAK_BYTES` unguarded. On the ESP32-C3,
    // once the firmware-upload route landed, that became a `usize` underflow —
    // and both operands were `const`, so the compiler evaluated it and refused
    // the build outright with `attempt to compute 52224_usize - 54620_usize`.
    // The condition this line reports is exactly the one it would not compile
    // for, which is a good way round for it to have been found.
    //
    // **That accident cannot happen again**, because the figure is now read
    // from the linker at run time rather than computed at compile time — which
    // is precisely why the `saturating_sub`s stay: the compiler will no longer
    // refuse the build, so an edit to the guard above would produce a panicking
    // subtraction on a device instead of an error on a desk.
    if installed < WIFI_PEAK_BYTES {
        crate::logln!(
            "heap: {} bytes is {} BELOW the worst announcement peak ever measured ({}), \
             though still {} above the driver's resting working set. This board is expected \
             to associate and to connect to a broker; what is in doubt is the burst of \
             retained discovery configs, which is where the peak was measured. Watch \
             `heap: session announced` — if it approaches the total, an announcement can \
             exhaust the heap and reset the board. The peak is an ESP32-S3 measurement and \
             has never been taken on this chip. See crates/firmware/src/heap.rs.",
            installed,
            WIFI_PEAK_BYTES.saturating_sub(installed),
            WIFI_PEAK_BYTES,
            installed.saturating_sub(WIFI_WORKING_SET_BYTES),
        );
        return;
    }
    crate::logln!(
        "heap: {} bytes leaves {} above the worst announcement peak ever measured \
         ({}), which is inside the {}-byte spread that peak showed between boots. \
         Watch `heap: session announced` on this board — if it lands near the \
         total, an announcement can exhaust the heap and reset it. See \
         crates/firmware/src/heap.rs.",
        installed,
        installed.saturating_sub(WIFI_PEAK_BYTES),
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
#[allow(dead_code, reason = "see the allow on `radio_heap_bytes`")]
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
#[allow(dead_code, reason = "see the allow on `radio_heap_bytes`")]
pub fn install_for_radio() {
    install(radio_heap_bytes());
}

/// Install the scheduler's heap only. For bring-up binaries with no network.
///
/// The same region, capped: [`SCHEDULER_HEAP_BYTES`] out of the bottom of it,
/// with the rest simply not handed to the allocator. That keeps the *stack*
/// identical in every binary this file is included by, so `crate::stack_region`
/// and `crate::paint_stack` need to know nothing about which of these two ran.
#[allow(
    dead_code,
    reason = "used by tx-check, which includes this file by path"
)]
pub fn install_scheduler_only() {
    install(SCHEDULER_HEAP_BYTES.min(radio_heap_bytes()));
}

/// Hand `bytes` at the bottom of the linker's `.heap` section to `esp-alloc`.
///
/// **This is what `esp_alloc::heap_allocator!` would have done**, minus the
/// `static [u8; N]` it declares — which is the whole change, because that array
/// is what made the heap a static the linker had to place and therefore made
/// the split a measured constant. The macro's body is three lines and this is
/// those three lines with a runtime address; nothing about the allocator's
/// contract changes, and `esp-radio`'s buffers still come from a region marked
/// `Internal`, which is what its driver requires.
///
/// Zero is a legitimate answer and is not an error here: it means this image's
/// statics consumed everything the budget left, which [`warn_if_undersized`]
/// then says out loud on the next line of the boot log. It cannot mean the
/// stack is short — `build.rs`'s `ASSERT` would have failed the link first.
fn install(bytes: usize) {
    if bytes == 0 {
        return;
    }
    let (start, _) = heap_region();
    // SAFETY: `start .. start + bytes` is inside the `.heap` output section
    // `build.rs` reserves, which is `(NOLOAD)` and named by no other code in
    // this image — `bytes` is bounded by `radio_heap_bytes()`, which is that
    // section's own length. The section is part of the image's memory map, so
    // it lives for the whole program, and this runs once before anything can
    // allocate. `esp_alloc::heap_allocator!` makes the identical claim about a
    // static array; the only difference is where the address came from.
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            start as *mut u8,
            bytes,
            esp_alloc::MemoryCapability::Internal.into(),
        ));
    }
}

/// Print the heap's size, its current use and its high-water mark.
///
/// [`radio_heap_bytes`] is read from the linker rather than from this figure,
/// but this is what says the division left enough: the high-water mark against
/// the size, on a running board. So it is the measurement rather than decoration — a
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
/// [`radio_heap_bytes`] is what this build asked for and this is what it got.
/// They agree today; reporting the measured one means a diagnostics screen
/// cannot disagree with a serial console, and means the day they stop agreeing
/// is visible rather than inferred.
///
/// **Restored 2026-08-19 after a merge dropped it**, together with
/// [`used_bytes`], [`RESTORE_CHAIN_BYTES`] and 224 bytes of
/// [`BOOT_CHAIN_BYTES`] — see the module docs. Losing this one was the only
/// loss of the four that stopped the crate compiling, which is the whole reason
/// the other three went unnoticed.
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
/// **This is the figure [`radio_heap_bytes`] is checked against**, and it is
/// worth publishing rather than only printing, for two reasons that measurement
/// established. It is reached within a second of the
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
