# somfy-rs Plan 4b — Firmware RX, tasks, and the persist-before-TX invariant

Completes Plan 4. Plan 4a delivered the transmit path and proved it on air;
this plan adds receive, the two Embassy tasks, and the rolling-code store whose
ordering guarantee is the whole point of the exercise.

Design source: [`docs/specs/2026-08-15-plan4-firmware-radio-design.md`](../specs/2026-08-15-plan4-firmware-radio-design.md)
§6, §7, §12 steps 5–7.

## What 4a established that constrains this plan

Four findings from building and flying the transmit path. Each is a constraint
here, not a preference.

1. **`crates/firmware` cannot be compiled for the host at all.** `esp-hal`'s
   build script panics on a host target, and it is an unconditional dependency,
   so *no* target in that crate — lib, bin or test — builds for the host. Every
   piece of testable logic must therefore live in a workspace crate beside it.
   `somfy-rts`, `somfy-rmt` and `somfy-cc1101` are all shaped by this. The
   firmware crate holds only what genuinely needs `esp-hal`.

2. **Verification belongs in an independent receiver, not in the artifact.** A
   transmitter that reports on its own output cannot detect the failures that
   matter: a wrongly built pulse train and the account it gives of itself are
   wrong in the same way. This applies symmetrically to RX — a receiver that
   validates its own decode proves nothing; feed it frames from a known-good
   transmitter.

3. **`transmit_frame` needs ~6.5 KB of stack**, mostly two 320-pulse buffers.
   That is more than a default Embassy task gets. The radio task must be sized
   deliberately or it will overflow in a way that looks like random corruption.

4. **A single burst that decodes nothing proves nothing.** During 4a bring-up
   the reference link decoded ~4–12% of frames, and one 3-frame burst returning
   zero decodes was misread as a broken RMT path. It was not. Any on-air
   measurement in this plan needs ≥10 trials per configuration.

## Prerequisite — fix the RF link before any on-air RX work

**This blocks Task 6, and only Task 6.** Tasks 1–5 are host work and can
proceed regardless.

The link between the two boards currently decodes ~4–12% of frames with bit
errors at −72…−75 dBm, where an OOK link with that much margin should be
essentially error-free. Note this is a problem with the *measuring instrument*:
the motor received reliably throughout (both directions, first try). But RX
validation means *our* board listening to a known-good transmitter, so a
marginal link makes every result ambiguous — which is exactly how it produced a
confident wrong diagnosis in 4a.

Likely causes, in order of cheapness to check: antenna or module seating on
either board, a 433 MHz interferer, board placement. Requires physical access.

---

## Task 1 — `PulseSource` trait and a host-replay implementation

**Crate:** `crates/somfy-rmt` (hardware-free; the trait yields `Pulse` values
and nothing else).

Do **not** put the trait in `crates/firmware`: the whole point is that the
radio task can be exercised on the host, and nothing in that crate can be.

**Produces:**
- `pub trait PulseSource { fn next_pulse(&mut self) -> Option<Pulse>; }` — exact
  shape to be settled by the implementer against how `RxDecoder` consumes; the
  requirement is that it yields **merged edge-to-edge** pulses, which is both
  what RMT hands back and what `RxDecoder` already accepts (no decoder change).
- `pub struct ReplayPulseSource<'a>` — replays a `&[Pulse]` slice. This is
  §13 open question 4, and it is nearly free once the trait exists.

**Tests (host):** replay a golden capture through `ReplayPulseSource` into
`RxDecoder` and assert the decoded frame matches. The `.pulses` fixtures
already in `crates/somfy-rts/tests/fixtures/` are real wall-remote captures —
**read them, never modify them.**

This task is what makes Tasks 4 and 5 testable without hardware. Do it first.

## Task 2 — `RollingCodeStore` trait and the ordering helper

**Crate:** a hardware-free workspace crate (extend `somfy-domain`, or add
`somfy-store` if that pollutes the domain model — implementer's call, stated in
the report).

This task carries the plan's critical invariant. From §7.1, verbatim:

> A single `transmit()` helper takes the `RollingCodeStore`, commits, and
> *then* enqueues the `TransmitRequest`. Nothing else can enqueue — the
> channel's producer end is not exposed. No call site can get the order wrong
> because no call site can reach the channel directly.

**Enforce this structurally, not by convention.** If a call site *can* enqueue
without committing, the design has failed even if no current call site does.
The failure mode is a crash between increment and transmit de-syncing a motor
pairing, which costs the user a physical re-pairing procedure.

**Tests (host), and these are the ones that matter:**
- A mock store asserts `commit` is observed **before** enqueue.
- **A failed commit means no transmission at all** — not a transmission with a
  stale code.
- Attempting to enqueue directly should not compile. If you cannot express that
  in the type system, say so explicitly in your report rather than settling for
  a runtime check and calling it structural.

## Task 3 — flash-backed `RollingCodeStore`

**Crate:** `crates/firmware` (needs real flash).

Append-only, wear-levelled counter region — **rolling codes only**, no other
configuration. Plan 6 replaces the backing implementation; the seam and the
ordering guarantee stay. ~2 writes per command against 100k-cycle endurance.

Keep any address arithmetic or wear-levelling *slot selection* logic in a pure
function in the Task 2 crate so it is host-testable; the firmware side should
be the flash I/O and nothing else.

## Task 4 — `RmtPulseSource`

**Crate:** `crates/firmware`.

Implements Task 1's trait over `esp-hal`'s RMT RX.

**Idle threshold: 12,000 µs**, from §6.2. It must sit above the longest in-frame
LOW and below the inter-frame gap so each repeat lands as its own completed
transaction rather than being concatenated or truncated:

```
WAKEUP_LOW (7,357 µs)  <  idle_threshold  <  INTER_FRAME_GAP (27,434 µs)
```

Add a **compile-time assertion** for that inequality against the `TIMINGS`
constants, in the same spirit as 4a's `RMT_CLK_DIVIDER`/`TICK_US` and
`MAX_TICKS`/`PulseCode::MAX_LEN` guards. A threshold that silently drifts
outside the window would fragment or merge frames with nothing to explain why.

The field is a `u16` of ticks; at 1 µs resolution the ceiling is 65,535 µs, so
12,000 is comfortably representable — assert that too if it is not implied.

`GpioPulseSource` (the interrupt-timestamping fallback for the recorded RMT-RX
risk) is **out of scope unless Task 6 shows RMT RX cannot do the job.** Do not
build it speculatively; the trait exists precisely so it can be added later
without a redesign.

## Task 5 — radio and state tasks

**Crate:** `crates/firmware`.

Two Embassy tasks, statically allocated, over bounded channels (§7):

- **radio** — sole owner of the CC1101 and both RMT channels. Consumes
  `TransmitRequest`s; publishes decoded frames. **Radio timing never blocks on
  anything else** — that is the reason for the split.
- **state** — owns the `somfy-domain` `Controller`. Applies commands and
  received frames, runs position-estimator ticks, publishes state deltas.

**Size the radio task's stack for ≥6.5 KB** (finding 3 above) and state the
chosen figure with its justification. A stack overflow here presents as random
corruption, not as a clean failure.

Everything worth asserting about these tasks should be reachable on the host
via `ReplayPulseSource` and a mock store.

## Task 6 — on-air bring-up: listen, then transmit, then position

**Blocked on the RF-link prerequisite above.** Do not start until the link is
healthy, or every result will be ambiguous.

Order matters and is not arbitrary — listening is strictly safer than
transmitting, so it comes first:

1. **Listen.** Our board receives; a wall remote or the reference device
   transmits. Assert decoded address, command, `bits`, and the sync counts
   (**4** on first frames, **14** on repeats — measured, not assumed).
2. **Transmit**, verified by the reference receiver as in 4a. Read the rolling
   code live immediately beforehand — see
   [`docs/hardware-checklist.md`](../hardware-checklist.md); a stale value is
   rejected as a replay and looks exactly like a broken transmitter.
3. **Position tracking** end to end: command → transmit → overheard decode →
   domain position update.
4. **Reflash survives the rolling code.** The store's whole purpose. Flash,
   transmit, note the code, reflash, transmit again, and confirm the motor
   accepts it.

≥10 trials per configuration before concluding anything.

Update [`docs/hardware-checklist.md`](../hardware-checklist.md) with the RX
procedure, and `docs/provenance.md` with any new hardware-verified values.

---

## Out of scope

WiFi, MQTT and Home Assistant discovery (Plan 5); OTA and A/B partitions
(Plan 6); the position-accuracy work in
[`docs/specs/2026-08-15-position-accuracy-requirements.md`](../specs/2026-08-15-position-accuracy-requirements.md)
beyond what Task 6 step 3 exercises.

Two items carried forward, neither blocking this plan:

- **Travel times are uncalibrated.** The office shade still has the default
  `upTime`/`downTime` of 10,000 ms, which is half of the position-accuracy
  problem and will make step 3's dead-reckoning wrong in a way that is not the
  firmware's fault.
- **Pre-public obligation.** The committed `.pulses` fixtures encode a real
  remote's address and rolling codes, and must be re-captured with a throwaway
  address or removed before this repository goes public. Re-capturing needs two
  working radios — which, once the RF link is fixed, this plan will have.
