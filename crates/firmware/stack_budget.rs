// The one number that divides this chip's DRAM, in the one place both readers
// can see it.
//
// It is `include!`d twice and compiled nowhere on its own: `src/heap.rs` takes
// it for the compile-time gate against `heap::REQUIRED_STACK_BYTES`, and
// `build.rs` takes it to write the linker fragment that *reserves* it. Two
// copies of this figure — one in Rust and one in a linker script — would be
// two numbers that can disagree, and disagreeing numbers about DRAM are the
// whole reason `src/heap.rs` was rewritten.
//
// It lives outside `src/` deliberately. A file in `src/` is a module of the
// binary and `build.rs` cannot include one without cargo re-running the build
// script on every source edit; a file beside `build.rs` is neither crate's
// module and is named by both.

/// The main stack, exactly, before the heap takes the rest of DRAM.
///
/// **This is the division, and it is now the only hand-written number in it.**
/// `build.rs` emits a `.heap` output section that runs from the end of the
/// statics to exactly this many bytes below the top of DRAM, and esp-hal's own
/// `.stack` section — which begins where ours ends — therefore measures exactly
/// this. Everything else about the division is read back out of the linker's
/// symbols at boot.
///
/// ### Why it is 66,280 and why it must not rise
///
/// It used to be written as `49_592 + 16_688` — a requirement plus a margin —
/// and **both halves of that were wrong while their sum was right.** Nothing
/// available then was unavailable now; only the account of it was wrong, which
/// is why the sum is kept unchanged and every heap figure measured against it
/// stays valid. At today's `heap::REQUIRED_STACK_BYTES` of 57,344 the margin
/// this division buys is 66,280 − 57,344 = **8,936**, not 16,688.
///
/// ### It was fixed by the ESP32, and the ESP32 is gone
///
/// The figure was chosen because it was the most the *ESP32* could give the
/// stack while still leaving the Wi-Fi driver a heap — that chip was the binding
/// constraint on the whole design. It was dropped on 2026-08-18 and the ESP32-C3
/// on 2026-08-19, so the binding constraint is gone with them.
///
/// **The figure is kept unchanged anyway, and that is a judgement rather than an
/// oversight.** Lowering it is the only direction that would buy anything — more
/// heap — and there is almost nothing there to buy: `REQUIRED_STACK_BYTES` is
/// 57,344 and `STACK_MARGIN_FLOOR_BYTES` is 8,192, so the floor on this budget
/// is 65,536 and the whole available move is **744 bytes**. It would pin a
/// number that a growing call graph moves, for less than a kilobyte. Every heap
/// figure ever measured against 66,280 also stays valid, which is worth more
/// than 744 bytes of nothing.
///
/// ### What happens when it no longer fits
///
/// A **link error**, which is new as of 2026-08-19 and is the point of the
/// rewrite. The linker fragment reserves this many bytes at the top of DRAM
/// before the heap gets anything, so an image whose statics grow past what is
/// left fails the `ASSERT` in that fragment naming both numbers, rather than
/// producing a device whose stack is quietly short.
pub const STACK_BUDGET_BYTES: usize = 66_280;
