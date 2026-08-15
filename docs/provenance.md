# Provenance

somfy-rs is an independent project. Its source code stands on its own: comments
explain what the code does and why, in this project's own terms, without
pointing at another codebase.

That independence must not cost **auditability**. Many of this firmware's
constants and algorithms are not free choices — they are dictated by the Somfy
RTS protocol and by the behaviour of motors already paired in the field. Where a
value was *derived* rather than invented, this document records where it came
from and when it was checked, so a reader can verify it without the code
advertising its ancestry.

**This file is the only place the reference implementation is cited.**

## The reference

- **ESPSomfy-RTS** (C++ / Arduino), released into the public domain (Unlicense).
- Verified against locally at `/home/dhia/Sources/personal/ESPSomfy-RTS`,
  firmware version **v2.5.6**.
- somfy-rs is independently licensed **GPL-3.0-only**. The Unlicense imposes no
  attribution requirement; this document exists for engineering auditability,
  not for licence compliance.

## Rules

1. **Deriving is required; citing in code is not.** Never invent protocol
   behaviour. Read the reference, verify the value, then record it here — not in
   a source comment.
2. **Preserve the reasoning, drop the pointer.** When removing a citation, the
   *knowledge* it carried must survive in the comment, restated in this
   project's own terms. A comment that asserts a constraint with its
   justification deleted is worse than either extreme.
3. **Inline references are permitted only as a documented exception.** If a
   comment cannot convey its meaning without naming the reference — typically
   where the point *is* a deliberate divergence — keep it, and add a row to
   [Inline exceptions](#inline-exceptions) saying why. An undocumented inline
   reference is a defect.
4. **Ground truth beats derivation.** Where a value has since been confirmed
   against real hardware (captured pulses, a real device backup), say so. That
   evidence outranks the reference implementation, which is itself only a
   reading of the protocol.

## Format

Each crate gets a section. One row per derived value or algorithm:

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| `TIMINGS::HALF_SYMBOL` = 640 µs | `somfy-rts/src/pulse.rs` | reference transmitter's `SYMBOL` define | 2026-08-15 — confirmed against real wall-remote capture |

`Derived from` names the reference construct in prose (function or field name),
not a line number: line numbers rot against a codebase we do not control, and a
prose name is what a reader actually searches for.

---

## somfy-rts

<!-- filled by the somfy-rts cleanup pass -->

## somfy-domain

<!-- filled by the somfy-domain cleanup pass -->

## somfy-migrate

<!-- filled by the somfy-migrate cleanup pass -->

## somfy-api

<!-- filled by the somfy-api cleanup pass -->

---

## Inline exceptions

Comments that still name the reference implementation, with justification.
Every entry here is a deliberate decision, reviewed. An inline reference that
is not listed here should be removed.

| Location | Why it must stay |
|---|---|
| _(none yet)_ | |

---

## Hardware-verified values

Values confirmed against real hardware, which outrank any derivation:

| Value | Evidence | Date |
|---|---|---|
| 56-bit first frame emits 2 hardware syncs; repeat emits 7 | Captured wall-remote frames reported `hwsync` 4 and 14 half-pulses respectively | 2026-08-15 |
| Wake-up pulse ≈ 10.9 ms HIGH | First pulse of a real capture measured 10229 µs | 2026-08-15 |
| Backup file format and record layout | Parsed a real v25 device backup with zero field misalignment | 2026-08-15 |
