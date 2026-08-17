# firmware

ESP32-family firmware for somfy-rs: `no_std`/`no_main` binaries that exercise
the parts of the controller that can only exist against real hardware.

| Binary | What it does | Puts RF on the air |
|---|---|---|
| `firmware` | The controller: radio task, state task, the three flash regions, and — when credentials are provisioned — Wi-Fi and the TCP/IP stack | only when a broker commands a provisioned shade |
| `tx-check` | Brings up the CC1101 and transmits one Somfy frame plus repeats, at a synthetic address | **yes** |
| `store-check` | Reads the rolling-code region, commits the next code, reads it back | no — flash only |
| `config-check` | Reads the Wi-Fi config region, writes a **placeholder** credential, reads it back | no — flash only, and no network |

They are separate binaries so that proving the rolling-code store survives a
power cycle never involves keying a transmitter, and so that flashing the
controller cannot put a frame on the band by itself. `firmware` transmits only
what the MQTT session commands, and it can only be commanded to move a shade
the `shades` region names — so a board whose shade table is erased receives,
decodes and logs, and keys the transmitter never.
`docs/hardware-checklist.md` has the procedure for each.

**The network is optional and cannot take the radio down.** A board with no
credentials provisioned — which is what a freshly flashed one is — boots
cleanly, says so, and runs the radio; so does one whose credentials are wrong,
retrying with bounded backoff. `src/net.rs` documents the four structural
reasons that holds.

The rolling-code store needs a `rollcode` partition, the config store a
`wificfg` one, and the shade table a `shades` one — which is why this crate
carries its own `partitions.csv` and
an `espflash.toml` pointing espflash at it. Run `espflash` from this directory so it finds them; a device flashed with
espflash's default table reports `PartitionMissing` and stops rather than
running without durable storage.

This crate is its own Cargo workspace (see the root `Cargo.toml`'s
`exclude = ["crates/firmware"]`), so building or testing the rest of the
repository never requires an ESP toolchain. Do not add `crates/firmware` back
into the root `[workspace] members` list.

## Supported chips

Exactly one `chip-*` feature must be selected per build; esp-hal's own
per-chip features are mutually exclusive, so "supports four chips" means four
separate builds, never one combined binary.

| Feature | Chip | Target triple |
|---|---|---|
| `chip-esp32` | ESP32 | `xtensa-esp32-none-elf` |
| `chip-s2` | ESP32-S2 | `xtensa-esp32s2-none-elf` |
| `chip-s3` | ESP32-S3 | `xtensa-esp32s3-none-elf` |
| `chip-c3` | ESP32-C3 | `riscv32imc-unknown-none-elf` |

Only the ESP32-S3 pin map in `src/chip.rs` has been checked against a real
working device (see `docs/provenance.md` for details and the "Hardware-verified
values" table). The ESP32, ESP32-S2, and ESP32-C3 pin maps are unverified
defaults — confirm them against real hardware before wiring a board to those
pins.

## Toolchain setup

This crate needs Espressif's `esp` Rust toolchain, which is distinct from the
`stable` toolchain the rest of the workspace uses. `crates/firmware/rust-toolchain.toml`
pins `channel = "esp"` so `rustup` and `cargo` pick it up automatically inside
this directory.

1. Install `espup` (the tool that installs and manages the `esp` toolchain).
   The usual route is `cargo install espup`, but **on Fedora this fails**:
   `espup` depends on `openssl-sys`, which needs a `perl` toolchain and the
   OpenSSL development headers to build from source, and a stock Fedora
   install is typically missing one or both. Rather than chasing down every
   build dependency, download the prebuilt `espup` binary from its
   [GitHub releases page](https://github.com/esp-rs/espup/releases) for your
   architecture (e.g. `espup-x86_64-unknown-linux-gnu`), mark it executable,
   and put it on your `PATH`. This sidesteps the local build entirely.

2. Install the toolchain itself:

   ```bash
   espup install
   ```

   This writes an environment setup script, by default at `~/export-esp.sh`.

3. Before any `cargo` command in this crate, source that script in the current
   shell:

   ```bash
   source ~/export-esp.sh
   ```

   This puts the `esp` toolchain, the Xtensa targets, and the associated
   linker (`xtensa-esp32*-elf-gcc`) on `PATH` for that shell session. It needs
   to be re-sourced in every new shell.

## Building

From `crates/firmware/`, with `~/export-esp.sh` already sourced:

```bash
cargo build --features chip-s3    --target xtensa-esp32s3-none-elf
cargo build --features chip-esp32 --target xtensa-esp32-none-elf
cargo build --features chip-s2    --target xtensa-esp32s2-none-elf
cargo build --features chip-c3    --target riscv32imc-unknown-none-elf
```

Each of those builds all four binaries. Add `--bin store-check`,
`--bin config-check` or `--bin tx-check` to build only one harness.

A bare `cargo build` (no chip feature) or a build with more than one chip
feature enabled is expected to fail — see `src/chip.rs`'s `compile_error!`
guards. In practice, esp-hal's own dependencies (`esp-println`'s
`assert_unique_used_features!` build-script check, and duplicate generated
macros when two `esp-hal` chip features are both active) already enforce
"exactly one chip feature" earlier in the dependency graph, so the build
fails before this crate's own guards get a chance to run; the outcome (build
fails) is the same either way, just with a less specific upstream error
message instead of the one in `chip.rs`.

## Notes on `build-std`

The `esp` toolchain does not ship a prebuilt `core` for these bare-metal
targets, so `crates/firmware/.cargo/config.toml` sets
`[unstable] build-std = ["core", "alloc"]` to compile them from source for
each build. `alloc` is there for `esp-radio`, which needs a heap; `src/heap.rs`
carries the size, the two measurements that bracket it, and the argument that
nothing on the frame path can be starved by it. This requires the `esp` toolchain's nightly-derived compiler
components (which `espup install` provides) — it will not work on a plain
`stable` toolchain.

## Why no dev-dependencies

`embedded-hal-mock` (a natural fit for host-side driver tests) is a std-only
crate — it does not declare `#![no_std]` and uses `std` macros unconditionally
— so it cannot be built for any of these bare-metal targets under
`build-std = ["core"]`, regardless of which features are selected. Host-side
unit tests for driver logic belong in a separate lib target built for the
host triple. In practice almost nothing has needed to be: the task bodies live
in `somfy-tasks`, the symbol pipeline in `somfy-rmt`, the CC1101 register set
in `somfy-cc1101` and the store's arithmetic in `somfy-store`, all of which the
host test suite covers. What is left here is the code that can only be checked
on a chip.
