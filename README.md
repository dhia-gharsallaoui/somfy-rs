<div align="center">

# somfy-rs

**A ground-up Rust firmware for Somfy RTS shades on ESP32-class hardware.**

`no_std` · `esp-hal` + Embassy · allocation-free outside the Wi-Fi driver ·
Home Assistant over MQTT · a web UI served from flash

[![CI](https://github.com/dhia-gharsallaoui/somfy-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/dhia-gharsallaoui/somfy-rs/actions/workflows/ci.yml)
[![License: GPL-3.0-only](https://img.shields.io/badge/License-GPL--3.0--only-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-no__std-orange.svg)](https://www.rust-lang.org)

</div>

---

## Standing on ESPSomfy-RTS

This project exists because of **[ESPSomfy-RTS](https://github.com/xkain/ESPSomfy-RTS)**,
and it owes it more than a citation.

That firmware has been controlling real shades in real houses for years. Every
protocol timing here was **derived from it and then verified**, not guessed:
the 640 µs half-symbol, the wake-up and sync structure, the hardware-sync counts
that differ between first and repeat frames and again between 56- and 80-bit
frames, the rule that a long `Prog` burst *removes* a remote where a short one
adds it. Its architecture taught us things a datasheet could not — it receives
with a GPIO change-interrupt rather than the RMT peripheral, which sidesteps a
constraint that cost this project a long detour before we read it properly.

It is released into the **public domain (Unlicense)**, which is a generous thing
to do with years of work.

**This is a rewrite, not a fork.** Where the two differ, it is recorded
deliberately in [`docs/provenance.md`](docs/provenance.md) — including the
handful of places where reading the reference closely turned up defects we chose
not to reproduce. Those are noted there with the evidence, in the spirit of a
project that made its own work inspectable.

---

## Why a rewrite

Not because the original is bad — because a few things are easier to guarantee
than to retrofit:

| | |
|---|---|
| **Host-testable core** | The protocol, domain model, position engine and config format are `no_std` crates with no hardware in them. ~950 tests run on a laptop in seconds. |
| **One command path** | HTTP and MQTT reach the *same* functions. Feature-gating the transports proves it: the core compiles with both switched off. |
| **Measured, not assumed** | Stack and heap budgets are derived from linked ELFs and **checked again at every boot**, printed next to the claim. A board refuses to start rather than overflow. |
| **Provenance** | Every constant is traceable to a measurement, a reference line, or an honest "this is a policy figure". |

---

## Features

### Radio

- **Somfy RTS**, 56-bit and 80-bit frames, **per shade** — a shade imported from
  a controller that drove it at 80 bits is driven at 80 bits.
- Transmit and receive via the **RMT peripheral**; CC1101 in asynchronous-serial
  OOK at 433.42 MHz.
- **Rolling codes persisted before the frame goes out**, never after. Overwriting
  a stored code is not expressible in the API.
- **Overheard frames tracked** — press a wall remote and the position estimate
  follows, provided the remote is registered as linked.

### Shades

- 32 shades, 16 rooms, 16 groups; 7 linked remotes per shade.
- **Pairing with the controller's own virtual remote**, derived from
  device-unique MAC bytes, so two boards never collide.
- **Adding a shade and pairing it are one flow.** Nothing is announced to Home
  Assistant until you confirm the motor actually moved — RTS is one-way, so the
  device can never know on its own, and it does not pretend to.
- **Position dead-reckoning** with per-direction travel times, dead-time
  compensation, endpoint resynchronisation and reported confidence.
- **A dedicated vent command** for perforated shutters: drives to the closed
  limit, then up by the measured slat-separation time. Uses no position estimate
  at all, so it is immune to drift.
- **Guided calibration**, with hand-entered values always available — and a value
  equal to the reference firmware's factory defaults is reported as
  *uncalibrated* rather than presented as configured.

### Network

- Wi-Fi via `esp-radio` + `embassy-net`.
- **MQTT with Home Assistant discovery** — covers, per-shade pairing buttons and
  device diagnostics, with no YAML. Discovery and state namespaces are separate
  values that **cannot be concatenated**; the type system refuses it.
- **Web UI served from flash** (Preact + Vite, English and French, well inside a
  200 KB gzipped budget), with REST and a WebSocket event stream.
- **mDNS** — reachable at `http://somfy-<mac>.local`.
- **SNTP**, kept strictly out of everything that must stay monotonic.
- **Origin and Host validation**, and a **per-shade rate limit** on commands.
  Not authentication — the two mitigations that need no password, one stopping a
  page in somebody else's browser tab driving your shades, the other bounding
  flash wear.

### Migration

- Import a **backup from the C++ firmware**: shades, addresses, rolling codes,
  rooms, groups and broker settings.
- The old firmware's topic concatenation is **undone**, not carried across.
- Anything unrecoverable — a group whose rolling code predates the format that
  stored it — is **surfaced to you**, not silently defaulted.

### Operations

- **A/B partitions** with the rolling-code region pinned where it already is, so
  moving to the new layout is a rename rather than a migration.
- Per-chip heap and stack sizing, verified at boot.
- Feature-gated transports (`mqtt`, `http`, `ui`, `mdns`, `sntp`).

---

## Hardware

| Chip | Status |
|---|---|
| **ESP32-S3** | **Hardware-proven.** The reference platform; everything below was measured on one. |
| ESP32-C3 | Builds and is budgeted; never run. Heap margin is tight — the firmware warns at boot. |
| ESP32 | Builds without the web server, which its DRAM cannot hold (refused at compile time, with the reason). Never run. |

Plus a **CC1101** 433 MHz module. Pin maps are in `crates/firmware/src/chip.rs`;
only the S3 map is verified.

> **Note** — the ESP32-S2 was supported until it was measured: it has too little
> DRAM to hold the Wi-Fi driver's heap and a bootable stack at once.

---

## Quick start

```bash
# Toolchain (Xtensa needs espup; the C3 does not)
cargo install espup && espup install && source ~/export-esp.sh
cargo install espflash

# Build and flash
cd crates/firmware
cargo build --release --features chip-s3 --target xtensa-esp32s3-none-elf
espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/firmware
espflash monitor --port /dev/ttyUSB0
```

Then provision Wi-Fi and shades — [`docs/hardware-checklist.md`](docs/hardware-checklist.md)
has the procedures, including **how to identify the right board before flashing**,
which matters more than it sounds if you own two.

---

## Workspace

```
crates/
├── somfy-rts/      protocol: frames, rolling codes, pulse rendering, RX decode
├── somfy-domain/   shades, groups, rooms, the position engine, pairing
├── somfy-store/    rolling-code flash ring — overwrite is inexpressible
├── somfy-config/   persisted device, shade and estate records
├── somfy-api/      REST/WS/MQTT DTOs, with ts-rs TypeScript generation
├── somfy-mqtt/     topic construction and Home Assistant discovery
├── somfy-migrate/  C++ backup-file parser
├── somfy-cc1101/   radio driver      somfy-rmt/  RMT pulse I/O
├── somfy-tasks/    Embassy task bodies, transport-agnostic
└── firmware/       the only hardware-aware crate
ui/                 Preact + Vite, embedded in the image
```

Everything above `firmware` is `no_std` and host-testable. CI builds each of them
for `thumbv7em-none-eabihf` — a target with no allocator — so "allocation-free"
is checked rather than claimed.

---

## Development

```bash
cargo test --workspace          # ~950 tests, no hardware needed
cargo clippy --workspace --all-targets -- -D warnings
cd ui && bun run dev            # UI against a mock device, no firmware required
```

The UI's mock serves the **real API paths**, so the same client code runs against
mock and device with no "mock mode" branch — and the generated TypeScript is a CI
gate, so the two cannot drift.

`crates/firmware` is excluded from the workspace (it needs a different target and
`build-std`), so **workspace-wide commands skip it** — see its README, and note
that its `rust-analyzer.toml` is what stops the crate looking broken in an editor.

---

## Documentation

| | |
|---|---|
| [`docs/specs/`](docs/specs/) | Design specification and the requirement documents behind each subsystem |
| [`docs/plans/`](docs/plans/) | Implementation plans, in order |
| [`docs/provenance.md`](docs/provenance.md) | **Where every constant came from** — reference line, measurement, or an honest policy figure |
| [`docs/hardware-checklist.md`](docs/hardware-checklist.md) | Bring-up procedures, written from the mistakes |

---

## Status

Daily-driver on the author's own installation: shades paired, driven from Home
Assistant, and served from the device's own web UI.

**Still to come:** OTA (partitions are ready), a diagnostics screen,
backup/restore in the UI, and captive-portal onboarding.

---

## License

**GPL-3.0-only.** ESPSomfy-RTS is public domain (Unlicense); this rewrite is
independently licensed and shares no code with it.
