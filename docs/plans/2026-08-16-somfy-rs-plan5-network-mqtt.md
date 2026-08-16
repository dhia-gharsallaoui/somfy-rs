# somfy-rs Plan 5 — WiFi, MQTT and Home Assistant discovery

Design source:
[`docs/specs/2026-08-15-mqtt-ha-discovery-requirements.md`](../specs/2026-08-15-mqtt-ha-discovery-requirements.md)
— nine requirements written from an observed field failure, with host-testable
acceptance criteria.

Plan 4 is complete: the radio works in both directions, proven on hardware. This
plan gives the controller a network, a command source, and a Home Assistant
integration that is complete enough to need no custom component.

## Dependency evaluation

Required by [`CLAUDE.md`](../../CLAUDE.md) before writing anything. **Done up
front, on the real dependency graph, not from crate descriptions.**

| Need | Chosen | Evidence |
|---|---|---|
| WiFi | **`esp-radio` 0.18.0** | Resolves cleanly against our `esp-hal 1.1.2` — verified by adding it and inspecting `cargo tree -i esp-hal`, which stayed at 1.1.2. Same repo as esp-hal, so it tracks the HAL. `cargo add` skips `1.0.0-beta.0` as a pre-release; do not chase the beta without a reason. `esp-wifi 0.15.1` is the older line. |
| Network stack | **`embassy-net` 0.9.1** | The standard stack for Embassy, which we already run via `esp-rtos 0.3`. |
| MQTT | **evaluate `rust-mqtt` 0.5.1 vs `minimq` 0.13.0 in Task 3** | Both `no_std`, both maintained, both MIT/Apache-2.0 and so compatible with our GPL-3.0-only. `rust-mqtt` targets embassy directly; `minimq` (quartiq) is well-regarded and transport-agnostic. Pick on evidence — see Task 3. |
| HA discovery payloads | **own** | These are our data model rendered as JSON; there is nothing to reuse. The *topic and payload rules* come from the spec. |

**Do not hand-roll an MQTT client.** The spec's hard parts are topic
construction, retention and lifecycle semantics — not packet framing.

## The finding that changes the firmware's shape

**WiFi requires a heap.** `esp-radio` pulls `esp-alloc`, `linked_list_allocator`
and `rlsf`, and the build fails today with:

```
error[E0463]: can't find crate for `alloc`
```

because `crates/firmware/.cargo/config.toml` sets `build-std = ["core"]` only.
(The `allocator-api2` `E0308` errors that accompany it are downstream of the
same cause, not a version skew — they disappear with `alloc` present.)

So Plan 5 must add `alloc` to `build-std` and initialise an allocator. **This is
a real change of character**: every crate in this workspace is allocation-free
today, and that is a property worth keeping where it still holds.

Constrain it deliberately:

- The heap exists **for `esp-radio`'s internals**. `somfy-rts`, `somfy-domain`,
  `somfy-rmt`, `somfy-cc1101`, `somfy-store` and `somfy-tasks` stay
  allocation-free and must keep building for `thumbv7em-none-eabihf`, which is
  what proves it.
- Size the heap explicitly and say where the number came from. Record it in
  `docs/provenance.md` like any other measured constant.
- The radio and state tasks must not allocate on the frame path. A GC pause has
  no analogue here, but heap exhaustion mid-frame does.

## Provisioning — the staging decision

The firmware currently boots with an empty registry: it receives and tracks, but
**nothing can command it**. Plan 4 ships no config store and no command source,
so most of this plan is unobservable until that is fixed.

**Ruling: Plan 5 brings a minimal persisted config — WiFi credentials, MQTT
settings, and shades — built on the same flash primitives as the rolling-code
store, and explicitly marked as a stopgap that Plan 6 replaces.**

The precedent is already set: `RollingCodeStore` is a trait whose Plan 4
implementation is a minimal flash region, with the seam and its guarantees
surviving into Plan 6. Do the same here. The alternative — waiting for Plan 6 —
would leave Plan 5 untestable end to end, and "it compiles" is not a standard
this project has accepted anywhere else.

Credentials in flash are **not** secrets-at-rest. Say so plainly in the docs
rather than implying protection that is not there; anyone with the board has the
WiFi password. Do not invent an encryption scheme.

---

## Task 1 — Topic construction and config validation (pure, host-tested)

**Crate:** new `crates/somfy-mqtt`, hardware-free, `no_std`.

This is the entire reason the C++ MQTT integration is unusable, and it is pure
data. Do it first, and do it properly, before any network code exists.

Implements spec R1–R4 and acceptance criteria 1–4:

- `discovery_prefix` and `state_root` are **separate values that must never be
  concatenated** (R1). Model them so concatenating is not expressible.
- Discovery topic `{discovery_prefix}/cover/{node_id}/{object_id}/config` (R2),
  with `node_id`/`object_id` sanitised to `[a-zA-Z0-9_-]`. A shade named
  `Salon / Porte-fenêtre` must not produce topic segments.
- **No empty segment, ever.** A topic containing `//` is a bug, not a
  configuration outcome.
- Config validation **rejects, never degrades** (R3): empty prefix or root, or
  any value containing `#`, `+`, leading/trailing `/`, or `//`, returns a typed
  error naming the field. The C++ failure mode is that every bad combination was
  accepted and looked like it worked.
- `availability_topic` under `state_root`, **never** under `discovery_prefix`
  (R4) — `{prefix}/status` collides with Home Assistant's own birth/will topic,
  which makes availability actively wrong rather than merely absent.

**Tests.** Table test including the three C++ failure combinations. Property
test: for any valid config and any shade name including Unicode, slashes and
spaces, the topic matches `^[a-zA-Z0-9_\-]+(/[a-zA-Z0-9_\-]+)*$` and contains no
`//`. And the **round-trip check** — every topic referenced in a discovery
payload after `~` expansion is a topic the firmware actually publishes or
subscribes to. That last one is what would have caught the C++ leading-slash
bug, where the payload and the publisher disagreed and nothing noticed.

## Task 2 — WiFi and the network stack

**Crate:** `crates/firmware`.

`esp-radio` 0.18.0 + `embassy-net` 0.9.1, plus the `alloc` work above. Station
mode, DHCP, bounded reconnect backoff.

Per spec R9 and design spec §11, **the network is a degradable service**: a
broker that is down, unreachable, or rejecting credentials must not affect radio
control. The radio task must not be able to block on the network — that
separation is the reason the tasks are split, and it is easy to lose here.

Report the heap figure and the measured stack headroom.

## Task 3 — MQTT client, lifecycle and retention

**Crate:** `crates/firmware` for the transport; keep anything testable in
`somfy-mqtt`.

**The client is chosen: `minimq` 0.13.0** (runner-up `rust-mqtt` 0.5.1). See the
survey in `CLAUDE.md` — 230 crates swept, every `no_std` client's source read,
both finalists proven to build for `xtensa-esp32s3-none-elf` under
`build-std = ["core"]` with retained publish, retained LWT, zero-length retained
removal, non-retained QoS 1, subscribe and unsubscribe all exercised.

**Broker confirmed: the Home Assistant Mosquitto add-on**, which ships Mosquitto
2.x and has spoken MQTT 5.0 since 1.6. This clears the one condition that would
have eliminated *both* finalists — `minimq` is v5-only and `rust-mqtt`'s `v3`
feature is an empty stub (`src/v3/mod.rs` is 1 byte, `src/v3/packet/mod.rs` is
0 bytes, README: "`v3`: Unused"). Still worth confirming from a real CONNACK on
first connect rather than from version numbers; it is one log line and it
converts an inference into an observation.

**Reconnection is ours to write.** Neither finalist provides it — `rust-mqtt`
says so outright ("does not implement opinionated connection management —
automatic reconnects, keepalive loops, retry policies... intentionally left to
the user"), and `minimq` is the same shape. So R9's bounded backoff, plus
re-subscribing and republishing retained state on reconnect, is real work to
budget rather than glue. `minimq`'s `ConnectEvent::Connected` vs `Reconnected`
is the hook for "republish discovery only on a fresh broker session".

Then implement R5 and R6, which are where the sharp edges are:

- LWT registered **on CONNECT**, publishing `offline` retained to
  `availability_topic`; `online` retained after CONNACK.
- Discovery configs published **retained**, so a broker or HA restart
  re-populates entities without touching the device.
- **Removing an entity means publishing a zero-length retained payload to its
  config topic.** Deleting a shade, or disabling discovery, must do this for
  every entity it owns. Cleaning up after the experiments behind the spec
  required deleting 49 retained topics by hand.
- Changing `state_root` or `discovery_prefix` must delete the old retained
  configs **before** publishing the new ones.
- Commands are **never retained**, QoS 0 or 1. A retained command replays on
  every reconnect — a shade that closes itself whenever the broker restarts.

## Task 4 — The full entity set

R7, and the difference between "MQTT works" and "MQTT is enough". The C++ path
exposes covers only; its HACS plugin exposes ~24 entities for the same device,
and that gap is why upstream tells HA users to avoid MQTT.

Publish discovery for sensors, binary sensors and diagnostics alongside covers,
so no custom component is required. Tilt topics only for tilt-capable shades
(R8) — omit them rather than publishing dead topics.

## Task 5 — Integration against real Home Assistant

Acceptance criterion 5, and the one that cannot be faked:

> entities appear without YAML, report correct position, and a command **moves a
> shade**. Appearing is not working — the C++ build produced three entities that
> were permanently `unavailable`.

Also verify: broker restart re-populates entities from retained configs; a
deleted shade leaves no orphan; killing the broker does not affect radio
control.

Transmissions can be triggered and verified without a human — see
[`docs/hardware-checklist.md`](../hardware-checklist.md). **Ten trials, not one.**

---

## Out of scope

- Compatibility with the C++ topic layout. Not salvageable, and nothing depends
  on it — discovery never worked there, so no HA install can rely on those
  topics.
- A user-configurable `component` segment.
- OTA and A/B partitions (Plan 6); the web UI (Plan 7).
- Secrets-at-rest. Flash-stored credentials are not protected; say so.

## Carried forward

- **Pre-public fixture obligation.** The committed `.pulses` encode a real
  remote's address and rolling codes. Re-capturing our *own* transmitter would
  make the fixtures circular — they would pin the decoder against our encoder
  instead of against real hardware — so the honest options are to remove them or
  capture a real remote paired to a throwaway address. Owner's decision.
- **Travel times are uncalibrated** (`upTime`/`downTime` still 10,000 ms), so
  position dead-reckoning will be wrong in a way that is not the firmware's
  fault. Relevant as soon as MQTT reports position.
