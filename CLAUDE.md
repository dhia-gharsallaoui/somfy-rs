# somfy-rs — working rules

## Do not reinvent what a well-maintained crate already does

**Before writing any non-trivial component, search for an existing crate and
say what you found.** Check crates.io, check how actively it is maintained, and
check whether it covers the case at hand. Prefer adopting, wrapping, or porting
a proven implementation over writing a new one.

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
| MQTT client (Plan 5) | [`rust-mqtt`](https://crates.io/crates/rust-mqtt) 0.5.1, [`minimq`](https://crates.io/crates/minimq) 0.13.0, [`embassy-mqtt-lite`](https://crates.io/crates/embassy-mqtt-lite) 0.3.0 | **`embassy-mqtt-lite` REJECTED — no retain flag.** Its only publish is `publish(&mut self, topic: &str, payload: &[u8])`; there is no retain parameter and the docs describe it as QoS 0 fire-and-forget only. Spec R5 is built on retention — discovery configs must be retained so a broker or HA restart repopulates entities, the LWT `offline` must be retained, and **removing an entity means publishing a zero-length retained payload**. None of that is expressible, so it cannot meet the requirement at all. A shame otherwise: it is `no_std` and alloc-free (fixed `MAX_PACKET_SIZE` buffer), generic over `embedded_io_async::Read + Write` so it would drop straight onto `embassy-net`, has a real `LastWill` type, and GPL-2.0-**or-later** is compatible with our GPL-3.0-only. Revisit if it grows retain. Remaining choice is `rust-mqtt` vs `minimq` — decide in Task 3, and **verify retain, LWT-retain and unsubscribe against the actual signatures**, not the crate description. Do **not** hand-roll a client: the hard parts here are topic construction, retention and lifecycle, not packet framing. |
| `somfy-mqtt` (topics + HA discovery, Plan 5 Task 1) | [`hass-mqtt-discovery`](https://crates.io/crates/hass-mqtt-discovery) 0.2.0, [`hamqtt`](https://crates.io/crates/hamqtt) 0.1.10 | **Own.** `hass-mqtt-discovery` is MIT and models the payloads well, but it is `std`-only (no `no_std` attribute anywhere in it, and it pulls `thiserror`/`serde`), last published 2022-11-27, and — decisively — it contains **no discovery-topic construction at all**: grepping its source for `discovery_prefix`, `node_id`, `object_id` or `/config` returns nothing. It models the payload, and the payload was never the bug. `hamqtt` 0.1.10 is `no_std` and current (2025-11-04), but it is licensed **LGPL-2.1-only**, which has no "or later" clause and so cannot be combined with our GPL-3.0-only; it also requires a heap (`serde_json` with `alloc`), which the allocation-free crates in this workspace do not have. |
| `somfy-cc1101` (radio driver) | [`cc1101`](https://crates.io/crates/cc1101) 0.1.3, [`cc1101-embassy`](https://crates.io/crates/cc1101-embassy) 0.1.0 | **Own — but this was never consciously evaluated, which was a process failure.** The high-level `cc1101` API is packet-oriented (sync words, address filtering, packet length); this project runs the chip in asynchronous-serial OOK where every one of those is switched off, so we would be using its `lowlevel` raw-register module and writing the same bytes by hand anyway. Revisit if that crate grows async-serial support. |

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
