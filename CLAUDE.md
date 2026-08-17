# somfy-rs — working rules

## Do not reinvent what a well-maintained crate already does

**Before writing any non-trivial component, search for an existing crate and
say what you found.** Prefer adopting, wrapping, or porting a proven
implementation over writing a new one.

"Well-maintained" is a claim to be checked, not assumed. Look at the actual
repository and report the numbers: **stars, forks, contributors, commit count,
date of the last commit, open issues and PRs, and whether anyone outside the
author uses it.** A single-author crate with four commits and zero stars is not
a dependency for firmware that runs unattended in someone's home, however
pleasant its API. Publication on crates.io and a tidy README are not adoption.

Then check it covers the case at hand — **against the real API, not the crate
description**.

Record the decision either way. "I looked, and here is why the existing crate
does not fit" is a required part of the work, not an optional extra — and it is
just as valuable as adopting one, because it stops the same question being
reopened later.

Reasons that justify writing our own, when they are actually true:

- **The crate does not cover the mode we need.** Check this against the real
  API rather than the crate description.
- **It is unmaintained or pre-release** in a way that matters for firmware we
  intend to run unattended in someone's home.
- **Its licence is incompatible** with this project's GPL-3.0-only, or absent.
- **The wrapping would be larger than the thing.** Rare, and suspicious when
  claimed — say concretely what the wrapper would have to do.

Reasons that do **not** justify it: it looks easy; we would learn more; the
crate's API is not quite the shape we would have chosen; we have already
started.

This applies to the reference implementation too. `docs/provenance.md` rule 1
already requires deriving from it rather than inventing protocol behaviour —
and note that the C++ reference itself gets its radio configuration from an
external library rather than hand-rolling it. Reuse is the norm on both sides
of this port.

### Recorded evaluations

| Component | Existing option | Decision |
|---|---|---|
| `somfy-rts` (protocol) | [`somfy`](https://crates.io/crates/somfy) 0.1.0 | **Own.** Frame construction only, transmit-oriented; no pulse rendering, no receive decode. 0 stars/forks, no releases, no visible licence file. Ours does 56/80-bit encode **and** decode, rolling codes, pulse rendering, dual-stream RX and repeat dedupe, and is pinned against real wall-remote captures. |
| WiFi (Plan 5) | [`esp-radio`](https://crates.io/crates/esp-radio) 0.18.0, [`esp-wifi`](https://crates.io/crates/esp-wifi) 0.15.1, [`esp-wifi-sys`](https://crates.io/crates/esp-wifi-sys) | **Adopt `esp-radio` 0.18.0.** Verified both ways against the real graph: `esp-radio` resolves and leaves `esp-hal` at 1.1.2, while **`esp-wifi 0.15` fails outright** — "failed to select a version for `esp-wifi` which could resolve this conflict" against our `esp-hal 1.1.2` / `esp-rtos 0.3`. `esp-radio` continues esp-wifi's version line (0.15 → 0.18) after the rename and lives in the esp-hal repo, so it tracks the HAL. `esp-wifi-sys` is **not an alternative** — it is bindgen FFI to Espressif's closed blobs, the layer *beneath* the driver, and we already consume it transitively. There is no independent pure-WiFi Rust driver for ESP32; the MAC/PHY is closed, so every option binds the same blob and the only choice is which layer to sit at. Requires a heap — `build-std` must gain `alloc`. |
| Network stack (Plan 5) | [`embassy-net`](https://crates.io/crates/embassy-net) 0.9.1 | **Adopt.** The standard stack for Embassy, which we already run via `esp-rtos`. |
| Allocator (Plan 5 Task 2) | [`esp-alloc`](https://crates.io/crates/esp-alloc) 0.10.0 | **Adopt, and there is effectively no choice.** `esp-radio`'s own `default` feature *is* `esp-alloc`, `esp-rtos` has a matching `esp-alloc` feature, and both allocate through its `InternalMemory` allocator specifically — a generic `linked_list_allocator` as `#[global_allocator]` would satisfy the `alloc` requirement but not the internal-memory capability the Wi-Fi driver's buffers need. Enabled its `internal-heap-stats` feature, which is what turns the heap size from an assertion into a measurement: it keeps a high-water mark, and `docs/provenance.md` records the boot the figure came from. |
| `'static` handoff for `embassy-net`'s `StackResources` (Plan 5 Task 2) | [`static_cell`](https://crates.io/crates/static_cell) 2.x | **Adopt.** `embassy_net::new` borrows `&'d mut StackResources` for the lifetime of the stack, so it needs a `'static` handed out exactly once. The alternative is a `static mut` plus an `unsafe` block asserting single use — the same guarantee with the check deleted. `StaticCell::init` panics on a second call instead. By embassy's own author, MIT/Apache-2.0, ~3 KB of code, no transitive dependencies. |
| Persisted device config (Plan 5 Task 2) | [`sequential-storage`](https://crates.io/crates/sequential-storage) 8.0.1, [`ekv`](https://crates.io/crates/ekv) 1.0.0, ESP-IDF NVS via [`esp-storage`](https://crates.io/crates/esp-storage) | **Own — `somfy-config`, and deliberately minimal, because Plan 6 replaces it. Revisit there.** `sequential-storage` is the serious candidate and is well adopted (675,620 downloads, 189,786 recent, MIT OR Apache-2.0, last release 2026-07-19); `ekv` is embassy-rs's LSM-tree key-value store for raw NOR flash but far less used (6,753 downloads, 664 recent). Both are **key-value stores, and a key-value store is not the part that was missing.** What this task needs is a *validated record*: an SSID over 32 bytes, a passphrase under 8 characters or a field containing a NUL must be **refused**, not stored, and a store would persist all three happily — the validation is where the whole value is, and it is what `somfy-config` actually consists of. What a store adds over the ring already written and hardware-proven in `somfy-store` is wear levelling across a region written a handful of times in a device's lifetime. `esp-storage` is not an alternative at all: it exposes raw NOR flash, not NVS, and there is no NVS reader in the Rust ESP stack — writing one to hold two strings would be format-compatibility work for something Plan 6 deletes. **What would change this:** Plan 6's config is a whole device model rather than two strings, and at that size `sequential-storage` under the same validation layer is the likely answer. **Plan 5 Task 3 moved that threshold closer**: the record grew to six fields and 512 bytes to carry the broker settings and the two MQTT namespaces, and it now also has to hand back the *superseded* namespace pairs still readable in the ring, which a key-value store would express far better than a hand-rolled scan. The validation argument still holds — the MQTT settings are refused, not stored, by the same rules `somfy-mqtt` builds topics with — but the storage half is now the larger part. |
| MQTT client (Plan 5 Task 3) | Surveyed the whole crates.io field, not a remembered shortlist (230 crates carry the `mqtt` keyword; every `no_std` client among them was checked). Clients: [`minimq`](https://crates.io/crates/minimq) 0.13.0, [`rust-mqtt`](https://crates.io/crates/rust-mqtt) 0.5.1, [`mcutie`](https://crates.io/crates/mcutie) 0.4.0, [`mountain-mqtt`](https://crates.io/crates/mountain-mqtt) 0.2.0, [`embedded-mqttc`](https://crates.io/crates/embedded-mqttc) 1.0.1, [`myrtio-mqtt`](https://crates.io/crates/myrtio-mqtt) 0.3.0, [`embassy-ha`](https://crates.io/crates/embassy-ha) 0.4.0, [`mqtt-async-embedded`](https://crates.io/crates/mqtt-async-embedded) 1.0.0, [`tinymqtt`](https://crates.io/crates/tinymqtt) 0.1.3, [`mqttrust`](https://crates.io/crates/mqttrust) 0.6.0, [`w5500-mqtt`](https://crates.io/crates/w5500-mqtt) 0.4.0, [`embassy-mqtt-lite`](https://crates.io/crates/embassy-mqtt-lite) 0.3.0 | **Adopt `minimq` 0.13.0. Runner-up `rust-mqtt` 0.5.1.** Both clear every hard requirement — retain, retained LWT on CONNECT, subscribe/unsubscribe, QoS 0–2, `embedded-io-async` 0.7 (the version `embassy-net` 0.9.1 itself uses, so `TcpSocket` is accepted with no adapter) — and **both were proved by building a throwaway spike for `xtensa-esp32s3-none-elf` under `build-std = ["core"]`**, exercising retained publish, retained will, zero-length retained removal, non-retained QoS 1 command, subscribe and unsubscribe. Neither needs `alloc`; `esp-hal` stays at 1.1.2. The decider is **allocation strategy and `unsafe`**. `minimq` has **0 occurrences of `unsafe`** in its source, takes caller-owned fixed `rx`/`tx` slices, and advertises the rx size as MQTT5 `MaximumPacketSize` in CONNECT so the broker will not send anything larger — inbound is bounded by construction. `rust-mqtt` alloc-free means the `bump` feature, which is a raw-pointer bump allocator that never frees: every inbound dynamically-sized field consumes it (`src/io/read.rs:147`), reclaiming needs `pub unsafe fn reset()`, and the caller must prove no borrows remain — its own 0.5.0 changelog records fixing UB in exactly that code, and its README's worked example uses the heap `AllocBuffer` instead. So `rust-mqtt` offers a choice between an `unsafe` soundness obligation on the hot path of a device that runs unattended for months, or a heap on the MQTT path that Plan 5 deliberately confines to `esp-radio`. `minimq` also drives keepalive/PINGREQ itself and exposes `ConnectEvent::Connected` vs `Reconnected`, which is precisely the R5 hook for "republish discovery only on a fresh broker session". **Maintenance, both:** `minimq` 191 stars, 19 forks, 656 commits, 13 contributors (11 non-maintainer), last commit 2026-07-14, 1 open issue / 0 open PRs, 13 merged non-maintainer PRs, 10 distinct issue authors, ~19 third-party repos, 141,594 downloads (3,473 recent), MIT. `rust-mqtt` 119 stars, 54 forks, 124 commits, 16 contributors (15 non-author), last commit 2026-08-13, 8 open issues / 2 open PRs, 55 merged non-owner PRs, 22 distinct issue authors, **~120 third-party repos and 13,223 recent downloads**, MIT OR Apache-2.0. **`rust-mqtt` is the more widely adopted crate, and by a wide margin on our exact stack** (ESP32 + Embassy + `no_std` demos from esp-rs community authors) — that is the strongest argument against this ruling and the reason it is the runner-up rather than an elimination. Countervailing: both current APIs are recent rewrites with little field time (`minimq` 0.11 "breaks most of the API surface" 2026-05-05, then 0.12 and 0.13 broke it again by 2026-07-14; `rust-mqtt` 0.5.0 was a rewrite 2026-03-27), and quartiq's own deployed firmware — `stabilizer`, `booster`, `thermostat-eem` — is still pinned to `minimq ^0.9`, so the async rewrite is no more field-proven than `rust-mqtt` 0.5. **What would change the choice:** if the broker turns out to be MQTT 3.1.1-only, *both* are out — `minimq` is v5-only and `rust-mqtt`'s `v3` feature is an **empty stub** (`src/v3/mod.rs` is 1 byte; its own README says "`v3`: Unused"); if we ever accept a heap on the MQTT path, `rust-mqtt`'s adoption advantage wins on `alloc`; if `minimq` breaks its API a fourth time before Task 3 lands, revisit. **Cost this implies:** neither crate reconnects or backs off — `rust-mqtt`'s README says so explicitly — so **R9's bounded-backoff reconnect loop is ours to write**, along with re-subscribing and republishing retained state on `ConnectEvent::Connected`. Budget that as real work, not glue. **Eliminated, each with its cause:** `mcutie` 0.4.0 — the only candidate that *does* reconnect for us (5 s backoff) and speaks embassy-net natively, but 22 stars, 51 commits, **2 third-party repos**, **no LICENSE file in the repo or the published crate** (GitHub reports the licence as null; only the Cargo.toml declares MIT), its codec is `mqttrs` 0.4.1 **unmaintained since 2021-05-26**, its broker port is hardcoded to 1883, and its HA module takes the discovery prefix from a **compile-time** `option_env!("HA_DISCOVERY_PREFIX")` and has no `cover` component — R1/R3 need it runtime-validated and R5 needs old configs deleted when it changes. `mountain-mqtt` 0.2.0 and `embedded-mqttc` 1.0.1 — capability-complete but pinned to **`embedded-io-async` 0.6** (`embedded-mqttc` explicitly `>=0.6.0, <0.7.0`), a different trait version from the 0.7 that `embassy-net` 0.9.1's `TcpSocket` implements, so the socket does not satisfy their bounds; `embedded-mqttc` additionally caps `embassy-time` at `<0.5.0` against our 0.5. `myrtio-mqtt` 0.3.0 — pins `embassy-net` 0.7.1 and `embedded-io-async` 0.6.1, publishes **no `repository` field**, and its source repo `MyrtIO-archive/myrtio-mqtt` is **archived**. `tinymqtt` 0.1.3 — `publish(&mut self, topic: &str, payload: &[u8])` writes `Flags::zero()` as the PUBLISH header, so retain is hardcoded off; CONNECT never sets the will bit; 1 star, 10 commits, all within 39 minutes on 2024-06-04, untouched since. `mqtt-async-embedded` 1.0.0 — the strings `retain` and `will` **do not occur anywhere in its source**; its CONNECT flags handle only `clean_session`; its `repository` field is the unedited template `github.com/your-username/…`. `mqttrust` 0.6.0 — `publish(&self, topic, payload, qos)` hardcodes `retain: false` (`src/lib.rs:30`); last release 2022-09-22. `w5500-mqtt` 0.4.0 — `src/publish.rs:28` comments "retain=0, do not retain this message" and hardcodes it; it also targets the W5500 SPI Ethernet chip, not `embassy-net`. `embassy-ha` 0.4.0 — matches our stack exactly and does have `will_retain`, but 8 stars, 72 commits, 196 downloads, **no licence**, and it is an HA-discovery layer, which is the part we already own in `somfy-mqtt`. `embassy-mqtt-lite` 0.3.0 — already ruled out and confirmed: only `publish(&mut self, topic: &str, payload: &[u8])`, no retain; 0 stars, 4 commits, 0 third-party uses. Everything else with real download volume (`rumqttc`, `paho-mqtt`, `ntex-mqtt`, `mqtt5`, `gneiss-mqtt`, `mqttier`) is `std`/tokio, and the rest (`mqttrs`, `mqttrs2`, `gmqtt`, `mqute-codec`, `mqtt-format`, `mqtt-proto`, `embedded-mqtt`) are packet codecs with no client, which is the hand-rolling this rule exists to prevent. Do **not** hand-roll a client: the hard parts here are topic construction, retention and lifecycle, not packet framing. |
| `somfy-mqtt` (topics + HA discovery, Plan 5 Task 1) | [`hass-mqtt-discovery`](https://crates.io/crates/hass-mqtt-discovery) 0.2.0, [`hamqtt`](https://crates.io/crates/hamqtt) 0.1.10 | **Own.** `hass-mqtt-discovery` is MIT and models the payloads well, but it is `std`-only (no `no_std` attribute anywhere in it, and it pulls `thiserror`/`serde`), last published 2022-11-27, and — decisively — it contains **no discovery-topic construction at all**: grepping its source for `discovery_prefix`, `node_id`, `object_id` or `/config` returns nothing. It models the payload, and the payload was never the bug. `hamqtt` 0.1.10 is `no_std` and current (2025-11-04), but it is licensed **LGPL-2.1-only**, which has no "or later" clause and so cannot be combined with our GPL-3.0-only; it also requires a heap (`serde_json` with `alloc`), which the allocation-free crates in this workspace do not have. |
| `somfy-cc1101` (radio driver) | [`cc1101`](https://crates.io/crates/cc1101) 0.1.3, [`cc1101-embassy`](https://crates.io/crates/cc1101-embassy) 0.1.0 | **Own — but this was never consciously evaluated, which was a process failure.** The high-level `cc1101` API is packet-oriented (sync words, address filtering, packet length); this project runs the chip in asynchronous-serial OOK where every one of those is switched off, so we would be using its `lowlevel` raw-register module and writing the same bytes by hand anyway. Revisit if that crate grows async-serial support. |
| Persisted shade table (`somfy_config::ShadeRecord`) | [`postcard`](https://crates.io/crates/postcard) 1.1.3, and the key-value stores already weighed above ([`sequential-storage`](https://crates.io/crates/sequential-storage), [`ekv`](https://crates.io/crates/ekv)) | **Own, narrowly, and for a reason that is about validation rather than bytes.** `postcard` is the serious candidate and is by far the most adopted `no_std` serializer there is: 1,491 stars, 140 forks, ~66 contributors, 385 commits, last push 2026-07-20, 74 open issues / 24 open PRs, **52,932,072 downloads (19,789,013 recent)**, MIT OR Apache-2.0, and licence-compatible with our GPL-3.0-only. It is `no_std` without `alloc` through `to_slice`, and its `use-crc` feature would genuinely have covered the checksum for free — **verified against its own docs, not remembered.** Three things decided against it. **(1) The encoding is the small half.** Every decoded field goes back through `ShadeConfig::new`, `ShadeKind::from_raw`, `TiltMode::from_raw` and the zero-travel-time rule, so that flash cannot deliver a shade `Registry::add_shade` would then refuse — serde deserialises straight into fields past all of that unless a mirror type and a conversion are written, which is where most of `shade.rs` already is. Postcard replaces the ~90 lines of byte-shuffling and none of the ~200 that matter. **(2) The slot is fixed and the ring reads bytes.** A slot is 2048 bytes, "blank" means every byte is `0xFF`, and equal records must encode identically because that is how a write is proved to have landed; postcard's output is varint and variable-length, so a framing header — magic, version, count, seq, padding, CRC — would still be ours, and that header *is* this format. **(3) It would be the odd one of three.** `RTSC`, `RTSW` and `RTSS` are read by the same `SectorRing`/`newest_slot` machinery, and a varint record among two fixed-offset ones costs the property that a hex dump of flash can be read against the file that wrote it. **What would change this:** the same thing that would change the `sequential-storage` ruling — Plan 6's configuration being a whole device model rather than one table. At that size a serde format under the same validation layer, inside a length-framed slot, is the likely answer, and the two rows should be revisited together. |

## Consult the C++ reference EARLY — it is a record of solved problems

The reference implementation runs on this same hardware, in this same room,
talking to these same motors. Every value in it and every structural choice it
made has already survived contact with reality. **Check it before deriving
anything from first principles, not after.**

Read it for three things, in this order of value:

1. **Architecture.** *How* did it solve the problem, not just with what number.
   It receives with a GPIO change-interrupt and an IRAM handler, **not** with
   the RMT peripheral — which sidesteps the "wait for the band to go quiet"
   requirement that RMT reception imposes and that cost us a long detour. A
   structural choice like that is worth more than any constant.
2. **What it delegates.** It does not hand-roll its radio configuration; it
   takes an external library's defaults (`SmartRC-CC1101-Driver-Lib`). Where the
   reference reached for a library, that is a strong signal we should too — see
   the reuse rule above.
3. **Values.** Registers, timings, thresholds. Verify them, then record them in
   [`docs/provenance.md`](docs/provenance.md) as reference-derived.

**This cost real time, concretely.** A full session went into deriving CC1101
AGC settings from the datasheet and sweeping them on hardware, while a
field-proven configuration sat one file away in the library the reference
depends on. Worse, that config contradicted the theory being swept — it caps
gain *harder* while raising the magnitude target and widening the OOK decision
boundary. No amount of first-principles derivation was going to land there.

### The restriction is narrow, and it is not "do not read it"

Only **source comments** must avoid naming the reference. Reading it is
required: `docs/provenance.md` rule 1 says "never invent protocol behaviour —
read the reference, verify the value, then record it here, not in a source
comment." `crates/somfy-migrate/**` is the one documented exception to the
comment rule, because that crate's subject matter *is* the C++ backup format.

The one real hazard is context: agents have exhausted their window wandering a
large C++ codebase. **So the orchestrator extracts the relevant excerpt and
hands it to the implementer** — a named file and line range, or the decoded
values — rather than telling an agent either "go read it all" or "do not look".
Telling an implementer not to look, when the answer is in there, is the failure
this rule exists to prevent.

## Verification

- **A transmitter reporting its own success proves nothing.** If the pulse train
  is built wrongly, the firmware's account of what it sent is wrong in the same
  way. Verify against an independent receiver. The same applies in reverse for
  receive.
- **A single trial that shows nothing proves nothing.** Run at least ten. One
  3-frame burst decoding nothing was read as a broken RMT path; it was not, and
  that cost hours.
- **Match the CI matrix exactly.** Clippy runs on the dev profile and builds run
  on release, so four green release builds do not imply four green clippy runs —
  the ESP32 clippy job was silently red for three tasks that way.
- Unexplained constants are treated as fabricated. Where a value is empirical or
  a table lookup, say so and give the measurement. A fabricated derivation is
  worse than an honest "this is measured".

## Hardware

Two physically identical ESP32-S3 boards exist. **Verify the MAC before every
flash** — see [`docs/hardware-checklist.md`](docs/hardware-checklist.md).
Flashing the wrong one destroys the working device and the reference receiver in
a single action.

Never modify `crates/somfy-rts/tests/fixtures/*.pulses` or
`crates/somfy-migrate/tests/fixtures/*.backup`: real hardware captures and a
real user's private device data.
