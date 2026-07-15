# Golden pulse captures

Source: ESPSomfy-RTS C++ firmware (this project's reference device). These
fixtures pin the Rust engine against pulses the real hardware actually produced,
so the folklore-vs-reality timing/polarity decisions in `pulse.rs` and `rx.rs`
are settled by capture, not by assumption. Once captured, the files are
committed and CI never needs hardware.

The C++ RX struct `somfy_rx_t` (`src/Somfy.h:95-116`) stores raw pulse
**durations** in `pulses[MAX_TIMINGS]` — `unsigned int pulses[]`, one entry per
edge, **no level bit**. Everything below follows from that fact.

## What the C++ actually records (read before capturing)

The receive ISR is `Transceiver::handleReceive` (`src/Somfy.cpp:4384-4516`).
Two behaviours change how a raw `pulses[]` dump must be interpreted:

1. **Durations only, no levels.** Each `pulses[]` entry is
   `micros() - last_time`, the time since the previous recorded edge. The level
   of each segment is *not* stored. Because the receiver runs off a `CHANGE`
   interrupt, the physical line strictly alternates HIGH/LOW at every recorded
   edge, so levels can be reconstructed by **alternation from a known phase**.
   The first pulse of a transmission is the wake-up / first hardware-sync pulse,
   which is **HIGH** (`render_pulses`, `pulse.rs`), so:

   ```
   pulse[0] = HIGH, pulse[1] = LOW, pulse[2] = HIGH, ...
   ```

   The loader in `../golden.rs` performs exactly this reconstruction when it is
   given a durations-only file.

2. **Sub-448µs glitches are logged but ignored.** On a segment shorter than
   `bitMin` (`SYMBOL * TOLERANCE_MIN = 640 * 0.7 = 448µs`), the ISR does this
   (`Somfy.cpp:4388-4395`):

   ```c
   if (duration < bitMin) {
       if (pulseCount < MAX_TIMINGS && cpt_synchro_hw > 0)
           pulses[pulseCount++] = duration;   // logged into rx.pulses[]
       return;                                // <-- last_time NOT advanced
   }
   last_time = time;                          // only reached for real edges
   ```

   So a glitch **appears in `pulses[]`** even though the decoder skips it, and
   `last_time` is left untouched — meaning the *next* real duration is measured
   from before the glitch and already spans it. The loader therefore just
   **drops** every sub-448µs entry; it does **not** merge the glitch duration
   into the following pulse (the C++ already did that implicitly). Dropping
   also happens *before* level reconstruction, so the surviving edges keep their
   strict alternation and the HIGH/LOW phase stays correct.

## Capture procedure

1. Enable the firmware's transceiver debug output (Radio settings → enable pulse
   logging), **or** add a temporary dump of `rx.pulses[0..pulseCount]` in
   `src/Somfy.cpp` right where a completed frame is processed
   (`somfy_rx.status == complete`).
2. Press a paired remote button near the device, once per capture file.
3. Save one line per pulse. Either supported format is accepted (see below);
   the durations-only format is the natural dump of `rx.pulses[]`.
4. Do **not** hand-edit durations. If a capture will not decode, capture again —
   the file is the authority, the code bends to it (see golden.rs Step 4).

## Supported file formats

The loader (`load` / `parse_pulses` in `../golden.rs`) accepts both:

- **Durations-only** — `<duration_us>` per line. Levels are reconstructed by
  alternation from HIGH. This is the direct dump of `rx.pulses[]`.

  ```
  2560
  2560
  4850
  1280
  ...
  ```

- **Level + duration** — `<level 0|1>,<duration_us>` per line. Levels are taken
  as written (no reconstruction). Useful when a capture tool already emits
  levels, or for hand-authored regression cases.

  ```
  1,2560
  0,2560
  1,4850
  ...
  ```

Common rules for both formats:

- Lines that are blank or start with `#` are skipped (comments).
- Any entry shorter than 448µs is dropped as a glitch, regardless of format.
- A file must be a single format; do not mix commas and bare durations.

## File naming

`<command>_<bits>bit_<n>.pulses`, e.g. `up_56bit_1.pulses`. `<n>` disambiguates
multiple captures of the same command. Each file's expected decode result is
asserted in `../golden.rs`.

## Expected fixtures (device captures — PENDING)

These are **not yet committed**: they require one capture session on the
author's running C++ ESPSomfy-RTS device. The corresponding tests in
`../golden.rs` are marked `#[ignore]` until the files exist, so the suite stays
green in the meantime. Un-ignore each test when its file lands.

| File                 | Expected decode      | Status  |
|----------------------|----------------------|---------|
| `up_56bit_1.pulses`  | `Command::Up`        | PENDING |
| `down_56bit_1.pulses`| `Command::Down`      | PENDING |
| `my_56bit_1.pulses`  | `Command::My`        | PENDING |
| `*_80bit_1.pulses`   | 80-bit (if a compatible remote is available) | OPTIONAL / PENDING |

If no 80-bit-capable remote is available, note its absence here and defer the
80-bit fixture; the 56-bit fixtures are sufficient to settle polarity, sync
counts, and timing.

## Synthetic fixtures (checked in, no hardware)

`synthetic_up_56bit.pulses` is **not** a real capture. It is generated
deterministically from the crate's own transmitter so the loader itself
(parsing, level reconstruction, glitch filtering, end-to-end decode) is
validated on every CI run without waiting for device captures. It carries a
`# SYNTHETIC` header so it is never mistaken for a real capture.

Recipe (mirrored by `synthetic_up_pulses` in `../golden.rs`):

1. `render_pulses(&encode56(&f), FrameKind::Repeat, &mut out)` for
   `f = { key: 0xA7, command: Up, rolling_code: 0x000A, address: 0x00C0DE }`.
2. Merge adjacent same-level half-symbols into edge-to-edge segments — the shape
   a `CHANGE`-interrupt receiver produces. The merged stream strictly alternates
   from HIGH, matching a real `rx.pulses[]` dump.
3. Write one duration per line, durations-only, and inject one sub-448µs line so
   the checked-in file also exercises the glitch-drop path.
