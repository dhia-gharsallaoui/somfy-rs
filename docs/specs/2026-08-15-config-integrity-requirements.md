# somfy-rs — Configuration integrity requirements

> A Plan 6 (persistence) requirement, from a failure observed on real hardware.
> Short by design: one failure, one root cause, four requirements.

## What happened

On 2026-08-15 the reference C++ device stopped controlling any shade. Symptoms:

- Web UI, API and WiFi all healthy.
- `radioInit` reported **true** — the CC1101 answered its version register over SPI.
- Motors did not respond to any command.
- The device also failed to decode frames from a transmitter a few metres away.

Failing in **both** directions while reporting a healthy radio pointed at the one
component transmit and receive share — the antenna. That diagnosis was wrong, and
several hours went into it.

The actual cause: the device's **radio data-pin configuration had reverted to the
firmware's compiled-in defaults**. It had been configured with the CC1101's data
lines on GPIO3/GPIO4, matching the physical wiring. It was found running with
GPIO15/GPIO14 — the built-in defaults for that chip family. The SPI pins were
untouched, which is why the health check still passed.

So the firmware drove transmit data onto a pin connected to nothing, and listened
for receive data on another pin connected to nothing, while truthfully reporting
that the radio was initialised. Restoring the two pin values fixed it immediately.

## Why the health check did not catch it

`radioInit` proves exactly one thing: an SPI register read returned a plausible
value. It says nothing about the data path, the antenna, or whether the configured
pins match reality. **A health indicator that can report success while the device
is totally non-functional is worse than no indicator**, because it actively steers
diagnosis away from the truth — as it did here.

## Requirements

### R1 — Configuration must never silently fall back to a compiled-in default (MUST)

If stored configuration is missing, unreadable, or fails validation, the firmware
MUST surface that as a distinct, visible state — not quietly substitute a default
and continue as though configured. Defaults are for first boot, and first boot
must be distinguishable from "your settings are gone".

### R2 — Radio health must reflect the data path, not just the control path (MUST)

Whatever the firmware reports as radio health MUST NOT be satisfiable by an SPI
register read alone. At minimum, report the control path and the data path as
**separate** facts, so "the chip answers" can never be mistaken for "the radio
works".

### R3 — Surface a transmit/receive silence signal (SHOULD)

A device that has transmitted many commands and heard **nothing** on the air for a
long period is probably deaf, whatever its configuration claims. Surface that as a
diagnostic. It is the cheapest possible detector for this entire class of fault,
and it would have pointed at the radio path within minutes.

### R4 — Configuration changes must be attributable (SHOULD)

Record when radio configuration last changed and what it changed from. In this
incident the pins were known-good earlier the same day and defaulted later, and
there was no way to tell whether a person, a UI action, or a storage failure did
it — which left the underlying bug undiagnosed even after the symptom was fixed.

## Acceptance criteria

1. Corrupt or erase the stored config; assert the firmware reports an explicit
   unconfigured/invalid state rather than running on defaults.
2. Configure data pins that are deliberately wrong; assert radio health does **not**
   report fully healthy.
3. Unit-test the silence detector: no frames received within the window raises the
   diagnostic.

## Open question

Whether the reversion was caused by a user action, a UI default-preset applied
accidentally, or genuine storage loss is **unresolved**. If it recurs after a power
cycle, storage loss is the likely cause and this document should be revisited —
R1 addresses the symptom either way, but not necessarily the disease.
