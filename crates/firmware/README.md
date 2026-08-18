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

Exactly one `chip-*` feature must be selected per build; esp-hal's own
per-chip features are mutually exclusive, so "supports two chips" means two
separate builds, never one combined binary.

| Feature | Chip | Target triple | Ships with |
|---|---|---|---|
| `chip-s3` | ESP32-S3 | `xtensa-esp32s3-none-elf` | everything: `mqtt`, `ui`, `mdns`, `sntp` |
| `chip-c3` | ESP32-C3 | `riscv32imc-unknown-none-elf` | `mqtt`, `ui`; `mdns` and `sntp` refused |

Two instruction sets on purpose: the ESP32-S3 is Xtensa, the ESP32-C3 is
RISC-V. The pair has already caught a fault an S3-only matrix would have
shipped — `AtomicU32::fetch_add` does not exist on `riscv32imc`.

**The ESP32-C3 is reached by IP address, not by name**, and has no wall clock.
It has the DRAM for the web server but not for the mDNS responder or the SNTP
client on top of it, so `src/heap.rs` refuses both with a `compile_error!`
naming the measurement: with them on, that chip's Wi-Fi heap is 52,224 bytes
against a 54,620-byte announcement peak; without them it is 60,416. Those
refusals are not advice. They are what keeps `heap::DRAM_FOR_STACK_AND_HEAP` a
per-chip *maximum* — it is measured at the largest feature set each chip can
build, and a build larger than its own measurement takes DRAM the constant has
already promised to the stack.

### Two chips have been dropped

Both for the same reason, and both recorded with their arithmetic in
`docs/provenance.md`: **neither had ever booted this firmware.**

- **ESP32-S2, 2026-08-17.** It could not hold the Wi-Fi heap and a bootable
  stack at the same time.
- **ESP32, 2026-08-18.** It was already excluded from the web server by a
  `compile_error!`, and its one buildable configuration — `mqtt` alone — left
  its Wi-Fi heap 1,700 bytes above the measured announcement peak, inside that
  peak's own ~2,000-byte boot-to-boot spread, with nothing smaller to retreat
  to. That is not a fit; it is a claim the project could not back.

Removing them removed unverified claims rather than capability.

Only the ESP32-S3 pin map in `src/chip.rs` has been checked against a real
working device (see `docs/provenance.md` for details and the "Hardware-verified
values" table). The ESP32-C3 pin map is an unverified default — confirm it
against real hardware before wiring a board to those pins.

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
# The C3 has no DRAM for mDNS or SNTP on top of the web server — see below.
cargo build --no-default-features --features chip-c3,mqtt,ui \
  --target riscv32imc-unknown-none-elf
```

Each of those builds all four binaries. Add `--bin store-check`,
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
| the C3's shipping image | `--no-default-features --features chip-c3,mqtt,ui` |
| API with no browser front end | `--no-default-features --features chip-s3,http` |
| broker only | `--no-default-features --features chip-s3,mqtt` |
| radio only | `--no-default-features --features chip-s3` |

**`mdns` and `sntp` cannot be built for the ESP32-C3.** They cost 4,672 and
2,880 bytes of DRAM, and with both on that chip's Wi-Fi heap falls to 52,224
against a 54,620-byte announcement peak — a board that would associate, connect
to the broker, and then exhaust the heap part-way through publishing its
discovery configs. `src/heap.rs` refuses both with a `compile_error!` naming the
measurement. Without them its heap is 60,416, which is 5,796 above that peak.
The ESP32-S3 carries everything with 10,916 bytes to spare.

Note that the web UI is *not* what does not fit: it costs 240 bytes of DRAM,
because its assets are `include_bytes!` in flash. The connection tasks and
`picoserve`'s monomorphised router come with `http`, which the C3 keeps.

A bare `cargo build` (no chip feature) or a build with more than one chip
feature enabled is expected to fail — see `src/chip.rs`'s `compile_error!`
guards. In practice, esp-hal's own dependencies (`esp-println`'s
`assert_unique_used_features!` build-script check, and duplicate generated
macros when two `esp-hal` chip features are both active) already enforce
"exactly one chip feature" earlier in the dependency graph, so the build
fails before this crate's own guards get a chance to run; the outcome (build
fails) is the same either way, just with a less specific upstream error
message instead of the one in `chip.rs`.

## Editor setup, and why the crate looks broken without it

Open any file in this crate in an editor and rust-analyzer will most likely
report:

```
no chip selected: build with exactly one of --features chip-s3 | chip-c3
```

with the whole file greyed out as inactive. **Nothing is wrong.** rust-analyzer
checks with no features by default, `src/chip.rs` refuses a build that names no
chip, and the guard is doing its job — esp-hal's own chip features are mutually
exclusive, and without that `compile_error!` a zero-feature build fails roughly
twenty-four macro expansions deep in a dependency, naming none of the actual
problem.

`rust-analyzer.toml` in this directory fixes it by naming a chip. It picks the
**C3**, because `riscv32imc-unknown-none-elf` ships with stable Rust — the
ESP32-S3 is Xtensa and needs the `esp` toolchain from `espup`, so it would
require the editor to be launched from a shell that had already sourced
`~/export-esp.sh`. Verified: `cargo check --features chip-c3 --target
riscv32imc-unknown-none-elf` completes on a plain stable toolchain.

The cost is that the `chip-s3` arms of `chip.rs` and `heap.rs` show as inactive,
and anything Xtensa-specific is analysed against the wrong chip. Working on
those, change both keys and start the editor from an esp-sourced shell.

Per-directory `rust-analyzer.toml` is a recent feature and your editor may
ignore it. The equivalent settings, for Neovim with `nvim-lspconfig`:

```lua
require("lspconfig").rust_analyzer.setup({
  settings = {
    ["rust-analyzer"] = {
      cargo = {
        features = { "chip-c3" },
        target = "riscv32imc-unknown-none-elf",
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
