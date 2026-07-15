# somfy-rs

A ground-up Rust rewrite of [ESPSomfy-RTS](https://github.com/xkain/ESPSomfy-RTS),
firmware for controlling Somfy RTS shades from ESP32-class hardware. The goal is
a `no_std` `esp-hal` + Embassy firmware that is a daily-driver replacement for
the C++ original and a clean, host-testable, community-adoptable codebase. The
full design specification lives in [`docs/specs/`](docs/specs/).

## Status

**Plan 1 of 7 — protocol engine — complete.** The `somfy-rts` crate (frames,
rolling codes, TX pulse rendering, RX decoding, repeat-frame dedupe) is
implemented and green on the host. Golden-capture *validation* against real
device pulses is **pending**: the fixture loader is exercised every CI run by a
checked-in synthetic capture, but three fixture-backed tests are `#[ignore]`d
until real captures from a running C++ device land (see
[`crates/somfy-rts/tests/fixtures/README.md`](crates/somfy-rts/tests/fixtures/README.md)).

Later plans (per [`docs/specs/`](docs/specs/)):

| Plan | Scope |
|------|-------|
| 1 | Protocol engine (`somfy-rts`) — **this plan** |
| 2 | Domain model: shades/groups/rooms + position/tilt engine |
| 3 | API + migration DTOs (`somfy-api`, `somfy-migrate`) |
| 4 | Firmware radio: `esp-hal` RMT TX/RX, ESP targets |
| 5 | Network: WiFi, MQTT, Home Assistant discovery |
| 6 | Persistence + OTA (A/B partitions, rollback) |
| 7 | Web UI (Preact) served from flash |

## Workspace crates

| Crate | `no_std` | Description |
|-------|:--------:|-------------|
| [`somfy-rts`](crates/somfy-rts) | yes | Somfy RTS protocol engine: 56/80-bit frame encode/decode, rolling codes, OOK pulse rendering (TX) and dual-stream pulse decoding (RX), repeat-frame dedupe. Hardware-free — pure pulse data in/out. |

Additional crates (`somfy-domain`, `somfy-api`, `somfy-migrate`, `firmware`) and
the `ui/` app arrive in later plans.

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
