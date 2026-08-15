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

> **Do not use `printBuffer`.** The `transceiver_config_t::printBuffer` flag is
> vestigial in v2.5.6 — its only assignment sits inside the commented-out block
> that closes at `Somfy.cpp:4766`, so it can never be set and nothing reads it.
> An earlier version of this document recommended it; that advice was wrong.

The working route needs **no firmware modification**. The device already emits
every decoded frame, with its full raw pulse array, over its websocket:

1. Connect a websocket client to `ws://<device>:8080` and send the text frame
   `join:0` to join `ROOM_EMIT_FRAME` (`Sockets.cpp:166-171`). `emitFrame` is
   gated on that room having at least one client (`Somfy.cpp:4603`).
2. Press a paired remote button near the device. Each decoded frame arrives as
   `42[remoteFrame,{...}]` — socket.io-like, but note the event name is emitted
   **unquoted** (`WResp.cpp:8`), so the message is not strict JSON and needs
   splitting at the first comma before the payload can be parsed.
3. The payload's `pulses` array is a verbatim `rx.pulses[]` dump; write one
   entry per line. It also carries `command`, `bits`, `sync`, `rssi`, `address`
   and `rcode`, so captures can be sorted by button automatically and each
   file's expected decode is known without guesswork.
4. Either supported format is accepted (see below); durations-only is the
   natural dump of `rx.pulses[]`.
5. Do **not** hand-edit durations. If a capture will not decode, capture again —
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

## Device captures (LANDED 2026-08-15)

Captured from a **physical Somfy wall remote** received by an ESP32-S3 + CC1101
running ESPSomfy-RTS v2.5.6, via its `remoteFrame` websocket event
(`Somfy.cpp:4602-4625`), whose `pulses[]` array is a verbatim dump of
`rx.pulses[]`. All three tests in `../golden.rs` pass unmodified — the engine
decoded genuine Somfy hardware on the first attempt, with no code changes.

| File                 | Expected decode | hwsync | pulses | Status |
|----------------------|-----------------|--------|--------|--------|
| `up_56bit_1.pulses`  | `Command::Up`   | 4      | 89     | LANDED |
| `down_56bit_1.pulses`| `Command::Down` | 4      | 97     | LANDED |
| `my_56bit_1.pulses`  | `Command::My`   | 4      | 84     | LANDED |
| `*_80bit_1.pulses`   | 80-bit          | —      | —      | DEFERRED — no 80-bit-capable remote available |

The capture session also independently confirmed the sync model: first frames
reported `hwsync == 4` and repeat frames `hwsync == 14`, exactly the 2-and-7
hardware-sync counts `pulse.rs` renders and `rx.rs` expects.

## ⚠️ BEFORE MAKING THIS REPOSITORY PUBLIC

> **These three fixtures contain a real remote's radio address and rolling
> codes.** The pulse train *is* the frame: address `1772642` and rolling codes
> in the 8792–8809 range are recoverable by anyone who decodes these files —
> the same class of secret that `somfy-migrate`'s fixtures README says to treat
> like a key, and which is gitignored there for exactly that reason.
>
> The repository is private today, so nothing is exposed. **It is intended to
> become public** (design spec §1.1, "community-adoptable"). Before that
> happens, do one of:
>
> 1. **Re-capture with a throwaway address** (preferred). Configure a spare
>    ESPSomfy-RTS device with a virtual remote address paired to nothing, have
>    it transmit, and capture with a second device. This needs **two working
>    radios** — one transmitting, one receiving — because a radio cannot hear
>    its own transmission. Same test value, nothing sensitive.
> 2. **Delete these files and re-`#[ignore]` the three tests**, falling back to
>    the synthetic fixture for CI.
>
> Do **not** attempt to "anonymise" by rewriting the address in place: the
> address is Manchester-coded inside the payload, so changing it requires
> re-encoding through this crate's own encoder — which makes the golden test
> circular (our encoder agreeing with our decoder, which the loopback property
> tests already cover) and forfeits the checksum/obfuscation validation that
> makes a real capture worth having.

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
