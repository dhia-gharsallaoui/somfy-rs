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
| `MEASURED_MAX_INTRA_FRAME_SEGMENT_US` = 17738 µs — the longest segment of *either level* occurring *inside* a real transmission | `somfy-rts/src/pulse.rs` | **Not derived at all.** Measured directly from this repository's own wall-remote captures. It exists precisely because the transmit-side constant that looks like it should answer the same question (`TIMINGS::WAKEUP_LOW`, 7357 µs) is wrong by a factor of 2.4 against real hardware | Hardware-measured: the post-wake-up gap reads 17738 µs in `somfy-rts/tests/fixtures/up_56bit_1.pulses`, 17722 in `down_56bit_1.pulses` and 17711 in `my_56bit_1.pulses`. Re-derived from those three files on every test run by `somfy-rts/tests/measured.rs`, which also asserts each capture contains exactly one such gap and that it is the wake-up gap. **Level-agnostic deliberately:** the hardware rule it feeds ends a reception when no *edge* arrives, so a long HIGH counts exactly as a long LOW does. The maximum happens to be a LOW today — the longest HIGH is the ~10.2 ms wake-up pulse — but a LOW-only bound would leave the wake-up pulse outside anything checking it — so the constant cannot drift from its evidence, and a re-capture forces a re-derivation. **Bounds the intra-frame case only:** the capture ISR stops recording when a frame completes, so no committed capture contains a real remote's inter-frame silence |
| Key-byte derivation (`0xA0 \| (rolling_code & 0x0F)`) and the rolling-code storage convention (this crate stores the NEXT-TO-SEND code; some deployed implementations store the LAST-SENT code instead) | `somfy-rts/src/rolling.rs::RollingCode` | real remotes' wire convention for the key byte, and the "persist before TX" invariant | Wire convention verified by round-trip frame tests; the storage-convention distinction is a design decision recorded in `docs/specs/2026-07-15-rust-rewrite-design.md` §4, not independently hardware-verified |

## somfy-domain

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| `Command::Stop` is never planned by the domain; `My` is the only stop frame a 56-bit motor accepts, and non-basic commands are downgraded to `My` on the wire | `somfy-domain/src/shade.rs` (module docs, `Shade::handle`) | the reference firmware's shade command-dispatch routine, which downgrades non-basic commands to `My` on 56-bit motors | Verified by `somfy-domain/tests/shade.rs::stop_is_never_emitted_only_my` |
| Mid-range arrival stop: a seek that lands on an intermediate position needs an explicit `My` frame because the motor only self-stops at its hard limits (fully open/closed); hard-limit and Step-originated seeks never schedule one | `somfy-domain/src/shade.rs` (`Shade::tick`, `stop_on_arrival`) | the reference firmware's movement-tracking routine, guarded by its position-seek-in-progress flag | Verified by `somfy-domain/tests/shade.rs::goto_midrange_emits_stop_on_arrival` and `goto_full_limit_does_not_emit_stop`, and by `somfy-domain/tests/overheard.rs::overheard_arrival_at_midrange_does_not_plan_stop` |
| Step size default (100 ms of travel per Step press) and the step-target formula `FULL_RAW * STEP_TRAVEL_MS / travel_ms`, direction-matched to the travel time of the direction being stepped — yielding a 1% nudge at the default 10 s travel time | `somfy-domain/src/shade.rs::STEP_TRAVEL_MS`, `Shade::step_target` | the reference firmware's default per-motor step size and its non-tilt Step target-math routine | Verified by `somfy-domain/tests/shade.rs::step_commands_nudge_target_and_emit_extended_commands` and `step_up_nudges_toward_open_and_clamps_at_zero`; the direction-matched guard/divide (rather than mismatching the zero-travel guard to the other direction's time) was cross-checked by re-reading the reference's Step branches during the port and is exercised by the same tests |
| Overheard-frame handling: `Up`/`Down` retarget the hard limits, `My` while moving freezes the estimate, `My` while idle recalls the favorite, and `Step` nudges the estimate one step — all without transmitting, since the wall remote already drove the motor; a remote frame also abandons any of our own in-flight positioning | `somfy-domain/src/shade.rs::Shade::apply_overheard` | the reference firmware's frame-processing routine for frames from a non-internal source | Verified by `somfy-domain/tests/overheard.rs::overheard_down_moves_estimate_without_retransmit`, `overheard_my_while_moving_halts_estimate`, `overheard_my_while_idle_tracks_favorite`, `overheard_step_down_moves_estimate_by_one_step_without_tx`, `overheard_step_up_moves_estimate_up_without_tx` |
| Frame ownership check order: a shade's own remote address is checked before its linked-remote list | `somfy-domain/src/shade.rs::Shade::is_linked` | the reference firmware's frame-ownership test | Verified by `somfy-domain/tests/overheard.rs::is_linked_covers_own_and_linked_addresses` |
| Linked-remote registry bound of 7, and rejection of the sentinel addresses (0 / 0xFFFFFF) as invalid remotes | `somfy-domain/src/shade.rs::Shade::linked`, `Shade::link_remote` | the reference firmware's fixed-size linked-remotes array and its address-validity guard | Verified by `somfy-domain/tests/overheard.rs::link_remote_enforces_limit_and_duplicates` |
| DEVIATION: on real hardware the default behaviour for `My` pressed idle with no software-tracked favorite is to send a raw `My` frame, letting the motor recall a favorite stored in its own hardware; this crate always simulates positions in software instead, so idle-`My`-without-favorite is a no-op rather than a hardware recall | `somfy-domain/src/shade.rs::Shade::handle` (the `ShadeCommand::My` arm) | the reference firmware's default (simulate-My-off) behaviour, contrasted with this crate's always-simulate design | Verified by `somfy-domain/tests/shade.rs::my_while_idle_without_favorite_is_noop`; reproducing the hardware-recall path is deferred to Plan 4 |
| `Pos` stores position as a fixed-point `u16` in hundredths of a percent rather than a float | `somfy-domain/src/types.rs::Pos` | deployed controllers' floating-point 0.0-100.0 position model, replaced with an integer representation for deterministic, reproducible math | Design decision documented in the crate docs; exercised by the full `somfy-domain` test suite (204 passed) |
| `ShadeKind` discriminants (`Roller`=0x00, `Blind`=0x01, `DraperyLeft`=0x02, `Awning`=0x03, `Shutter`=0x04, `DraperyRight`=0x07, `DraperyCenter`=0x08) and `TiltMode` discriminants (`None`=0x00, `TiltMotor`=0x01, `Integrated`=0x02, `TiltOnly`=0x03, `EuroMode`=0x04) | `somfy-domain/src/types.rs::ShadeKind`, `somfy-domain/src/types.rs::TiltMode` | the shade-type and tilt-type byte values used in deployed device backups | Verified by parsing a real v25 device backup with zero field misalignment (see "Hardware-verified values") |
| Unsupported-on-import shade kinds: garage (0x05/0x06), drycontact (0x09/0x0A), gate (0x0B-0x10) fall back to `Roller` with a warning surfaced to the user | `somfy-domain/src/types.rs::ShadeKind::from_raw` | the fuller set of shade kinds deployed devices can contain, of which v1.0 models only a subset | Plan 6 policy decision recorded in the crate's README contracts |
| Movement direction sign convention: -1 toward open, +1 toward closed, 0 idle | `somfy-domain/src/types.rs::Direction`, `somfy-domain/src/motion.rs::Motion::direction` | the position-tracking sign convention deployed firmware uses | Verified by the `somfy-domain` motion and shade test suites |
| `TiltMode` trap: only `TiltMode::None` has command semantics in Plan 2; `TiltOnly`/`EuroMode` need long-press redirection of Up/Down onto the tilt axis, and `TiltMotor`/`EuroMode` need a half-second hold to disambiguate a tilt press from a lift press | `somfy-domain/src/types.rs::ShadeConfig::tilt_mode` | the reference firmware's per-tilt-mode input handling, which this crate has not yet ported | Not yet ported or tested; tracked as a Plan 3/4 gap so the API must not advertise tilt as functional until it lands |
| `ShadeConfig` factory defaults: 10s up, 10s down, 7s tilt | `somfy-domain/src/types.rs::ShadeConfig::new` | the factory defaults deployed devices ship with | Verified by parsing a real v25 device backup with zero field misalignment (see "Hardware-verified values") |
| Address plausibility guard: 0 and 0xFFFFFF are rejected as invalid sentinel addresses | `somfy-domain/src/types.rs::ShadeConfig::new` | the "unset" sentinel addresses used by deployed devices | Verified by the `somfy-domain` type test suite |
| Open-loop dead-reckoning position estimator: direction is recomputed from position vs. target every tick; while moving, position is `start_offset + elapsed` as a ratio of the direction's travel time, integrated from the open end (down) or the closed end (up) | `somfy-domain/src/motion.rs::Motion::tick` | the reference firmware's movement-tracking routine, required because RTS is a one-way protocol and the motor never reports position back | Verified by the `somfy-domain` motion test suite |
| Zero-travel-time guard: a direction with no configured travel time jumps instantly to the target instead of dividing by zero | `somfy-domain/src/motion.rs::Motion::tick` | the reference firmware's zero-travel-time handling | Verified by the `somfy-domain` motion test suite |
| Up-branch integer-division floor placement (computing `consumed` before subtracting, rather than an algebraically equivalent rearrangement) | `somfy-domain/src/motion.rs::Motion::tick` | the reference firmware's specific floor placement in its up-branch integration math, kept faithfully rather than "simplified" because the two orderings round differently | Verified by the `somfy-domain` motion test suite. `docs/specs/2026-08-15-position-accuracy-requirements.md` documents that this estimator faithfully reproduces the reference's dead-reckoning inaccuracy by design, though it does not call out this specific rounding placement |
| Snap-to-target-on-crossing rule: a tick that computes past the target reports the target position exactly, with `arrived = true`, instead of the overshot value | `somfy-domain/src/motion.rs::Motion::tick` | the reference firmware's arrival-detection logic | Verified by the `somfy-domain` motion test suite |
| Integrated-tilt sequencing gate: with `TiltMode::Integrated`, the lift axis cannot move up until tilt is fully open, and cannot move down until tilt is fully closed | `somfy-domain/src/tilt.rs::tilt_first` | the reference firmware's integrated-tilt-motor sequencing rule | Verified by the `somfy-domain` tilt test suite |
| Registry fixed capacity: 32 shades, 16 groups, 16 rooms, and at most 32 shades per group | `somfy-domain/src/registry.rs::MAX_SHADES`, `MAX_GROUPS`, `MAX_ROOMS` | the capacity bounds deployed configurations can contain | Verified by parsing a real v25 device backup with zero field misalignment (see "Hardware-verified values") |
| Registry ids are stable slot indices that survive removal of other entries, with holes reused by the next add before growing | `somfy-domain/src/registry.rs::Registry` | the reference firmware's fixed-array addressing contract, needed because ids are stored elsewhere (group/room membership) and must not silently repoint | Verified by the `somfy-domain` registry test suite |
| Overheard `My`-while-idle recall is immediate: the domain does not replicate a physical remote's ~500 ms tap-vs-hold disambiguation window, since it receives an already-decoded command rather than raw button timing | `somfy-domain/src/lib.rs` (module docs); behavior lives in `somfy-domain/src/shade.rs::Shade::apply_overheard` | the reference firmware's My-button hold-to-set timing logic, which distinguishes a tap (recall) from a hold (set new favorite) | Not independently verified; the resulting immediate-recall behavior is exercised by `somfy-domain/tests/overheard.rs::overheard_my_while_idle_tracks_favorite`, but the absence of a timing window is a design difference, not something a unit test can positively prove |
| Step command (`StepUp`/`StepDown`) transmits its frame unconditionally, even when the position is already at the hard limit and cannot move further | `somfy-domain/src/shade.rs::Shade::handle` (the Step arms) | the reference firmware's unconditional step-frame transmission | Verified by `somfy-domain/tests/shade.rs::step_up_nudges_toward_open_and_clamps_at_zero` (asserts `Command::StepUp` is still transmitted from position ZERO) |

<!-- filled by the somfy-domain cleanup pass -->

## somfy-migrate

**This crate is a documented exception: it names the reference implementation
freely, in code, and that is correct.**

Every other crate here implements the Somfy RTS protocol, which is a property of
the motors — the reference implementation was a source of knowledge about it, not
the subject. `somfy-migrate` is different in kind. Its entire purpose is to read a
backup file **produced by** the C++ ESPSomfy-RTS firmware, so an existing
installation can move to somfy-rs without re-pairing motors at the wall.

There, the other codebase *is* the subject matter. A comment saying which routine
wrote a given field is describing the input this parser must accept, not confessing
an implementation history. Stripping those names would make the crate harder to
maintain and would obscure the one thing a reader most needs to know: what produced
the bytes.

The rest of this document therefore does not attempt to catalogue this crate's
derivations — the code itself is the record, and it is allowed to be explicit.

Two things still hold here:

- **Verification outranks derivation.** The parser was validated against a real
  backup exported from a live device (format version 25), which parsed with zero
  field misalignment and no skipped resyncs. That evidence is stronger than any
  reading of the writer's source.
- **Nothing else may follow this crate's convention.** If a future crate wants to
  name the reference, it needs its own entry in
  [Inline exceptions](#inline-exceptions), justified on its own terms.

## somfy-api

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| DTO wire shape (camelCase field names, whole-percent `u8` positions rather than floats) | `somfy-api/src/entities.rs::ShadeDto`, `GroupDto`, `RoomDto`; `somfy-api/src/events.rs::ShadeStateEvent` | the reference firmware's REST/WS payload shape, kept for backup/migration parity with deployed devices | Verified by `somfy-api/tests/entities.rs::shade_dto_serializes_to_stable_json` (camelCase keys, whole-percent values) and `shade_dto_roundtrips`; the TypeScript shape is cross-checked by `somfy-api/tests/ts_export.rs::entities_use_camelcase_and_heapless_overrides` |
| `kind`/`tiltMode` reuse the numeric discriminants deployed devices already emit, rather than a string union | `somfy-api/src/entities.rs::ShadeDto` | the reference firmware's numeric `kind`/`tiltMode` wire fields (same discriminants recorded in the `somfy-domain` table above) | Verified by `somfy-api/tests/entities.rs::shade_dto_serializes_to_stable_json` and `somfy-api/tests/ts_export.rs::entities_use_camelcase_and_heapless_overrides` (asserts `kind`/`tiltMode` stay `number`, not a string union) |
| `direction` reuses the same sign convention on the wire (-1 up, 0 idle, +1 down) | `somfy-api/src/entities.rs::ShadeDto`, `somfy-api/src/events.rs::ShadeStateEvent` | the reference firmware's direction sign convention on the wire (same convention recorded for `somfy_domain::Direction` above) | Verified by `somfy-api/tests/entities.rs::shade_dto_snapshots_live_state` / `shade_dto_serializes_to_stable_json` |

## somfy-cc1101

Two different kinds of source meet in this crate, and the distinction matters.

The **target radio parameters** — what frequency, what modulation, what
bandwidth — came from the known-working reference configuration. They are a
statement about what the motors in the field are listening to, so they are
derived knowledge in exactly the sense the rules above describe, and they are
recorded here rather than in the source.

The **register byte values** were then worked out independently from the CC1101
datasheet formulas, and the arithmetic sits in the source next to each constant.
The datasheet is a primary vendor document for a part this project uses, not a
borrowed codebase, so naming it in a comment is not a reference-implementation
citation and Rule 1 does not apply to it. `docs/provenance.md` remains the only
place the *reference firmware* is named.

Where the two disagree, the disagreement is recorded rather than smoothed over.
Several values in this crate are **not derived at all**, and say so in their own
doc comments as well as here.

A third kind of source enters with the AGC registers: TI's own design note
**DN022, "CC110x/CC111x OOK/ASK Register Settings" (SWRA215E)**. The datasheet
declines to answer the ASK/OOK gain question itself — it states that the
settings its design tool produces for FSK/MSK "are not optimum" for OOK/ASK and
points at DN022 — and DN022 answers with measured recommendations, not formulas.
It is a primary vendor document for this part, like the datasheet, so naming it
in a source comment is not a reference-implementation citation. Its
recommendations are recorded below as recommendations, and where this project's
own measurements required departing from them, the measurement is recorded as
the reason.

A fourth kind of source enters with those same registers, and its provenance
chain is worth spelling out because it runs *through* the reference. The
reference firmware **does not configure the AGC itself**: it drives the radio
through a third-party Arduino driver library,
**`ELECHOUSE_CC1101_SRC_DRV` (lsatan/SmartRC-CC1101-Driver-Lib), v2.5.7**, and
takes that library's defaults. So the values are not the reference's design —
they are a widely deployed library's defaults, which the reference inherits and
which are field-proven for OOK remote work on this exact silicon. They are
adopted here as what they are: **empirical, externally validated, not derived**,
and then re-measured against a real transmitter before being committed. That is
the same category `PATABLE` already occupies. Source comments name the library,
which is not the reference implementation; this file records the chain.

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| Target radio parameters: 433.42 MHz carrier, deviation 47.6 kHz, "rxBandwidth" 99.97 kHz, TX power 11, ASK/OOK, asynchronous serial, CRC off, sync mode 4, address check off | Realised as register values throughout `somfy-cc1101/src/config.rs` | the reference firmware's transceiver configuration block | Hardware-verified: read back off a live production ESP32-S3 unit currently controlling three shades (see "Hardware-verified values") |
| `FREQ2`/`FREQ1`/`FREQ0` = 0x10/0xAB/0x85 | `somfy-cc1101/src/config.rs::FREQ` | Derived, not borrowed: datasheet `f_carrier = (f_xosc / 2^16) * FREQ` at 26 MHz, rounded to nearest | Arithmetic asserted at compile time (`ACHIEVED_CARRIER_HZ == 433_419_952`, 0.11 ppm low) and pinned byte-for-byte by `init_writes_the_ook_async_serial_register_set` |
| `DEVIATN` = 0x47 (DEVIATION_E = 4, DEVIATION_M = 7) | `somfy-cc1101/src/config.rs::DEVIATN` | Derived: datasheet `f_dev = (f_xosc / 2^17) * (8 + M) * 2^E`; E = 4, M = 7 is the only pair inside the field widths | Compile-time assertion `ACHIEVED_DEVIATION_HZ == 47_607` (7.4 Hz over the 47.60 kHz target). Inert in ASK/OOK — the datasheet states the setting has no effect in this modulation — and written only so the radio's state is wholly deliberate |
| `MDMCFG4` channel-bandwidth half = CHANBW_E 3, CHANBW_M 0 → **101.5625 kHz, not the 99.97 kHz requested** | `somfy-cc1101/src/config.rs::CHANBW_E` | Derived: datasheet `BW = f_xosc / (8 * (4 + M) * 2^E)`. The two 2-bit fields admit exactly 16 bandwidths at 26 MHz and 99.97 kHz is not one of them | Compile-time assertion `ACHIEVED_CHANBW_HZ == 101_562`. **Deliberate divergence:** the requested figure is unreachable on this part; 101.5625 kHz is 1.6% high, the next setting down (81.25 kHz) is 18.7% low. Not yet confirmed on air |
| `MDMCFG4` data-rate half + `MDMCFG3` = DRATE_E 5, DRATE_M 248 → 1562.1 baud | `somfy-cc1101/src/config.rs::DRATE_E`, `DRATE_M` | **Not from the reference at all** — chosen by this project. The reference configuration specifies no data rate, and one must be picked because the field shares a register with the bandwidth. Derived from this repo's own hardware-verified 640 µs RTS half-symbol (1562.5 chips/s) | Compile-time assertion that the achieved rate is within 0.1% of one chip per half-symbol. Unused in asynchronous-serial TX, where the modulator follows the pin; it only shapes the receive path, so it is untested on air |
| `MDMCFG2` = 0x34 (MOD_FORMAT 3 = ASK/OOK, SYNC_MODE 4, Manchester off) | `somfy-cc1101/src/config.rs::MDMCFG2` | Derived: field values read straight off the datasheet's `MDMCFG2` bit table for the requested modulation and sync mode | Compile-time assertion on the composed byte; pinned by `init_writes_the_ook_async_serial_register_set` |
| `PKTCTRL0` = 0x32 (async serial, CRC off, infinite length), `PKTCTRL1` = 0x00 (address check off), `ADDR` = 0x00 | `somfy-cc1101/src/config.rs` | Derived from the datasheet bit tables. Note both `WHITE_DATA` and `CRC_EN` reset to 1, so these writes actively disable them rather than confirming a default | Compile-time assertion on `PKTCTRL0`; all three pinned by the register-set test |
| `FREND0` = 0x11 (PA_POWER = 1) | `somfy-cc1101/src/config.rs::FREND0` | Derived: the datasheet requires OOK to take its logic-0 level from `PATABLE[0]` and its logic-1 level from `PATABLE[PA_POWER]`, so PA_POWER must be non-zero. The byte 0x11 itself appears nowhere in the datasheet; it is composed from the two field values | Compile-time assertion on the composed byte |
| `PATABLE` = [0x00, 0xC0] — **not derived** | `somfy-cc1101/src/config.rs::PATABLE_OOK` | A datasheet **table lookup**, not a formula: the PA register is a group of bias and ramp fields with no published relation to output power. 0xC0 is the tabulated setting for +10 dBm at 433 MHz | **Known gap.** The reference configuration asks for TX power 11 and the datasheet's only 433 MHz table has no +11 dBm row — it runs −30, −20, −15, −10, 0, +5, +7, +10 and stops. 0xC0 is that table's maximum. A +11 dBm row with the same byte 0xC0 exists in a different table, for 868/915 MHz parts with wire-wound inductors, so 0xC0 is the answer under either reading — but the exact provenance of the number 11 is unresolved. Not yet confirmed against measured output power |
| `MCSM0` = 0x14 (FS_AUTOCAL = 1) | `somfy-cc1101/src/config.rs::MCSM0` | **Added by this project; the reference configuration does not mention it.** The datasheet makes the `STX` strobe conditional on this field — from IDLE it "performs calibration first if MCSM0.FS_AUTOCAL = 1" — and the field resets to 0, "never". Without it a driver that parks in IDLE between frames transmits on an uncalibrated VCO while every register read still reports a healthy chip | Compile-time assertion on the composed byte. The failure it prevents has not been reproduced on hardware; it is a reading of the datasheet's strobe table, not an observed fault |
| Post-reset settle delay of 1 ms, taken with chip select still asserted — **not derived** | `somfy-cc1101/src/config.rs::RESET_SETTLE_NS` | Nothing: the datasheet bounds `SRES` completion only by a handshake (the chip drives MISO low when ready) and publishes no time. That handshake is unobservable through `embedded_hal::spi::SpiDevice`, which owns the chip-select line | **Margin choice, not a measurement.** Over an order of magnitude beyond the longest oscillator-settling figure the datasheet quotes anywhere (~600 µs), and paid once per boot. Not validated on hardware |
| `AGCCTRL2` = 0xC7 (MAX_DVGA_GAIN 3, MAX_LNA_GAIN 0, MAGN_TARGET 7) — **not derived; a field-proven library default, re-measured here** | `somfy-cc1101/src/config.rs::MAX_DVGA_GAIN`, `MAX_LNA_GAIN`, `MAGN_TARGET` | **Not the reference firmware's own value — it configures no AGC at all** and inherits this byte from the `ELECHOUSE_CC1101_SRC_DRV` library described above. DN022 separately recommends `AGCCTRL2` = 0x03 to 0x07, which varies only `MAGN_TARGET` and leaves `MAX_DVGA_GAIN` at 0; that range is unusable, and the reason is arithmetic rather than opinion: 0x03 **is the datasheet reset value**, so the whole recommended range describes the behaviour that made the receiver slice its own noise floor. The library's byte sits at the top of DN022's `MAGN_TARGET` range and outside it on `MAX_DVGA_GAIN` | **Hardware-measured against a real transmitter** (see "Hardware-verified values"). **Supersedes 0x83 (MAX_DVGA_GAIN 2, MAGN_TARGET 3), which this project chose on its own sweep and which was wrong.** That sweep ranked settings by edges per second in an *empty* band and picked the smallest cap that silenced it; silence in an empty band turns out not to predict whether a frame survives. Re-ranked against confirmed transmissions with the signal walked out of the channel filter to stand in for distance, 0x83/0x91 decoded **0 of 6** complete frames where 0xC7/0xB2 decoded **12 of 12**. `MAX_LNA_GAIN` still 0, both because gain removed there costs noise figure directly and because `CS_THRESHOLD_DBM_TENTHS` is read along that row. **Two costs recorded, neither measured:** the datasheet warns that a high `MAGN_TARGET` "reduces the headroom for blockers, and therefore close-in selectivity" — untested, no blocker set up — and the carrier-sense table puts the DVGA cap 18 dB up, a figure that demonstrably did *not* predict reception and is retained as indicative only |
| `AGCCTRL1` = 0x00 — **not derived** | `somfy-cc1101/src/config.rs::AGCCTRL1` | A **DN022 recommendation taken verbatim**, with no derivation offered by either document. It differs from the reset value 0x40 in one bit, `AGC_LNA_PRIORITY`, for which the datasheet presents two strategies and no rule for choosing. The two carrier-sense threshold fields are left at their reset zeros | Made **no measurable difference** to the noise on hardware: the swept measurements ran with this register at 0x00 throughout and the reset baseline they are compared against had 0x40, everything else identical. Followed because it is the vendor's answer for this modulation, not because anything here demonstrates it |
| `AGCCTRL0` = 0xB2 (HYST_LEVEL 2, WAIT_TIME 3, AGC_FREEZE 0, 12 dB OOK decision boundary) — **not derived; the same field-proven library default** | `somfy-cc1101/src/config.rs::FILTER_LENGTH_OOK_12DB`, `WAIT_TIME_32_SAMPLES`, `HYST_LEVEL_MEDIUM` | DN022 offers "0x91 **or** 0x92" for the boundary field and does not choose; the library takes the wider one and additionally slows `WAIT_TIME` from the reset 1 (16 channel-filter samples) to 3 (32). `FILTER_LENGTH` is overloaded by modulation — an averaging length under FSK/MSK, the OOK/ASK decision boundary under this one | **Supersedes 0x91, and the reason the old value was chosen is now understood rather than discarded.** The earlier note recorded 12 dB as a trap: GDO2 stopped producing edges but sat **high 99–100% of the time**, a receiver stuck asserting a carrier that is not there and indistinguishable from success to an edge count alone. That measurement was real and was taken at `MAX_DVGA_GAIN` = 2. The two fields are coupled — a wider boundary sits lower relative to the peak, so it pins the line high only if the gain in front of it is too high for the band. Re-measured: 12 dB at `MAX_DVGA_GAIN` 2 gives **54–101 edges/s idle, 22–39‰ high**; the same boundary at `MAX_DVGA_GAIN` 3 gives **0 edges/s, 0‰ high**. `WAIT_TIME` = 3 is adopted as part of a proven set, **not on isolated evidence** — no measurement here separates it from the two threshold changes it arrived with. Boundary-to-dB mapping asserted at compile time as `OOK_DECISION_BOUNDARY_DB` |
| Accepted `VERSION` values {0x04, 0x14} | `somfy-cc1101/src/config.rs::KNOWN_VERSIONS` | Derived: 0x14 is the current datasheet reset value; 0x04 is what the datasheet published before revision I, for older silicon | Exercised by `init_accepts_every_known_silicon_version` and `init_rejects_an_unexpected_version`. **Trade-off recorded:** the datasheet annotates `VERSION` "subject to change without notice", so genuine future silicon could be rejected. An allowlist was chosen over a "not 0x00 or 0xFF" denylist because a miswired bus returning a plausible byte would otherwise pass as a working radio |

## somfy-rmt

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| RMT RX idle threshold `IDLE_THRESHOLD_US` = 22000 µs — how long the air must stay quiet before the peripheral calls a transmission finished | `somfy-rmt/src/lib.rs::IDLE_THRESHOLD_US` | **Not derived; chosen by this project**, and chosen against measured captures rather than against `TIMINGS`. Its floor is `somfy_rts::MEASURED_MAX_INTRA_FRAME_SEGMENT_US` (17738 µs, measured) and its ceiling is `TIMINGS::INTER_FRAME_GAP` (27434 µs). **Corrects the design spec**, which specified `WAKEUP_LOW < idle_threshold < INTER_FRAME_GAP` and chose 12000 µs — an inequality the committed captures contradict, and a value that would end the reception one pulse into every real first frame | Floor hardware-measured (see the `somfy-rts` row above). Both margins asserted at compile time. Behaviour host-tested by `somfy-rmt/tests/idle_threshold.rs`, which replays the three real captures through a model of the peripheral's split rule and pins that 22000 leaves each capture whole while 12000 splits every one of them. **Ceiling not verified:** 27434 µs is *our* transmitter's gap, and no committed fixture contains a real remote's repeat frame to confirm a remote uses the same. **Not confirmed on air** |
| `MAX_IDLE_THRESHOLD_TICKS` = 32767 — the narrowest RMT idle-threshold field across the supported chips | `somfy-rmt/src/lib.rs::MAX_IDLE_THRESHOLD_TICKS` | Vendor register widths, not the reference implementation: the field is 16 bits on the ESP32 and ESP32-S2 and **15 bits on the ESP32-S3 and ESP32-C3**. A different register from the 15-bit *duration* field `MAX_TICKS` describes | Asserted at compile time in `crates/firmware/src/radio/rmt_rx.rs` against `esp_hal::rmt::MAX_RX_IDLE_THRESHOLD`, esp-hal's own per-chip constant, on all four chip builds. Note the design spec's claim of a 65535 µs ceiling holds only on two of the four chips |
| Reading a received symbol buffer back out as pulses: two entries per 32-bit symbol, and a zero-length entry ends the stream | `somfy-rmt/src/lib.rs::pulse_at`, `unpack`, `BurstCursor` | Not derived — the inverse of this crate's own `pack`, against the peripheral's documented end-marker convention. Note the terminator rule is **per entry**, not esp-hal's per-code `PulseCode::is_end_marker`; the two are not interchangeable, and using the latter would drop the last pulse of every odd-length burst | Host-tested by `somfy-rmt/tests/unpack.rs` (round trip of every frame shape through `build_symbols` and back) and `somfy-rmt/tests/cursor.rs`. `BurstCursor` holds the receive path's whole walk-a-burst state so the firmware keeps no index arithmetic of its own — the one place a receiver can silently hand back a stale pulse, and the reason it does not live in a crate no host test can reach. Not exercised against a real reception |
| Receive budget: a worst-case reception needs **95** symbols (188 recorded edges + the peripheral's terminator, two entries per symbol) — the same figure as the transmit worst case, but by different arithmetic | `somfy-rmt/src/lib.rs::MAX_SYMBOLS` (documented there; asserted in `crates/firmware/src/radio/rmt_rx.rs`) | Not derived. Computed from this project's own pulse trains | Pinned on the host by `somfy-rmt/tests/unpack.rs::a_worst_case_reception_fits_max_symbols` for all four frame shapes. **One symbol of slack** against `MAX_SYMBOLS` on the ESP32-S3 and ESP32-C3, where two RMT blocks is exactly 96 — a bound, not a margin. Whether the peripheral records anything beyond one entry per edge plus a terminator is **not measured** |

## somfy-mqtt

Two sources meet here and they pull in opposite directions, so the distinction
carries weight.

The **state topic layout** is reference-derived: the reference's MQTT
*publishing* works, and its topic shapes are what any existing broker
subscription or dashboard on a migrated estate is written against. The **Home
Assistant discovery contract** is not reference-derived at all — it comes from
Home Assistant, and it was verified against a live instance precisely *because*
the reference gets it wrong in every configuration. Copying the reference there
would reproduce the bug this crate exists to remove.

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| Per-shade state topic layout: `{state_root}/shades/{id}` as the base, with `position`, `direction`, `direction/set`, `target/set` and `name` below it | `somfy-mqtt/src/entity.rs::ShadeTopic::segments`, `somfy-mqtt/src/config.rs::SHADES_SEGMENT` | The reference firmware's own published topic layout, read off a live broker rather than out of its source: `espsomfyrts/shades/2/position :: 69`, `espsomfyrts/shades/3/name :: Roller shade br`, and the topic set named by its discovery payload (`position_topic`, `state_topic`, `command_topic`, `set_position_topic`). Adopted because the reference's *publishing* is the half that works, and because Plan 5 gains nothing by inventing a different arrangement of the same topics | Captured against a real broker on 2026-08-15; the capture is quoted in `docs/specs/2026-08-15-mqtt-ha-discovery-requirements.md`. Pinned on the host by `somfy-mqtt/tests/topics.rs::shade_topics_are_exact`. **Note this is topic *shape* only, not compatibility** — the plan puts C++ topic-layout compatibility explicitly out of scope, and the root segment differs |
| Availability at `{state_root}/status`, publishing `online`/`offline` | `somfy-mqtt/src/config.rs::STATUS_SEGMENT`, `MqttConfig::availability_topic` | The reference firmware's `espsomfyrts/status :: online`, with one deliberate departure: it must live under the **state root**, never under the discovery prefix. `{discovery_prefix}/status` is Home Assistant's own birth and will topic, so availability published there is overwritten by HA's birth message and reports the device available while it is offline | Pinned by `somfy-mqtt/tests/topics.rs::availability_is_under_the_state_root_not_the_discovery_prefix` and, from the payload side, by `somfy-mqtt/tests/round_trip.rs::the_check_actually_catches_availability_under_the_discovery_prefix`. The same collision is also reachable when the two roots are set to the same string — both individually valid — so `MqttConfig::new` refuses overlapping namespaces; pinned by `somfy-mqtt/tests/config_rejection.rs::overlapping_namespaces_are_rejected`. **This rule is not in the requirements spec**; it was found while implementing R4 and is reported as a spec gap |
| Discovery topic `{discovery_prefix}/{component}/{node_id}/{object_id}/config`, with the component **immediately** after the prefix | `somfy-mqtt/src/config.rs::MqttConfig::discovery_topic` | **Not the reference.** Home Assistant's MQTT discovery contract, which the reference does not implement correctly in any configuration | Verified empirically on 2026-08-15 by publishing both orderings to a live broker and observing which one Home Assistant acted on: `homeassistant/<device>/cover/1/config` was ignored, `homeassistant/cover/<device>/1/config` created an entity that read live position. Recorded in the requirements spec above; pinned by `somfy-mqtt/tests/topics.rs::component_is_the_segment_immediately_after_the_prefix` |
| `position_open: 0` / `position_closed: 100` in the cover discovery payload | `somfy-mqtt/src/entity.rs::CoverDiscovery::render` | **Not the reference.** Home Assistant's cover defaults are the other way round (100 open, 0 closed), while this project's `Pos` runs 0 fully open to 100 fully closed — see the `somfy-domain` table above. Stating both ends explicitly is what stops every shade reporting itself inverted | Not yet confirmed against a live Home Assistant. **This is the one value in this crate whose correctness a host test cannot establish**, and it is on the Plan 5 Task 5 integration checklist |
| `object_id` = `shade_{id}`, built from the shade's stable slot index and never from its name | `somfy-mqtt/src/ident.rs::ObjectId::for_shade` | **Not the reference, and a deliberate reading of R2.** R2 says `node_id` and `object_id` "MUST be sanitised to `[a-zA-Z0-9_-]`", which reads as though a shade's name flows into the object id. Building it from the id satisfies the character class more strongly — there is no user text to sanitise — and avoids a lifecycle cost R5 does not name: an object id that follows the name moves the discovery topic on every rename, stranding the old retained config as an orphan unless it is separately cleared. The requirements themselves note `object_id` "does not influence the entity_id", so the stable form costs nothing a user can see; the name still reaches Home Assistant through the payload's `name` field, which is where the display name comes from | Pinned by `somfy-mqtt/tests/topics.rs::renaming_a_shade_does_not_move_its_discovery_topic` and `a_shade_name_cannot_produce_topic_segments`, and as a property by `somfy-mqtt/tests/topic_props.rs::topics_are_invariant_under_the_shade_name` over hostile names (Unicode, slashes, wildcards, control characters, 400 bytes). Uniqueness across all 256 shade ids is exhaustively checked in `somfy-mqtt/src/ident.rs`'s own test module |

### Deliberate deviation from R8 (tilt)

**R8 requires tilt-capable shades to expose `tilt_command_topic` / `tilt_status_topic`. somfy-rs implements the mechanism but will publish them for no shade until the domain's tilt port lands.**

`somfy-domain` states in its own docs that `TiltMode` is config-carriage only: it
is stored and round-tripped for backup and migration, but no command drives a
tilt axis, and `Shade::tilt_pos` always reports `Pos::ZERO`
(`somfy-domain/src/shade.rs`, `somfy-domain/src/types.rs::ShadeConfig::tilt_mode`,
which additionally warns that the API layer "MUST NOT surface tilt as functional
until that port lands, or it will advertise moves the domain does not make").

Publishing tilt discovery for a shade whose stored `TiltMode` is non-`None`
would give Home Assistant a tilt control that reports 0 forever and moves
nothing. That is the failure acceptance criterion 5 of the requirements spec
calls out — "Appearing is not working" — and it is worse than an absent control,
because a control that does nothing reads as a device fault rather than an
unimplemented feature.

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| Tilt topics are carried only when the caller asks for them, via an explicit `has_tilt` argument rather than a read of the stored `TiltMode` | `somfy-mqtt/src/entity.rs::ShadeTopic::for_shade`, `somfy-mqtt/src/config.rs::MqttConfig::cover_discovery` | **Not derived. A deliberate deviation from spec R8**, resolving it against `somfy-domain`'s own contract, which wins because it describes what the firmware can actually do today | Omission is pinned by `somfy-mqtt/tests/topics.rs::tilt_topics_are_omitted_for_non_tilt_shades` and `somfy-mqtt/tests/round_trip.rs::non_tilt_shades_carry_no_tilt_keys`. **Revisit when the domain's tilt port lands** — when `Shade::tilt_pos` reports a real position and a command drives the tilt axis, the caller starts passing `true` and R8 is satisfied with no change to this crate |

## firmware

| Item | Where it lives now | Derived from | Verified |
|---|---|---|---|
| ESP32-S3 CC1101 pin map (SCK=12, MOSI=11, MISO=13, CSN=10, GDO0=3, GDO2=4) | `crates/firmware/src/chip.rs::pins` (chip-s3 module) | — (not derived; read directly off a running board) | Hardware-verified against a real working ESP32-S3 device on 2026-08-15 (see "Hardware-verified values") |
| ESP32, ESP32-S2 and ESP32-C3 CC1101 pin maps | `crates/firmware/src/chip.rs::pins` (chip-esp32, chip-s2, chip-c3 modules) | the reference firmware's per-chip default pin assignments, one board-configuration table per chip | Not verified on hardware. Only the ESP32-S3 map above has been checked against a real device; treat these three as unverified placeholders until someone confirms them on the corresponding board |
| RMT source clock fixed at 80 MHz with a matching divider of 80 (giving 1 µs ticks) | `crates/firmware/src/chip.rs::RMT_CLOCK_MHZ`, `RMT_CLK_DIVIDER` | the reference firmware's RMT clock configuration, required because the ESP32 and ESP32-S2 RMT peripherals only accept an 80 MHz source clock | Not independently hardware-verified for this crate; carried over as a constraint of the RMT peripheral itself, consistent with the tick model already exercised by `somfy-rmt`'s test suite |
| `MEMSIZE_BLOCKS` = 2 RMT memory blocks for the TX channel | `crates/firmware/src/radio/rmt_tx.rs::MEMSIZE_BLOCKS` | **Not derived.** Measured: a worst-case 80-bit first frame packs to 95 symbols including its end marker, and one block is 48 symbols on the smallest-block chips. The reference implementation is not a source here — it uses a different transmit mechanism entirely | Measured on the host by sweeping every payload byte value (`somfy-rmt/tests/build_symbols.rs::worst_case_symbol_count_is_pinned_and_fits_max_symbols`, `no_payload_byte_pattern_overflows_the_budget`); enforced at compile time against `esp_hal::rmt::CHANNEL_RAM_SIZE` on all four chip builds. Recorded in `docs/specs/2026-08-15-plan4-firmware-radio-design.md` §5.2. Not confirmed on air |
| One frame per RMT transaction — repeats are separate transmissions, never one concatenated buffer | `crates/firmware/src/radio/rmt_tx.rs::RmtTx::transmit_frame` | A consequence of the frame structure already recorded in the `somfy-rts` table above: the 56-bit form carries its inter-frame gap inside its own pulse train, while the 80-bit form suppresses that gap and re-encodes byte 7 per repeat, so an 80-bit repeat is a *different* frame rather than the same one sent twice | Follows from `render_pulses`' gap suppression and `encode80_byte7`, both already verified in `somfy-rts`. The resulting inter-frame spacing for 80-bit frames is not yet timed on air |
| `TX_SETTLE_US` = 1000 µs between strobing the radio into TX and clocking the first RMT symbol — **not derived** | `crates/firmware/src/radio/rmt_tx.rs::TX_SETTLE_US` | Nothing: a consequence of this project's own `MCSM0.FS_AUTOCAL` choice (see the `somfy-cc1101` table above). `STX` from IDLE calibrates the synthesiser before enabling the transmitter, `Cc1101::set_tx` returns as soon as the strobe is on the wire, and no carrier is keyed until calibration finishes | **Margin choice, not a measurement**, in the same spirit as `RESET_SETTLE_NS`: comfortably beyond the calibration and PLL-settling figures the datasheet quotes, and paid once per transmit burst rather than per frame. Not checked on a scope. Without it, the leading edge of the 10920 µs wake-up pulse is transmitted into a radio that is not yet radiating, shortening the frame with nothing reporting it |
| TX channel idles with the data pin driven **low** | `crates/firmware/src/radio/rmt_tx.rs::tx_channel_config` | Not derived: in asynchronous serial mode the CC1101 keys its carrier to follow the data pin, so leaving the pin at whatever level the last symbol ended on would hold the carrier up between frames | Not confirmed on air; a reading of the modulation mode this project configures the radio into |
| RX `MEMSIZE_BLOCKS` = 2 RMT memory blocks, and `RX_SYMBOLS` derived from that reservation rather than written down | `crates/firmware/src/radio/rmt_rx.rs::MEMSIZE_BLOCKS`, `RX_SYMBOLS` | Not derived. Same block count and same reasoning as the TX row above (a worst-case frame is 95 symbols; one block is 48 on the smallest-block chips). The buffer length is `MEMSIZE_BLOCKS * esp_hal::rmt::CHANNEL_RAM_SIZE` because the ESP32 and ESP32-S2 cannot wrap a reception around the end of channel RAM and esp-hal rejects a longer buffer outright | Compile-time assertion that `somfy_rmt::MAX_SYMBOLS` fits, on all four chip builds. **Not confirmed on air** — no reception of any length has yet been taken with this firmware |
| RX glitch filter left **off** (`filter_threshold = 0`) | `crates/firmware/src/radio/rmt_rx.rs::rx_channel_config` | Not derived. The field is a `u8`, so under any plausible reading of its unit it reaches at most a few hundred microseconds — short of the ~448 µs glitch class a real capture of this protocol contains (see `somfy-rts/tests/fixtures/README.md`), and not wideable to reach it | **A deliberate non-choice, not a measurement.** Enabling it would add an unobserved hardware behaviour in exchange for filtering nothing the decoder does not already reject as an out-of-family duration. Worth revisiting during on-air bring-up, with a measurement |
| CC1101 SPI clock of 4 MHz | `crates/firmware/src/main.rs::SPI_HZ` | Datasheet bound, not the reference implementation: the part accepts 10 MHz for single-byte access but only 6.5 MHz for the burst reads `somfy-cc1101` uses for status registers and the PA table | Margin choice below the tighter of the two datasheet limits. Not measured |
| RMT channel numbers: transmit on channel 0 everywhere, receive on channel **4** for the ESP32-S3 and channel **2** for the ESP32, ESP32-S2 and ESP32-C3 | `crates/firmware/src/chip.rs::rmt_channels!` | Derived from `esp-metadata-generated`'s per-chip RMT channel tables, which say which channel indices exist in which direction (ESP32/S2: either direction on any channel; S3: 0-3 TX, 4-7 RX; C3: 0-1 TX, 2-3 RX). The receive channel is never the one immediately after the transmit channel because `memsize = 2` makes channel 0 own block 1 as well | Enforced by the type system on all four builds — `RxChannelCreator` simply is not implemented for a channel that cannot receive — and confirmed at run time on an ESP32-S3, where the receiver armed without reporting `InvalidDataLength`. **No reception has been decoded on air** |
| The radio is put into RX after `init` and after every burst | `crates/firmware/src/radio/air.rs::Air::listen`, `key_off`; `somfy-cc1101/src/lib.rs::Cc1101::set_rx` | Datasheet: `SRX` (0x34) is the receive strobe, and in asynchronous serial mode the packet handler is bypassed, so there is no packet end for `MCSM1.RXOFF_MODE` to act on and reception continues until something strobes the chip out of it | The strobe byte is pinned by `the_mode_strobes_are_srx_stx_and_sidle`. **The claim that reception persists is a reading of the datasheet, not a measurement** — an unstrobed radio and a quiet band look identical, which is exactly why the strobe was missing until now |

---

## Inline exceptions

Comments that still name the reference implementation, with justification.
Every entry here is a deliberate decision, reviewed. An inline reference that
is not listed here should be removed.

| Location | Why it must stay |
|---|---|
| **`crates/somfy-migrate/**` (whole crate)** | The crate reads backup files *produced by* the C++ ESPSomfy-RTS firmware. That codebase is the subject matter, not a source of borrowed knowledge — naming the routine that wrote a field describes the input, and removing it would obscure the single most important fact about the data. Scoped to this crate only; see [somfy-migrate](#somfy-migrate). |

No other exception has been needed. The four protocol/domain/API crates were
cleaned with **zero** retained references: every citation there proved
restatable in this project's own terms.

---

## Hardware-verified values

Values confirmed against real hardware, which outrank any derivation:

| Value | Evidence | Date |
|---|---|---|
| 56-bit first frame emits 2 hardware syncs; repeat emits 7 | Captured wall-remote frames reported `hwsync` 4 and 14 half-pulses respectively | 2026-08-15 |
| ESP32-S3 CC1101 pin map (SCK=12, MOSI=11, MISO=13, CSN=10, GDO0_TX=3, GDO2_RX=4) | Read directly off a running production ESP32-S3 board's wiring | 2026-08-15 |
| Wake-up pulse ≈ 10.9 ms HIGH | First pulse of a real capture measured 10229 µs | 2026-08-15 |
| Post-wake-up silence ≈ 17.7 ms LOW — **not** the 7357 µs `WAKEUP_LOW` this project transmits | Second pulse of all three real captures: 17738 / 17722 / 17711 µs. It is the longest LOW inside a real frame, and the only one above the hardware-sync family | 2026-08-15 (measured from the committed captures 2026-08-16) |
| Backup file format and record layout | Parsed a real v25 device backup with zero field misalignment | 2026-08-15 |
| Target radio configuration: 433.42 MHz, deviation 47.6, rxBandwidth 99.97, txPower 11, 56-bit frames | Read off a live production ESP32-S3 + CC1101 unit then controlling three shades, reporting `radioInit: true` | 2026-08-15 |
| A committed rolling code survives reset, a full reflash, and a ring wrap | `store-check` on a spare ESP32-S3 counted from `RollingCode(1)` to `RollingCode(43)` across ~45 reboots without ever restarting: the ring wrapped at the 33rd commit (`valid` 32 → 17 as a sector was erased, `damaged` 0 throughout), an `espflash flash` mid-run did not disturb it, and swapping the partition table out and back left the region intact. Procedure in `docs/hardware-checklist.md` | 2026-08-16 |
| A half-written record is rejected, and the commit after it steps over the wreckage | A sector image with a complete record in slot 0 and the first 64 bytes of one in slot 1 was written to the region directly. The store reported `valid=1 damaged=1`, loaded the **complete** record's code rather than the torn one, and committed into slot 2 | 2026-08-16 |
| Unconfigured AGC makes the receiver slice its own noise floor: **~750 edges/s on GDO2 with nothing transmitting**, line high ~70% of the time | A temporary bring-up binary — deliberately **not committed**, unlike `store-check` and `tx-check` — sampling the GDO2 pad directly as a plain GPIO input on a spare ESP32-S3, bypassing RMT entirely, with the radio confirmed in RX (`MARCSTATE` = 0x0D). Measured with `AGCCTRL2`/`AGCCTRL1`/`AGCCTRL0` written explicitly to their datasheet **reset** values 0x03/0x40/0x91 — i.e. the state this crate left them in — as 1461, 1598 and 1572 edges per 2-second window. This is also the state DN022's recommended range describes, so following the app note verbatim would not have fixed it | 2026-08-16 |
| Capping DVGA gain silences it: **~0.6 edges/s, line resting low** | The same uncommitted binary on the same board, unchanged, with `AGCCTRL2` = 0x83 now written by `Cc1101::init`: 36 consecutive 1-second windows gave 22 edges in total, 22 of the 36 windows completely flat, `high` = 0% throughout. Mean gap between events is seconds, against the 22 ms of quiet `somfy-rmt`'s idle threshold needs. Sweep behind the choice, per 2-second window at `MAX_LNA_GAIN` = 0 and `MAGN_TARGET` = 3: `MAX_DVGA_GAIN` 0 → 1461/1598/1572, 1 → 58/104/78, 2 → 0/2/0, 3 → 0/0/0 | 2026-08-16 |
| **A silent idle band does not mean a receivable one, and ranking AGC settings by idle noise picked the worst of them.** `AGCCTRL2`/`AGCCTRL0` = 0x83/0x91 receives whole frames only from a strong transmitter; attenuate it and the frame body is lost while the wake-up pulse still arrives — the exact symptom that prompted this work | A second uncommitted binary on the spare ESP32-S3, again sampling GDO2 as a plain GPIO input with RMT bypassed, stepping the three AGC registers between measurements and firing a real transmission at each setting from a second, known-good ESPSomfy-RTS device over HTTP. Every capture is backed by a **verified shade movement** (position 0↔100) and judged by running the recorded pulses back through this project's own `somfy_rts::rx::RxDecoder` + `decode56`: a capture counts only if it yields a **checksum-valid 56-bit frame**. Decoded frames carried the transmitter's real address (1032469) with rolling codes advancing 96→155 across the runs, and commands matching what was fired. Distance was simulated by writing `FSCTRL0.FREQOFF` to walk the signal out of the 101.6 kHz channel filter (1586.9 Hz per count) — a test-time attenuator only, never written by `init`. Complete frames decoded, three attempts per cell: <br>`offset` → `38 kHz / 51 kHz / 64 kHz / 76 kHz` <br>`0x83/0x00/0x91` (old) → **0/3, 0/3, –, –** <br>`0xC7/0x00/0xB2` (new) → **3/3, 3/3, 3/3, 3/3** <br>`0x47/0x00/0x91` → 2/3, 3/3, 2/3, 3/3 <br>`0x87/0x00/0x91` → –, –, 1/3, 2/3 <br>Unattenuated, every one of these decoded 4/4, which is why the earlier quiet-room sweep could not rank them | 2026-08-16 |
| `AGCCTRL2`/`AGCCTRL1`/`AGCCTRL0` = **0xC7/0x00/0xB2** leaves the band silent *and* the line resting low | Sixteen 2-second idle windows across three runs, radio in RX, nothing transmitting: **0 edges and 0‰ high in every one**. Confirmed independently by the original `rx_raw` diagnostic against the committed configuration — 150 consecutive 200 ms windows over 30 s produced **no window with more than 4 edges**. The line-resting-low half is measured, not inferred, precisely because the setting this replaced was rejected for pinning the line high while producing no edges at all | 2026-08-16 |
| A **complete** 56-bit Somfy frame is captured in a single 200 ms window: `w240: edges=92 longest_high=10240us longest_low=29468us` | `rx_raw`, unmodified, running the committed 0xC7/0x00/0xB2 configuration, during a transmission confirmed by the shade travelling 100 → 0. 92 edges with a 10240 µs wake-up HIGH. **92 edges is a whole frame, not a fragment:** this repository's own golden capture of a real device reports `pulseCount=89` for a 56-bit frame (`somfy-rts/tests/fixtures/up_56bit_1.pulses`), so an edge count in the high 80s to low 90s is the complete article. The two neighbouring firings split across a window boundary (70+20 and 73+19 edges) purely because `rx_raw`'s windows free-run rather than triggering on the frame | 2026-08-16 |
| Main stack available to the controller: **304,652 bytes** on an ESP32-S3 with 8 MB flash | `firmware` printed `stack: 304652 bytes available, 8192 required` at boot. esp-hal's linker script gives the stack whatever DRAM is left after the statics, so this figure is specific to this build and moves as statics are added — which is why it is checked at boot rather than asserted. It comfortably covers the ~6.5 KB `RmtTx::transmit_frame` needs, and settles the Plan 4a concern that an Embassy task might not get it: tasks have no stacks of their own, so this is the stack they all run on | 2026-08-16 |
