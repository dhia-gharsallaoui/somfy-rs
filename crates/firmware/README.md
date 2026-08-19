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

## The partition table, and why `build.rs` checks it

The table is A/B: two equal app slots `ota_0` and `ota_1`, plus the `otadata`
region the bootloader uses to choose between them. `partitions.csv` carries the
derivation of every offset in it — the short version is that `rollcode` is
treated as immovable, which fixes the first slot at 0x10000–0x200000 and leaves
the rest with no free choices. **The three data regions keep the offsets they
have always had, so a board already in service keeps its rolling codes, its
credentials and its shade table across the change.** There is no migration; see
`docs/hardware-checklist.md` for what an operator actually does.

`build.rs` parses that file with `esp-idf-part` — the same crate espflash uses —
and fails the build if the layout's invariants are broken. It exists because the
table is data: an edit that moves `rollcode` compiles, links, lints and passes
the whole two-chip matrix, and the first thing to notice would be a device
that had already lost the codes.

This crate is its own Cargo workspace — stated by an empty `[workspace]` table
in its `Cargo.toml`, not merely implied by the root manifest's
`exclude = ["crates/firmware"]`, because `exclude` only says "not a member of
*that* workspace" and cargo then keeps looking upwards. So building or testing
the rest of the repository never requires an ESP toolchain, and this crate
builds the same way wherever the checkout happens to sit. Do not add
`crates/firmware` back into the root `[workspace] members` list.

## Supported chips

One chip, and `chip-s3` still has to be named on a build: esp-hal has no
default chip, and "which chip" is a question this crate asks out loud rather
than answering by accident.

| Feature | Chip | Target triple | Ships with |
|---|---|---|---|
| `chip-s3` | ESP32-S3 | `xtensa-esp32s3-none-elf` | everything: `mqtt`, `ui`, `mdns`, `sntp` |

### Three chips have been dropped

All for the same reason, all recorded with their arithmetic in
`docs/provenance.md`: **none of them had ever booted this firmware.**

- **ESP32-S2, 2026-08-17.** It could not hold the Wi-Fi heap and a bootable
  stack at the same time.
- **ESP32, 2026-08-18.** It was already excluded from the web server by a
  `compile_error!`, and its one buildable configuration — `mqtt` alone — left
  its Wi-Fi heap 1,700 bytes above the measured announcement peak, inside that
  peak's own boot-to-boot spread, with nothing smaller to retreat to.
- **ESP32-C3, 2026-08-19.** The same judgement one step further. It had needed
  three accommodations to stay — two `compile_error!`s refusing it `mdns` and
  `sntp`, and halved TCP buffers — and its shipping build cleared the same peak
  by **676 bytes**, against a peak measured on an ESP32-S3, because no C3 had
  ever run this firmware to be measured on.

Removing them removed unverified claims rather than capability.

**What the C3 cost to remove, said plainly.** It was the only RISC-V row, and
that row caught a fault an Xtensa-only matrix would have shipped:
`AtomicU32::fetch_add` does not exist on `riscv32imc`. Five places in this crate
use a `blocking_mutex` where an atomic would be natural; they are kept in that
shape deliberately, and `docs/provenance.md` says so rather than leaving them
with a reason that has expired. It also cost the friction-free editor setup —
see "Editor setup" below.

The ESP32-S3 pin map in `src/chip.rs` is checked against a real working device
(see `docs/provenance.md` for details and the "Hardware-verified values"
table).

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

The web UI is embedded in the image with `include_bytes!`, so **build it
first** — `ui/dist/` is a build artefact and is not tracked:

```bash
cd ui && bun install && bun run build
```

Without it, `build.rs` stops with "there is no ../../ui/dist/ to embed" rather
than producing an image whose UI is a 404.

Then from `crates/firmware/`, with `~/export-esp.sh` already sourced:

```bash
cargo build --features chip-s3 --target xtensa-esp32s3-none-elf
```

That builds all four binaries. Add `--bin store-check`,
`--bin config-check` or `--bin tx-check` to build only one harness.

### Transport features

Three, all on by default: **`mqtt`** (the broker session), **`http`** (the web
server and `/api/v1/`), and **`ui`** (the embedded single-page app, which
implies `http`). Turning them off is not primarily a size knob — it is a
structural test that HTTP and MQTT reach the *same* functions rather than
reimplementing each other. A build with both off must still compile, and CI
builds exactly that per chip; `Cargo.toml` carries the argument.

| build | command |
|---|---|
| everything (default) | `--features chip-s3` |
| API with no browser front end | `--no-default-features --features chip-s3,http` |
| the responder alone (pulls `http`) | `--no-default-features --features chip-s3,mdns` |
| the clock alone (pulls nothing) | `--no-default-features --features chip-s3,sntp` |
| broker only | `--no-default-features --features chip-s3,mqtt` |
| radio only | `--no-default-features --features chip-s3` |

`mdns` costs 4,672 bytes of DRAM and `sntp` 2,880, additive to the byte; the
ESP32-S3 carries both with room to spare, and `src/heap.rs` prints the margin at
boot rather than asserting it. Note that the web UI is *not* the expensive part:
it costs 240 bytes, because its assets are `include_bytes!` in flash. The
connection tasks and `picoserve`'s monomorphised router come with `http`.

A bare `cargo build` (no chip feature) is expected to fail — see `src/chip.rs`'s
`compile_error!` guard. In practice esp-println's `assert_unique_used_features!`
build-script check already enforces it earlier in the dependency graph, so the
build fails before this crate's own guard gets a chance to run; the outcome
(build fails) is the same either way, just with a less specific upstream error
message instead of the one in `chip.rs`.

## Editor setup, and why the crate looks broken without it

Open any file in this crate in an editor and rust-analyzer will most likely
report:

```
no chip selected: build with --features chip-s3
```

with the whole file greyed out as inactive. **Nothing is wrong.** rust-analyzer
checks with no features by default, `src/chip.rs` refuses a build that names no
chip, and the guard is doing its job — esp-hal has no default chip, and without
that `compile_error!` a zero-feature build fails inside a dependency naming none
of the actual problem.

`rust-analyzer.toml` in this directory fixes it by naming the chip.

**You must start your editor from a shell that has run `source ~/export-esp.sh`.**
That is new, and it is a real regression in convenience rather than an
oversight. This file used to name the **ESP32-C3**, whose target
`riscv32imc-unknown-none-elf` ships with stable Rust, so analysis worked out of
the box in any shell. That chip was dropped on 2026-08-19 — it had never booted
this firmware and its Wi-Fi heap cleared the measured announcement peak by 676
bytes, against a peak measured on a different chip; `src/heap.rs` carries the
argument. The ESP32-S3 is Xtensa, `build-std` needs `rust-src` from the `esp`
toolchain, and without the environment the editor reports
`error[E0463]: can't find crate for 'core'`. There is no setting here that
avoids that.

Per-directory `rust-analyzer.toml` is a recent feature and your editor may
ignore it. The equivalent settings, for Neovim with `nvim-lspconfig`:

```lua
require("lspconfig").rust_analyzer.setup({
  settings = {
    ["rust-analyzer"] = {
      cargo = {
        features = { "chip-s3" },
        target = "xtensa-esp32s3-none-elf",
      },
    },
  },
})
```

That applies to every Rust project you open, which is wrong for all of them
except this one — so prefer a per-project override if your setup has one
(`.nvim.lua` with `exrc` enabled, `neoconf.nvim`, or a directory-local
`lspconfig` setup keyed on the root).

**One thing this does not fix**: the root workspace *excludes* `crates/firmware`
(see below), so an editor opened at the repository root may treat this crate as
a separate project or not analyse it at all. If that happens, either open this
directory directly, or add it to `rust-analyzer.linkedProjects`.

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
