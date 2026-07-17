# somfy-rs

A ground-up Rust rewrite of [ESPSomfy-RTS](https://github.com/xkain/ESPSomfy-RTS),
firmware for controlling Somfy RTS shades from ESP32-class hardware. The goal is
a `no_std` `esp-hal` + Embassy firmware that is a daily-driver replacement for
the C++ original and a clean, host-testable, community-adoptable codebase. The
full design specification lives in [`docs/specs/`](docs/specs/).

## Status

**Plans 1–3 of 7 complete.** The `somfy-rts` protocol engine (frames, rolling
codes, TX pulse rendering, RX decoding, repeat-frame dedupe), the `somfy-domain`
model (shade/group/room registries, travel-time position dead-reckoning, command
orchestration, overheard-remote tracking), and the Plan 3 contract layer —
`somfy-api` (serde REST/WS DTOs with `ts-rs` TypeScript generation) and
`somfy-migrate` (C++ backup-file parser) — are implemented and green on the host.
**Next: Plan 4** — firmware radio (`esp-hal` RMT TX/RX on ESP targets).

Golden-capture *validation* against real device pulses is still **pending**: the
fixture loader is exercised every CI run by a checked-in synthetic capture, but
three fixture-backed tests are `#[ignore]`d until real captures from a running
C++ device land (see
[`crates/somfy-rts/tests/fixtures/README.md`](crates/somfy-rts/tests/fixtures/README.md)).

Plans (per [`docs/specs/`](docs/specs/)):

| Plan | Scope |
|------|-------|
| 1 | Protocol engine (`somfy-rts`) — **complete** |
| 2 | Domain model: shades/groups/rooms + position/tilt engine — **complete** |
| 3 | API + migration DTOs (`somfy-api`, `somfy-migrate`) — **complete** |
| 4 | Firmware radio: `esp-hal` RMT TX/RX, ESP targets — **next** |
| 5 | Network: WiFi, MQTT, Home Assistant discovery |
| 6 | Persistence + OTA (A/B partitions, rollback) |
| 7 | Web UI (Preact) served from flash |

## Contracts for later plans

Boundaries deliberately left to downstream plans so the protocol engine stays
policy-free:

- **Plan 2 (domain layer)** owns the extended→56-bit *downgrade* policy. The
  `somfy-rts` `encode56` rejects extended commands outright
  (`FrameError::ExtendedCommand`); mapping `Stop → My` for a 56-bit motor (per
  Somfy.cpp:2944) is a product decision the domain layer makes explicitly. Plan
  2 also owns the C++ address / rolling-code plausibility guards
  (Somfy.cpp:169-170), which `somfy-rts` does not enforce.
- **Plan 4 (firmware TX)** requires `encode80` to grow a `repeat` parameter
  before transmitting extended *or* base commands as 80-bit on hardware: the C++
  reference re-encodes byte 7 per repeat (`196 + 4*repeat`, with Favorite/Stop
  flipping `196→132` on later repeats). Today's repeat-less `encode80` emits only
  the first-frame form (C++-exact for extended commands; a placeholder tail for
  base commands — see `frame.rs`).
- **Plans 5 & 7 (network + UI)** consume the `somfy-api` DTOs as the single wire
  contract. The `ts-rs`-generated TypeScript in `ui/src/api/generated/` is the
  UI's source of truth; regenerate it (build with `--features ts`) whenever a DTO
  changes so UI/firmware drift stays a compile error rather than a runtime bug.
- **Plan 6 (persistence)** owns applying `somfy-migrate` output. Two obligations:
  (1) persist `MigrationData` (shades, rooms, groups) into the new config store,
  surfacing v19–v22 groups and linked remotes whose rolling codes could not be
  recovered from the backup so the user re-pairs or sets them manually; and
  (2) **import MQTT settings**, which `somfy-migrate` deliberately defers — the
  C++ settings record (`ConfigFile.cpp` `writeSettingsRecord`, :1019) parses
  cleanly, but Plan 3 has nowhere to store it until Plan 6 persistence exists.
  This is a recorded deviation from design spec §3.4, not a dropped requirement.

## Workspace crates

| Crate | `no_std` | Description |
|-------|:--------:|-------------|
| [`somfy-rts`](crates/somfy-rts) | yes | Somfy RTS protocol engine: 56/80-bit frame encode/decode, rolling codes, OOK pulse rendering (TX) and dual-stream pulse decoding (RX), repeat-frame dedupe. Hardware-free — pure pulse data in/out. |
| [`somfy-domain`](crates/somfy-domain) | yes | Domain model: shade/group/room registries + position dead-reckoning. Travel-time position/tilt estimator, command orchestration (commands in → planned radio TX + state-delta events out), and overheard-remote tracking. Clock-free — callers inject `now_ms`. |
| [`somfy-api`](crates/somfy-api) | yes¹ | Shared REST/WebSocket contract: serde DTOs mirroring the domain entities (camelCase wire, whole-percent `u8` positions, C++ numeric discriminants). The `ts` feature generates TypeScript types into `ui/src/api/generated/` so UI/firmware drift is a compile error. |
| [`somfy-migrate`](crates/somfy-migrate) | yes | C++ ESPSomfy-RTS backup-file parser: reads an exported `.backup` into `MigrationData` (shades, rooms, groups) so an existing setup migrates without re-pairing. Applies the rolling-code `+1` (last-sent → next-to-send) contract; allocation-free. |

¹ `somfy-api` is `no_std` by default; the `std`/`ts` features are host-only for
TypeScript generation.

The remaining `firmware` crate and the `ui/` app arrive in later plans.

## Build & test

Everything in this plan runs on the host — no hardware required:

```sh
cargo test --workspace          # full suite (unit, property, loopback, golden)
cargo fmt --check               # formatting
cargo clippy --workspace --all-targets -- -D warnings
```

`no_std` compilation is guarded against a bare-metal target:

```sh
rustup target add thumbv7em-none-eabihf
cargo build -p somfy-rts --target thumbv7em-none-eabihf
```

(ESP-specific targets arrive with the firmware radio in Plan 4;
`thumbv7em-none-eabihf` is a cheap universal `no_std` guard until then.)

CI runs all of the above on every push and pull request
([`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

## License

GPL-3.0-only (declared in the workspace manifest). The ESPSomfy-RTS reference
implementation is released into the public domain (Unlicense); this rewrite is
independently licensed.
