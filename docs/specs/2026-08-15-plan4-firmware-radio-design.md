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

**Measured** by sweeping every payload byte value through
`somfy_rmt::build_symbols` on the host (`somfy-rmt/tests/build_symbols.rs`).
The worst case in every row is an **all-zeros** payload: each `0` bit renders
HIGH-then-LOW, so no bit's tail meets the next bit's head at the same level and
nothing merges. An all-ones payload is one merge short of it — its first `1` bit
opens LOW and absorbs the software sync's trailing LOW half.

| Frame | Merged pulses | Packed symbols | + end marker |
|---|---:|---:|---:|
| 56-bit, first (wake-up + 2 hw syncs + gap) | 120 | 60 | 61 |
| 56-bit, repeat (7 hw syncs + gap) | 128 | 64 | 65 |
| 80-bit, repeat (6 hw syncs, gap suppressed) | 174 | 87 | 88 |
| 80-bit, first (wake-up + 12 hw syncs, gap suppressed) | **188** | **94** | **95** |

The end-marker column is the one that matters. A zero-length entry is how the
peripheral is told to stop; an odd pulse count leaves one for free in the last
symbol's unused second half, but an **even** count fills every half and needs a
whole extra symbol appended. Every row here is even, so each pays for one.

An RMT memory block holds 48 symbols on the S3 and C3 and 64 on the ESP32 and
S2. Two blocks is therefore 96 symbols at worst, and the largest frame is **95**
— it fits on every chip, with exactly **one symbol spare**. `MEMSIZE_BLOCKS = 2`
in `firmware/src/radio/rmt_tx.rs`, next to a `const` assertion of
`MAX_SYMBOLS <= MEMSIZE_BLOCKS * esp_hal::rmt::CHANNEL_RAM_SIZE`. That assertion
reads the block size from `esp-hal`'s own per-chip constant rather than from this
document, and is checked on all four chip builds; one block fails it on all four.

The fallback this section previously reserved is **not needed and not
implemented**: `esp-hal`'s blocking `transmit` already streams data longer than
the channel's RAM, refilling from the remainder on a threshold interrupt inside
`TxTransaction::poll`, and the crate documents wrapping TX as supported on every
device. (`transmit_continuously` is the one that genuinely caps at RMT RAM; this
path does not use it.) Fitting entirely in RAM is still preferred, and asserted
above, so that a real-time OOK stream never depends on that refill keeping up.

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
transaction rather than being concatenated or truncated.

This section originally stated that bound as

```
WAKEUP_LOW (7,357 µs)  <  idle_threshold  <  INTER_FRAME_GAP (27,434 µs)   [WRONG]
```

and chose **12,000 µs**. Both were **corrected during implementation** and are
recorded here so the reasoning is not repeated.

**The longest in-frame LOW is not `WAKEUP_LOW`.** That constant describes the
pulse train *this project transmits*; a receiver hears whatever a physical
remote transmits, and every committed wall-remote capture shows a post-wake-up
gap near **17.7 ms** — `up_56bit_1.pulses` 17738, `down_56bit_1.pulses` 17722,
`my_56bit_1.pulses` 17711. A 12,000 µs threshold sits below that and would end
the reception one pulse into every real first frame. The real window is

```
17,738 µs (measured, somfy_rts::MEASURED_MAX_INTRA_FRAME_SEGMENT_US)
    <  idle_threshold  <  27,434 µs (INTER_FRAME_GAP)
```

Chosen: **22,000 µs** (`somfy_rmt::IDLE_THRESHOLD_US`), roughly mid-band, with
~4.3 ms of margin below and ~5.4 ms above. Both margins are asserted at compile
time against the measured constant, and `somfy-rmt/tests/idle_threshold.rs`
replays the captures through a host model of the hardware's split rule — pinning
both that 22,000 leaves each capture whole and that 12,000 would have cut it.

Note the two bounds are not equally trustworthy. The floor is measured from real
hardware. The ceiling is *our* transmitter's gap: no committed fixture contains a
real remote's repeat frame, so nothing establishes what a remote's inter-frame
gap actually is. Capturing one belongs to on-air bring-up.

Note the floor constant is **level-agnostic** — the longest segment of either
level, not the longest silence. The peripheral ends a reception when no *edge*
arrives for long enough, so a sufficiently long HIGH ends one exactly as a long
LOW does. The maximum is a LOW today (the longest HIGH is the ~10.2 ms wake-up
pulse), but a LOW-only bound would leave the wake-up pulse outside the guard.

**The field is also not always 16 bits.** It is 16 on the ESP32 and ESP32-S2 but
**15 on the ESP32-S3 and ESP32-C3**, so the ceiling is 32,767 µs at 1 µs
resolution, not 65,535. Any value in the window above is comfortably
representable either way, and the firmware asserts the bound against `esp-hal`'s
own per-chip `MAX_RX_IDLE_THRESHOLD` on all four builds rather than against a
figure written down here.

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

Both halves of that argument are now real. `somfy-store` makes a ticket
unforgeable; `somfy-tasks`' queue module is where "the producer end is not
exposed" stops being an obligation on implementations and becomes a fact: the
`embassy_sync::Channel` is private to that crate, the only handle it hands out
is usable solely through `TransmitQueue`, and `crates/firmware` — where the
tasks are wired together — is on the other side of a crate boundary from the
private field.

### 7.2 Corrected during implementation

Five statements this document or the plan implied that turned out to be false
against the toolchain and the hardware, recorded here so the reasoning is not
repeated.

- **`esp-hal-embassy` cannot be used.** Its `executors` feature (on by default)
  enables `esp-hal/__esp_hal_embassy`, a feature esp-hal **removed in
  1.0.0-rc.1**, so no published version of it resolves against esp-hal 1.1.
  `esp-rtos` is the replacement and supplies both the time driver and the
  thread-mode executor. `embassy-executor` moves to 0.10 with it.

- **Embassy tasks have no stacks.** Plan 4a's finding that `transmit_frame`
  needs ~6.5 KB and that "a default Embassy task is smaller" conflated two
  things. A task is a state machine polled on the executor's stack; what it
  gets statically is space for its *future*, and only what is live across an
  await goes there. The 6.5 KB is in `build_symbols`'s two 320-pulse buffers,
  which are locals of a synchronous call and therefore on the **main** stack.
  The arena that made the "default task size" claim true was removed in
  `embassy-executor` 0.9; 0.10 sizes each task's future exactly, so being wrong
  about a future is a link error. The main stack is whatever DRAM esp-hal's
  linker script has left over — measured at **304,652 bytes** on the ESP32-S3
  on 2026-08-16 — and `main::check_stack_headroom` reports it at boot and
  refuses to start below 8 KB.

- **The RMT driver mode is per *peripheral*, not per channel.** `Rmt::into_async`
  converts every channel creator at once and there is no way back for one of
  them, so §6.1's requirement that the receiver be asynchronous forces the
  transmitter to be asynchronous too. Plan 4a's blocking `RmtTx` was converted.
  A frame's *internal* timing is unaffected — a whole frame fits in reserved RMT
  RAM and is clocked from there — but the gap *between* the frames of a burst
  can now stretch if another task runs. **This change has not been re-verified
  on air.**

- **Not every RMT channel can receive, and the numbers differ per chip.** The
  ESP32 and ESP32-S2 allow either direction on any channel; the ESP32-S3 splits
  them 0-3 transmit / 4-7 receive and the ESP32-C3 splits them 0-1 / 2-3. With
  `memsize = 2` a channel also owns its neighbour's memory block. `chip.rs`'s
  `rmt_channels!` carries both facts per chip.

- **Nothing put the CC1101 into receive.** `somfy-cc1101` had `set_tx` and
  `set_idle` only, so the receive path as delivered could not have heard
  anything: in asynchronous serial mode the chip drives GDO2 only while it is
  receiving, and an unstrobed radio is indistinguishable from a quiet band.
  `set_rx` (`SRX`, 0x34) was added, with the register set already correct for it
  (`IOCFG2 = 0x0D`, `MCSM0` autocalibrating on IDLE→RX).

Two further consequences worth stating rather than discovering:

- **`encode80` takes a repeat index** and re-encodes the frame tail per repeat,
  so a burst must encode once per frame rather than once per burst. A 56-bit
  frame encodes identically every time, which is why 4a never met this.
- **A failed commit still moves the position estimate.** The domain updates a
  shade's motion model when it handles the command; the frames it plans are
  dispatched afterwards. A store failure therefore transmits nothing while the
  estimate believes the shade is moving. Not fixable without either
  pre-flighting the store (which cannot promise the commit) or telling the
  domain about transmission outcomes, which crosses the Plan 2 boundary. The
  recovery is the same one that covers a motor that simply did not hear: the
  next overheard frame or command re-anchors the estimate.

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
| ~~80-bit frame barely fits two RMT memory blocks~~ **Closed.** Measured at 95 symbols including the end marker, against 96 on the smallest-block chips (§5.2) | One spare symbol, held by a `const` assertion against `esp-hal`'s own per-chip block size and checked on all four chip builds. The wrap-around fallback proved unnecessary: `esp-hal`'s blocking `transmit` already refills the channel from the remaining data |
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

1. ~~Does the C++ `printBuffer` dump serve as a golden fixture directly?~~
   **Resolved 2026-08-15.** `printBuffer` is dead code in v2.5.6. The working
   route is the `remoteFrame` websocket event (room `join:0` on port 8080),
   whose `pulses[]` is a verbatim `rx.pulses[]` dump. Step 1 of §12 is complete:
   all three golden tests pass against a real wall-remote capture, unmodified.
2. ~~Which motor is the first TX target?~~ **Resolved 2026-08-15.** The office
   roller shade, driven successfully from the spare S3 (Up fully opened it, Down
   closed it) at rolling code 55. The motor accepted the first frame, confirming
   no rolling-code desync.
3. **Pre-public obligation (blocking on open-sourcing, not on Plan 4).** The
   committed pulse fixtures encode a real remote's address and rolling codes.
   Re-capture with a throwaway address — which needs two working radios — or
   remove them before the repository goes public. Tracked in
   `crates/somfy-rts/tests/fixtures/README.md`.
4. Does the `PulseSource` trait need a third implementation for bring-up
   (a file/replay source) so the radio task can be exercised on the host? Likely
   yes, and it is nearly free given the trait already exists.
