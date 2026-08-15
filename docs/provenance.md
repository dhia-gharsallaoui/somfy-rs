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

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| `TIMINGS::WAKEUP_HIGH` = 10920 µs, `TIMINGS::WAKEUP_LOW` = 7357 µs | `somfy-rts/src/pulse.rs` | reference transmitter's wake-up delay constants | Hardware-verified (see "Hardware-verified values" below) |
| `TIMINGS::HW_SYNC_HALF` = 2560 µs, `TIMINGS::SW_SYNC_HIGH` = 4850 µs | `somfy-rts/src/pulse.rs` | reference transmitter's sync-pulse timing constants | Hardware-verified against captured wall-remote frames (see "Hardware-verified values") |
| `TIMINGS::HALF_SYMBOL` = 640 µs, `TIMINGS::INTER_FRAME_GAP` = 27434 µs | `somfy-rts/src/pulse.rs` | reference transmitter's bit-timing and inter-frame-gap constants | Hardware-verified against captured wall-remote frames; also exercised end-to-end by the golden hardware-capture suite (`somfy-rts/tests/golden.rs`) |
| Sync-pulse counts per frame kind (56-bit: 2 hardware syncs on the first frame, 7 on a repeat; 80-bit: 12 on the first frame, 6 on a repeat) and gap suppression on 80-bit frames | `somfy-rts/src/pulse.rs::render_pulses` | reference transmitter's frame-preamble routine | 56-bit counts hardware-verified (see "Hardware-verified values"); 80-bit counts verified only by round-trip decode tests (`somfy-rts/tests/frame80.rs`) — no 80-bit hardware capture available yet |
| RX sync-acquisition threshold (4 hardware-sync half-pulses minimum) and the ±25% timing tolerance window | `somfy-rts/src/rx.rs::MIN_HW_SYNCS`, `somfy-rts/src/rx.rs::within` | a simplification of the reference receiver's timing-tolerance bounds | Verified by the tolerance-boundary tests in `somfy-rts/tests/rx_loopback.rs` (a +24% stretched half-pulse still decodes, +26% aborts) |
| Bit-length detection from the accumulated hardware-sync count | `somfy-rts/src/rx.rs::RxDecoder::detect_bit_length` | the reference receiver's frame-length disambiguation logic | Verified by `somfy-rts/tests/frame80.rs::rx_decoder_recognizes_80_bit_frames` and the 56-bit loopback tests in `somfy-rts/tests/rx_loopback.rs` |
| 56-bit frame byte layout, forward-XOR obfuscation, and nibble-XOR checksum | `somfy-rts/src/frame.rs` (`encode56`, `decode56`, `checksum`, `obfuscate`, `deobfuscate`) | the reference transmitter's 56-bit frame-encoding routine | Verified by round-trip tests in `somfy-rts/tests/frame56.rs` and by the golden hardware-capture suite (`somfy-rts/tests/golden.rs`) |
| 80-bit frame byte layout and its two independent checksums (nibble-XOR over bytes 0-6, tail parity over bytes 7-9) | `somfy-rts/src/frame.rs` (`encode80`, `decode80`, `calc80_checksum`) | the reference transmitter's extended-command frame-encoding routine | Verified by round-trip tests in `somfy-rts/tests/frame80.rs` |
| Extended-command tail byte values per command, and the byte-7 repeat progression (`start + 4*repeat`, cycling by −15 once the sum would exceed 255) | `somfy-rts/src/frame.rs` (`encode80_tail`, `encode80_byte7`) | the reference transmitter's extended-command tail-encoding routine | Verified by `somfy-rts/tests/frame80.rs` (`byte7_progresses_by_four_per_repeat_and_wraps_at_15`, `favorite_and_stop_flip_byte7_on_later_repeats`, `base_command_tails_match_cpp`) |
| Command discriminant values, including the three extended-command byte values (StepUp = 0x8B, Favorite = 0xC1, Stop = 0xF1) | `somfy-rts/src/command.rs::Command` | the reference firmware's command enumeration | Verified by `somfy-rts/tests/frame56.rs::command_nibble_mapping_matches_cpp_enum` and the 80-bit round-trip tests in `somfy-rts/tests/frame80.rs` |
| Key-byte derivation (`0xA0 \| (rolling_code & 0x0F)`) and the rolling-code storage convention (this crate stores the NEXT-TO-SEND code; some deployed implementations store the LAST-SENT code instead) | `somfy-rts/src/rolling.rs::RollingCode` | real remotes' wire convention for the key byte, and the "persist before TX" invariant | Wire convention verified by round-trip frame tests; the storage-convention distinction is a design decision recorded in `docs/specs/2026-07-15-rust-rewrite-design.md` §4, not independently hardware-verified |

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
