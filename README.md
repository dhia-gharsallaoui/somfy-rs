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

Golden-capture validation **landed 2026-08-15**: the `somfy-rts` RX path is
pinned against pulses captured from a physical Somfy wall remote, and all three
fixture-backed tests pass **unmodified** — the engine decoded genuine hardware on
the first attempt. The capture independently confirmed the sync model (first
frames `hwsync == 4`, repeats `hwsync == 14`). `somfy-migrate` is likewise
validated against a real v25 device backup, though that test stays `#[ignore]`d
because the backup itself is private (see
[`crates/somfy-migrate/tests/fixtures/README.md`](crates/somfy-migrate/tests/fixtures/README.md)).

> **Before this repository is made public**, the committed pulse fixtures must be
> re-captured with a throwaway address or removed — they encode a real remote's
> radio address and rolling codes. See
> [`crates/somfy-rts/tests/fixtures/README.md`](crates/somfy-rts/tests/fixtures/README.md).

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
- **Plan 4 (firmware TX)** — **discharged 2026-08-15.** `encode80` now takes a
  `repeat` parameter and re-encodes byte 7 per repeat exactly like the C++
  reference (`196 + 4*repeat`, cycling by -15 past 255; Favorite/Stop flip
  `196→132` on any repeat > 0), for both extended and base commands — see
  `encode80_byte7`/`encode80_tail` in `frame.rs`. A transmitter MUST call
  `encode80` once per frame sent with the matching repeat index.
- **Plans 5 & 7 (network + UI)** consume the `somfy-api` DTOs as the single wire
  contract. The `ts-rs`-generated TypeScript in `ui/src/api/generated/` is the
  UI's source of truth; regenerate it (build with `--features ts`) whenever a DTO
  changes so UI/firmware drift stays a compile error rather than a runtime bug.
- **Plan 6 (persistence)** owns applying `somfy-migrate` output. Four
  obligations:
  (1) persist `MigrationData` (shades, rooms, groups) into the new config store,
  surfacing v19–v22 groups and linked remotes whose rolling codes could not be
  recovered from the backup so the user re-pairs or sets them manually;
  (2) **import MQTT settings**, which `somfy-migrate` deliberately defers — the
  C++ settings record (`ConfigFile.cpp` `writeSettingsRecord`, :1019) parses
  cleanly, but Plan 3 has nowhere to store it until Plan 6 persistence exists.
  This is a recorded deviation from design spec §3.4, not a dropped requirement;
  (3) **default unknown shade kinds to `Roller` and warn the user.**
  `ShadeKind::from_raw`/`TiltMode::from_raw` return `None` for a valid C++ kind
  outside the v1.0 subset (garage/gate/drycontact) or an invalid byte; Plan 6
  imports such a shade with `kind` defaulted to `ShadeKind::Roller` and surfaces a
  warning rather than dropping the shade or guessing a behavior; and
  (4) **warn when `MigrationData::skipped_resyncs` is nonzero.** A nonzero count
  means one or more records did not align exactly (e.g. an unescaped comma in a
  name shifted every field, which can yield a *plausible but wrong* rolling code),
  so Plan 6 must show the user the imported values for confirmation instead of
  silently applying them.
- **Group commands stay per-shade fan-out in v1.0.** The domain executes a group
  command by fanning it out to each member shade (Plan 2 `Controller::command_group`),
  not by transmitting a single group virtual-remote frame. Even so, group
  virtual-remote identities (`address` + `next_code` from `MigratedGroup`) MUST
  still be persisted by Plan 6 for future group-TX support and to preserve the
  option of pairing-compatible group frames. The v19–v22 fabricated-code warning
  applies only if/when group-TX is implemented.

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
