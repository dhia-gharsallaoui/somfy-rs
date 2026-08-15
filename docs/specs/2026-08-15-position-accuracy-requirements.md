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

### R7 — Migration should flag factory-default travel times (SHOULD)

`somfy-migrate` imports travel times, so a migrated setup inherits whatever the
C++ device held — typically the untouched 10 s/10 s defaults. Plan 6 SHOULD
surface values that look factory-default and invite the user to calibrate,
rather than presenting them as configured.

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
