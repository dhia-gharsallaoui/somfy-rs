# somfy-rs Plan 4 — Firmware Radio: design

> Refines §3.5, §4 and §5 of [`2026-07-15-rust-rewrite-design.md`](2026-07-15-rust-rewrite-design.md).
> The parent spec chose `esp-hal` + Embassy and RMT-based radio; it did not say
> how the firmware crate coexists with the host workspace, how the pulse model
> crosses into RMT hardware, or what "done" means for this plan. This document
> says all three.

**Status:** approved in brainstorming, 2026-08-15.
**Predecessors:** Plans 1–3 complete (`somfy-rts`, `somfy-domain`, `somfy-api`,
`somfy-migrate`), 188 host tests green.

## 1. What Plan 4 delivers

A `firmware` crate that, on real hardware, **moves a shade from a command and
keeps tracked position honest when a wall remote is used**. That sentence is the
acceptance test; everything below serves it.

Concretely:

- CC1101 SPI driver (async-serial mode) and RMT TX/RX drivers.
- An Embassy **radio task** that is the sole owner of the CC1101 and both RMT
  channels, fed by a bounded `TransmitRequest` channel.
- An Embassy **state task** owning the Plan 2 `Controller`, applying commands and
  received frames, running position ticks, and publishing state deltas.
- A minimal **rolling-code store** in flash, so the persist-before-TX invariant
  is honoured from the first frame rather than deferred.

Explicitly **not** in Plan 4: WiFi, MQTT, HTTP, OTA, the config store for
anything other than rolling codes, and the web UI. The state task's delta
publisher will have no consumers until Plan 5 — that is intended, and is the
seam Plan 5 plugs into.

## 2. Decisions locked during brainstorming

| Decision | Choice | Consequence |
|---|---|---|
| Hardware available | Spare ESP32-S3-DevKitC-1 + CC1101, **plus** the running C++ v2.5.6 device | Golden captures are obtainable; on-air validation is in scope |
| Vertical slice | Radio task **wired to `somfy-domain`** | Plan 2 stops being dormant; Plan 5's seam is real code |
| Chip targets | **All four** — ESP32, S2, S3, C3 | 4-way CI matrix; no porting debt accrues |
| Rolling codes | **Minimal persisted counter**, nothing else persisted | Invariant honoured from frame one; survives reflashing |
| RX strategy | `PulseSource` trait; RMT RX primary, GPIO-interrupt fallback | Spec §5.3's recorded contingency becomes a swap, not a rewrite |

## 3. Workspace and toolchain

`crates/firmware` **must not** join the root workspace. `cargo test --workspace`
would try to build `esp-hal` for the host, which cannot work. Therefore:

- Root `Cargo.toml` gains `exclude = ["crates/firmware"]`.
- `crates/firmware` becomes its own workspace with its own `rust-toolchain.toml`
  pinned to espup's `esp` channel, which carries both the Xtensa targets and the
  RISC-V ones.
- Repo root keeps `channel = "stable"` for the four host crates.
- `firmware` depends on `somfy-rts` and `somfy-domain` by path; path
  dependencies cross workspace boundaries without issue.

The property being protected: **`cargo test --workspace` at the repo root stays
exactly as fast and as green as it is today**, with no ESP toolchain installed.
A contributor working on the protocol or domain layer never pays for the
firmware.

## 4. Chip matrix and CI

`esp-hal`'s chip features are mutually exclusive, so "all four chips" means four
builds, not one:

| Feature | `esp-hal` feature | Target triple |
|---|---|---|
| `chip-esp32` | `esp32` | `xtensa-esp32-none-elf` |
| `chip-s2` | `esp32s2` | `xtensa-esp32s2-none-elf` |
| `chip-s3` | `esp32s3` | `xtensa-esp32s3-none-elf` |
| `chip-c3` | `esp32c3` | `riscv32imc-unknown-none-elf` |

- **No default chip feature.** A bare `cargo build` hits a `compile_error!`
  naming the four options rather than silently selecting one.
- CI gains a 4-way matrix running, per chip, **`clippy -D warnings` first, then
  `build`**. Lint parity extends to cross-compiled code, not just host code.
- The RMT base clock **must be 80 MHz on ESP32 and ESP32-S2** (an `esp-hal`
  constraint). This becomes a per-chip constant with the constraint named in a
  comment, never a bare literal.

## 5. TX

`somfy-rts::render_pulses` deliberately does **not** merge adjacent same-level
half-symbols, stating that merging "is the concern of a later RMT-encoding
layer" (`pulse.rs:63-64`). **Plan 4 is that layer.** The pipeline:

```
Frame -> encode56/encode80 -> render_pulses -> [Pulse{high, micros}]
      -> merge adjacent same-level runs
      -> pack into [PulseCode]  (2 pulses per 32-bit symbol)
      -> RMT TX channel -> CC1101 GDO0
```

### 5.1 Tick resolution

1 µs per tick: RMT source clock 80 MHz with `clk_divider = 80`. Justification —
an RMT length field is **15 bits**, so a single pulse tops out at 32,767 ticks.
The longest Somfy pulse is `INTER_FRAME_GAP` at **27,434 µs**, which fits with
about 16% headroom. A `const` assertion pins this so that a future timing change
which would overflow the field fails the build instead of truncating silently on
air.

### 5.2 Buffer sizing — the tight constraint

Worst-case pulse counts, from the timing model (worst case is an all-ones or
all-zeros payload, where no adjacent half-symbols merge):

| Frame | Pulses | `PulseCode` symbols |
|---|---:|---:|
| 56-bit, first (wake-up + 2 hw syncs + gap) | 121 | 61 |
| 80-bit, first (wake-up + 12 hw syncs, gap suppressed) | **188** | **94** |

An RMT memory block holds 48 symbols on the S3 and C3 and 64 on the ESP32, so an
80-bit first frame needs **two blocks (96 symbols) and leaves room for barely one
end-marker**. That is uncomfortably tight and is the **first thing to measure on
hardware**.

Two caveats on those numbers. The S2's block size is **not verified** at the time
of writing, and block size is chip-dependent generally — so the implementation
must read it from `esp-hal`'s own constants and assert against the computed
requirement at compile time. **Do not hardcode the figures from this document.**
If two blocks prove insufficient on any chip, the fallback is RMT's wrap-around
refill mode, feeding the channel in halves from an interrupt — more code, same
external behaviour. Recorded as a risk in §11 rather than assumed away.

### 5.3 Repeats

Repeat frames are separate RMT transactions. The 56-bit form carries its
inter-frame gap at the end of its own pulse train; the 80-bit form suppresses the
gap (`Somfy.cpp:4379`), so repeat spacing for 80-bit frames is the transmitter's
responsibility, not the pulse renderer's.

## 6. RX

`RxDecoder` already accepts **merged edge-to-edge** pulse streams
(`lib.rs:29-31`) — which is exactly the representation RMT hands back. **No
decoder change is required.**

### 6.1 The `PulseSource` trait

The parent spec records RMT RX as a risk with GPIO-interrupt timestamping as the
contingency (§5.3, §12). Plan 4 makes that contingency structural: a
`PulseSource` trait yielding measured `Pulse` values, with

- `RmtPulseSource` — primary implementation, and
- `GpioPulseSource` — interrupt-timestamping fallback,

behind the same interface. The radio task is written against the trait, so
switching costs a type parameter, not a redesign under pressure on hardware.
This is the whole justification for the abstraction; it exists to discharge a
recorded risk, not for generality.

### 6.2 Idle threshold

The RMT RX idle threshold must sit **above the longest in-frame LOW** and **below
the inter-frame gap**, so that each repeat frame lands as its own completed
transaction rather than being concatenated or truncated:

```
WAKEUP_LOW (7,357 µs)  <  idle_threshold  <  INTER_FRAME_GAP (27,434 µs)
```

Chosen: **12,000 µs**. The field is a `u16` of ticks, so at 1 µs resolution the
ceiling is 65,535 µs — the chosen value is comfortably representable.

## 7. Tasks and the persist-before-TX invariant

Two Embassy tasks, statically allocated, communicating over bounded channels:

- **radio** — sole owner of the CC1101 and both RMT channels. Consumes
  `TransmitRequest`s from a bounded channel; publishes decoded frames. Radio
  timing never blocks on anything else.
- **state** — owns the `somfy-domain` `Controller`. Applies commands and received
  frames, runs position-estimator ticks, and publishes state deltas on a watch.

### 7.1 Structural enforcement, not discipline

The parent spec's critical invariant is that the incremented rolling code
reaches flash **before** the frame transmits; a crash in between de-syncs the
motor pairing. Plan 4 enforces this by construction:

> A single `transmit()` helper takes the `RollingCodeStore`, commits, and *then*
> enqueues the `TransmitRequest`. Nothing else can enqueue — the channel's
> producer end is not exposed. No call site can get the order wrong because no
> call site can reach the channel directly.

The store is a trait. Its Plan 4 implementation is a minimal append-only,
wear-levelled counter region — rolling codes only, no other configuration. Plan 6
replaces the backing implementation; the seam and the ordering guarantee stay
put.

## 8. Testing

The project's culture is that logic is host-tested and hardware is a smoke
checklist. Plan 4 keeps that line by making the interesting parts pure functions
over `Pulse`:

**Host-tested, no hardware:**

- Merge of adjacent same-level pulses, and packing into `PulseCode` — including
  the all-ones/all-zeros worst case that drives the §5.2 sizing.
- Symbol-count and memory-block computation per frame kind and per chip.
- Idle-threshold selection against the §6.2 inequality (a property test: for the
  ported timing constants, the chosen threshold separates frames).
- **Ordering:** a mock `RollingCodeStore` asserts `commit` precedes enqueue, and
  that a failed commit means *no* transmission.
- CC1101 driver register writes, against an `embedded-hal` SPI mock, asserted
  against the C++ register table.

**On hardware, documented manual checklist:** capture session, TX to a motor,
overheard-remote decode, reflash-survives-rolling-code.

## 9. Prerequisite in `somfy-rts`

`encode80` must grow its `repeat` parameter **before any 80-bit frame reaches
hardware**. The C++ reference re-encodes byte 7 per repeat as `196 + 4*repeat`,
with Favorite/Stop flipping `196 → 132` on later repeats; today's repeat-less
`encode80` emits only the first-frame form. This is already recorded as a Plan 4
obligation in the repository README and is a change to `somfy-rts`, not to
`firmware`.

## 10. Hardware reference

Verified on the development board, 2026-08-15: ESP32-S3 rev v0.2, 8 MB flash
(quad per eFuse), 40 MHz crystal, CP2102N bridge on `/dev/ttyUSB0`.

Pin map — **identical to the running C++ device** (read from its
`/controller` endpoint), so firmware defaults match a working installation and
C++/Rust comparisons are like-for-like:

| CC1101 | S3 GPIO | Role |
|---|---|---|
| VCC | 3V3 | 3.3 V only; not 5 V tolerant |
| GND | GND | |
| CSN | GPIO10 | SPI chip select |
| SCK | GPIO12 | SPI clock |
| MOSI / SI | GPIO11 | ESP → CC1101 |
| MISO / SO | GPIO13 | CC1101 → ESP |
| GDO0 | GPIO3 | TX data (`Somfy.cpp:4985` — `setGDO(TXPin, RXPin)`) |
| GDO2 | GPIO4 | RX data |

Constraints worth carrying: **GPIO3 is an S3 strapping pin** (JTAG source
select) and the CC1101 drives GDO0 at power-up until configured — proven fine in
production, but the first suspect for any boot anomaly. **GPIO33–37** are
reserved for octal PSRAM on `R8` modules. **GPIO19/20** are native USB D−/D+.

Radio settings from the same source: 433.42 MHz, deviation 47.60 kHz, RX
bandwidth 99.97 kHz, TX power 10, OOK/ASK modulation, async serial packet
format.

## 11. Risks

| Risk | Mitigation |
|---|---|
| 80-bit frame (94 symbols) barely fits two RMT memory blocks | Measure first, on hardware, before building on it; fall back to wrap-around refill |
| RMT RX unsuitable for long frames | `PulseSource` trait with a GPIO-interrupt implementation ready (§6.1) |
| Xtensa toolchain friction across three of the four chips | `espup`-pinned toolchain in a firmware-local `rust-toolchain.toml`; C3 stays the friction-free target |
| TX to a real motor de-syncs a production shade | Golden captures pin the encoder **before** first TX (§12); rolling codes persist from frame one (§7.1) |
| Rolling-code region wears out | Append-only wear-levelled region; ~2 writes per command against 100k-cycle endurance |

## 12. Sequencing

1. **Capture session.** Flash the spare S3 with the C++ firmware, enable
   `printBuffer`, record raw pulse dumps while the production device transmits.
   Receive-only — nothing on air, no risk to the estate. Clears the three
   `#[ignore]`d `somfy-rts` golden tests.
2. **`encode80` repeat parameter** in `somfy-rts`, pinned by the new captures.
3. **Firmware skeleton** — crate, chip features, CI matrix, `defmt`.
4. **TX path** — merge, pack, RMT transmit; host tests first.
5. **RX path** — `PulseSource`, RMT implementation, decode into `RxDecoder`.
6. **Tasks** — radio + state, bounded channels, rolling-code store.
7. **On-air bring-up** — listen first, then transmit, then position tracking.

Note the ordering property: **every step that could put a wrong frame on air is
preceded by a step that pins the frame against real captured hardware output.**

## 13. Open questions

1. Does the C++ `printBuffer` dump carry enough resolution and framing to serve
   as a golden fixture directly, or does the capture rig need a purpose-built
   sketch? Settle during step 1 of §12 — it does not block anything before then.
2. Which motor is the first TX target? A dedicated re-pairable test motor is
   preferable to a production shade for the first frame, even with the invariant
   honoured.
