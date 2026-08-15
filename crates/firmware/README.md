# firmware

ESP32-family firmware skeleton for somfy-rs: a `no_std`/`no_main` binary that
proves the build-and-link path for the Somfy RTS transmitter on real hardware.
It currently does nothing but initialize the chip and print a startup banner;
the CC1101 SPI driver and RTS transmit loop land in a later task.

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
`[unstable] build-std = ["core"]` to compile `core` from source for each
build. This requires the `esp` toolchain's nightly-derived compiler
components (which `espup install` provides) — it will not work on a plain
`stable` toolchain.

## Why no dev-dependencies

`embedded-hal-mock` (a natural fit for host-side driver tests) is a std-only
crate — it does not declare `#![no_std]` and uses `std` macros unconditionally
— so it cannot be built for any of these bare-metal targets under
`build-std = ["core"]`, regardless of which features are selected. Host-side
unit tests for driver logic belong in a separate lib target built for the
host triple, once this crate has logic worth testing that way; there is
nothing to test yet in this skeleton.
