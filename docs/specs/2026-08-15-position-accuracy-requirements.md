# somfy-rs — Position accuracy & travel-time calibration requirements

> Refines the position-engine behaviour ported in Plan 2 (`somfy-domain`
> `motion.rs` / `shade.rs`). Plan 2 deliberately reproduces the C++ model
> **faithfully**, citing `Somfy.cpp:1052-1234` line by line — which means it
> also reproduces its inaccuracy faithfully. Everything below is an
> **intentional divergence from the reference**, recorded as such rather than
> slipped in as a bug fix.

## Why this document exists

Observed in the field on 2026-08-15: a mid-range position request (~60%) drove
the shade **fully closed** instead of stopping. Endpoints (fully open / fully
closed) are consistently accurate; intermediate positions are not.

That asymmetry is not a tuning problem. It falls directly out of the design, and
it has **two independent causes that look identical from the outside**. A fix
that addresses only one will appear to work and then fail intermittently.

## Evidence: what the code actually does

**1. The estimator is pure open-loop dead reckoning.** `motion.rs:100-125` is a
linear integrator: `pos = start_pos + elapsed / travel_time`. There is no
feedback and there cannot be — RTS is a one-way protocol and the motor never
reports position.

**2. A mid-range seek depends on exactly one radio frame.** `shade.rs:257-268`:

```rust
if snap.arrived {
    if self.stop_on_arrival && snap.pos != Pos::ZERO && snap.pos != Pos::FULL {
        self.push(out, Command::My);
    }
    self.stop_on_arrival = false;
}
```

The motor self-stops **only at its hard limits** (`shade.rs:252-256`, citing
`Somfy.cpp:1166-1170` down / `:1218-1227` up). So a seek to 60% is: send `Down`,
integrate locally, and send a single `My` when the *estimate* says 60%. If that
one `My` is not received, nothing stops the motor and it runs to the limit.

**3. Endpoints are the only ground truth.** `Up`/`Down` target `Pos::ZERO` /
`Pos::FULL`, which the motor's physical limits enforce regardless of our
estimate. Every position in between is dead reckoning whose error accumulates
and is never corrected.

**4. Travel times are unmeasured defaults.** `ShadeConfig` (`types.rs:145-147`)
carries `up_time_ms`, `down_time_ms` and `tilt_time_ms`, defaulting to
10 s / 10 s / 7 s (`Somfy.h:314-316`). The reference device in the field still
holds those exact defaults. The fields are **already per-direction** — the
asymmetry is supported and simply never measured. A roller descends faster than
it rises.

### The two failure modes, separated

| # | Cause | Symptom | Fixed by calibration? |
|---|---|---|---|
| **A** | The single arrival-stop `My` frame is lost | Shade runs past target to the limit | **No** |
| **B** | Travel times are wrong | Shade stops at the wrong place, but *does* stop | Yes |

**Cause A is weighted higher.** A degraded RF path (as diagnosed on the
reference device on 2026-08-15 — a faulty antenna) would leave open/close
working perfectly, since those end at a physical limit regardless, while
silently breaking every intermediate position. That is exactly the reported
signature.

### Field evidence, 2026-08-17: it was cause B, and R7 called it

The owner reported commanding 25% open and getting "no more than 1% maybe", on
somfy-rs driving three imported shades. The table was read back:

| shade | `up_time_ms` | `down_time_ms` | `tilt_time_ms` |
|---|---|---|---|
| all three | 10000 | 10000 | 7000 |

**Identical across three physically different shades, and identical to the
reference firmware's compiled-in defaults** (`Somfy.cpp:698-700`,
`Somfy.h:314-316`). Nobody had ever calibrated them; the values came across in
the backup because they had never been changed, and `somfy-migrate` imported
them faithfully.

A 25% move therefore ran the motor for 2.5 s. Measured by hand the same day,
the shades take **~30 s to open and 27 s to close** — so the commanded run was
about a twelfth of the intended travel before dead time is even considered.
That is the whole reported error, and it needs no appeal to a lost stop frame.

Three things follow, and they change the weighting above:

1. **Cause B was underweighted.** The document reasoned that a degraded RF path
   made cause A more likely, from a real antenna fault on the reference device.
   That reasoning was sound and the conclusion was still wrong here: nobody
   checked whether the travel times were real numbers. **The cheap check should
   have come first** — reading three stored integers costs nothing and would
   have ended the question immediately.
2. **The 30 s / 27 s asymmetry is real and about 10%.** Closing is
   gravity-assisted. This is the direct justification for storing the two
   independently rather than one scalar, and any calibration that measures one
   direction and mirrors it is wrong by that much.
3. **R7 should be a MUST, not a SHOULD.** It predicted this precisely —
   "typically the untouched 10 s/10 s defaults" — and being advisory is why an
   import that was *known* to carry placeholder values was presented to the user
   as configured. A requirement that names the failure and then does not oblige
   anyone to prevent it is not doing the work of a requirement.

**Interim state:** 30000/27000 were written to the board by hand on 2026-08-17,
so positions are approximately right today. That is a stopgap for one estate and
not a fix — it is one person with a stopwatch, unrecorded provenance, and it
does not survive a re-import. R2 remains the deliverable.

## Requirements

### R1 — Arrival-stop frames must be transmitted redundantly (MUST)

The `My` frame that ends a mid-range seek is the single point of failure in the
whole position system. It MUST be transmitted with **more redundancy than an
ordinary command**, not the same.

Two concrete gaps block this today:

- `PlannedTx` is `{ address, command }` (`shade.rs:45-48`) — it **carries no
  repeat count**, so the domain currently cannot express "send this one harder".
  It must grow that capability.
- `ShadeConfig` has **no `repeats` field**, although the C++ shade record does
  (visible in its `/shades` JSON). Migration therefore silently drops it.

A lost stop frame is not recoverable after the fact: by the time the estimate
notices, the shade is already at the limit.

### R2 — Travel times must be measurable, per direction (MUST)

Provide a guided calibration that measures and stores `up_time_ms` and
`down_time_ms` **independently**. The storage already exists; only measurement
and a UI flow are missing. Design spec §8 already lists "travel-time
calibration" on the shade-detail screen, so this is in scope, not scope creep.

### R3 — Endpoint resynchronisation (MUST)

Exploit the one source of ground truth the system has:

- On reaching `Pos::ZERO` or `Pos::FULL`, snap the estimate to exactly that and
  **reset accumulated error to zero**.
- When accumulated uncertainty is high, a go-to-position MAY route via the
  nearest limit first and time from there — trading a longer movement for a
  known starting point.

Without this, error accumulates monotonically across partial moves.

### R4 — Track and surface confidence (SHOULD)

Uncertainty grows with each partial move and resets at an endpoint. Surfacing
"≈60%" is more honest, and more useful to automation, than a confidently wrong
"60%".

### R5 — Dead-time compensation (SHOULD)

The current model assumes motion begins the instant a command is planned. In
reality the frame takes time on air (wake-up plus hardware/software sync before
the first data bit) and the motor has soft-start/soft-stop ramps. Fold a
measured dead time into the R2 calibration rather than hard-coding one.

### R6 — Record the divergence from the reference (MUST)

`motion.rs` and `shade.rs` document themselves as faithful ports with per-line
C++ citations. Any change made under R1–R5 MUST update those doc comments to
state clearly **where somfy-rs now deliberately differs from ESPSomfy-RTS and
why**. The value of those citations is that they are trustworthy; a silent
divergence destroys that.

### R7 — Migration MUST flag factory-default travel times (MUST)

*Raised from SHOULD on 2026-08-17, after the failure it predicted happened in
production. See "Field evidence" above.*

A travel time equal to the reference firmware's compiled-in default
(10000/10000/7000) MUST be surfaced as **uncalibrated** rather than presented as
configured — in the import summary, in the API, and wherever the UI shows a
shade's timings. A value that is merely *plausible* is not evidence anybody
chose it, and three identical values across three different shades are evidence
nobody did.

The original wording, kept because the requirement is unchanged in substance:

`somfy-migrate` imports travel times, so a migrated setup inherits whatever the
C++ device held — typically the untouched 10 s/10 s defaults. Plan 6 SHOULD
surface values that look factory-default and invite the user to calibrate,
rather than presenting them as configured.

### R8 — Travel is not linear at the closed end (MUST)

*Added 2026-08-17 from the owner's observation.*

On a European roller shutter with perforated slats, the slats are compressed
shut at the fully-closed limit. The first seconds of Up travel **separate the
slats** — opening the light gaps — before the curtain begins to rise. Measured
on this estate: **~4 s of a ~30 s up-traverse**, so roughly **13% of the
commanded time produces no elevation at all**.

A linear position↔time model is therefore wrong near the closed end, and wrong
in the direction that matters: a "25% open" command spends half its budget
separating slats and lifts far less than a quarter. This is a **second,
independent cause of the 25%→1% report**, on top of the factory-default travel
times, and it does not go away when those are corrected.

The model MUST carry a per-direction **dead band at the closed limit** —
measured, not assumed, and per shade, since it depends on slat design.

**What is not yet established, and must be tested before this is designed.**
Two mechanisms produce this symptom and they need different implementations:

- **Mechanical dead band.** The slats separate during ordinary continuous
  travel; there is one motion and its first phase does not lift. Fits the
  owner's description ("4 seconds ... before starting elevating"). Fix is a
  piecewise travel curve.
- **A distinct tilt command.** The reference models this as
  `tilt_types::euromode`, where **burst length selects the operation**: a short
  press tilts, and a long press (`TILT_REPEATS` = 15 repeats, so 16 frames)
  travels. It reads the same distinction on receive. If these motors honour it,
  the vent position is a *command*, not a timed fraction, and it becomes an
  independent axis worth exposing as HA `tilt_position`.

The estate's shades are currently provisioned as `kind = roller`,
`tilt_mode = none`, and they do complete full traverses from our ordinary
3-frame bursts — which is evidence **against** euromode press-length semantics
being active on them, since a 3-frame burst is far below `TILT_REPEATS`. That is
inference from one behaviour, not a test. **The test is cheap**: send a
short Up burst from fully closed and see whether it stops after separating the
slats or continues to the limit. It transmits at a real motor, so it is the
owner's to run.

**The user-visible behaviour is settled regardless of which mechanism applies**,
by the owner's instruction of 2026-08-17:

> "I do prefer having a dedicated command that ensure that it's fully closed
> going fully down than opening to reach only sun holes"

So the vent position is **its own command**, and it is reached **from the closed
limit**, not from wherever the shade happens to be:

1. Drive **Down** to the physical limit and let the motor self-stop there.
2. Then **Up** for the measured slat-separation time.
3. Then the arrival `My`, which the existing mid-range stop already plans.

**This needs no position estimate at all**, which is the whole point. The closed
limit is the one piece of ground truth in a one-way protocol — the motor stops
itself there — so anchoring on it makes the most-used position immune to every
source of drift in this document: wrong travel times, missed overheard frames,
accumulated partial-move error. It is R3's "route via the nearest limit first
and time from there", applied to the position that will be asked for most.

The cost is deliberate and the owner accepted it: a shade already open travels
its whole range down before venting. Slower, and correct every time.

The hardware test above still matters for **implementation** rather than for
behaviour. If these motors honour euromode press-length semantics, a short burst
from the closed limit is the native way to do step 2 — no timing, no arrival
stop, and nothing to calibrate. If they do not, step 2 is timed against the R8
dead band and inherits R9's hand-override.

### R9 — Calibration must be overridable by hand (MUST)

*Added 2026-08-17 at the owner's request.*

Automatic calibration (R2) MUST NOT be the only way to set travel times. The API
and UI MUST accept operator-supplied values for `up_time_ms`, `down_time_ms`,
`tilt_time_ms` and the R8 dead band, on an existing shade and not only at
creation.

Three reasons, each real:

- A sweep moves the shade through its full range twice per direction, which is
  not always acceptable — a shade over a desk, a sleeping room, an awning in
  wind.
- Some operators already know their numbers. The hand measurement taken on
  2026-08-17 (30 s up, 27 s down) took minutes and was the fix that worked.
- **A calibration routine needs something to be checked against.** A sweep
  reporting 10 s where a stopwatch says 30 s must be visibly wrong, and that
  comparison is impossible if hand-entered values have nowhere to live.

Manual values MUST be distinguishable from measured ones and from factory
defaults — R7 requires flagging the last of those, and "the operator typed this"
is a third state, not the same as "the device measured it".

## Acceptance criteria

Host-testable, consistent with the project's existing culture:

1. **Redundancy is expressible and asserted.** A unit test shows a mid-range
   arrival plans a stop with strictly greater redundancy than an ordinary
   command, through whatever field R1 introduces.
2. **Asymmetric travel times produce asymmetric estimates.** Property test:
   with `up_time_ms != down_time_ms`, the time to traverse the same span
   differs by the same ratio.
3. **Endpoint resync clears error.** Drive the estimator to a limit with a
   deliberately wrong travel time, then assert the reported position is exactly
   `ZERO`/`FULL` and that the next move starts from a zeroed error term.
4. **Confidence monotonicity.** Uncertainty is non-decreasing across partial
   moves and returns to its floor at an endpoint.
5. **Field check (manual, documented).** After the reference device's antenna is
   repaired, re-test mid-range seeks **before** building anything — if accuracy
   is restored by the antenna alone, cause A was dominant and R1 is confirmed as
   the priority.

## Non-goals

- **Closed-loop position control.** RTS is one-way; the motor never reports
  state. This stays open-loop by physics, not by choice. Realistic ceiling with
  R2 + R3 is roughly ±5% mid-travel — good, never exact.
- Compatibility with the C++ position behaviour. R6 makes the divergence
  explicit and deliberate.

## Open questions

1. Which plan owns each requirement? R1 spans the domain and the radio layer, so
   it likely belongs with **Plan 4b** (which introduces the `TransmitRequest`
   channel and can carry redundancy). R2–R5 are `somfy-domain` refinements.
   R7 is **Plan 6**.
2. Should the arrival stop be re-sent unconditionally N times, or should the
   estimator keep re-issuing `My` until it believes the shade has stopped? The
   latter is more robust but can over-send on a healthy link, and a spurious
   `My` on an already-stopped shade moves it to the favourite position.
3. Does the C++ `repeats` field mean "extra repeat frames per command"? Confirm
   against `Somfy.cpp` before mapping it onto R1's new field, rather than
   assuming the semantics match.
