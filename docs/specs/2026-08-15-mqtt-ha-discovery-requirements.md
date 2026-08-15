# somfy-rs — MQTT + Home Assistant discovery requirements (Plan 5)

> Refines §7.4 of [`2026-07-15-rust-rewrite-design.md`](2026-07-15-rust-rewrite-design.md),
> which says the topic layout is "new/clean" without saying what that means.
> This document says what it means, and why — every requirement below traces to
> a failure observed on real hardware.

## Why this document exists

On 2026-08-15 the C++ firmware (**v2.5.6**, ahead of the newest tagged release
v2.4.7) was put into production against Home Assistant on a live estate. Its
MQTT publishing works. Its **Home Assistant discovery cannot be made to work in
any configuration.** Both faults were reproduced and measured directly against
a real broker and a real HA instance, not read from source.

This is not a porting note. It is the reason a rewrite gets to fix something.

### Evidence: what was actually observed

The device publishes 49 retained topics, all correct:

```
espsomfyrts/status                :: online
espsomfyrts/shades/2/position     :: 69
espsomfyrts/shades/3/name         :: Roller shade br
```

Its discovery payload is also correct — every topic inside it resolves:

```json
{"~": "espsomfyrts/shades/1",
 "availability_topic": "espsomfyrts/status",
 "position_topic": "~/position", "state_topic": "~/direction",
 "command_topic": "~/direction/set", "set_position_topic": "~/target/set"}
```

**Only the address the config is sent to is wrong.**

| Root topic | Discovery topic | Published to | HA result |
|---|---|---|---|
| `espsomfyrts` | `homeassistant` | `espsomfyrts/homeassistant/cover/1/config` | ignored — not under HA's prefix |
| `homeassistant` | *(empty)* | `homeassistant//cover/1/config` | ignored — empty component segment |
| *(empty)* | `homeassistant` | `homeassistant/cover/1/config` | discovered, but payload becomes `"~": "/shades/1"` (leading slash) while the device publishes to `shades/1` — **entities permanently `unavailable`** |

Every combination fails. The three failure modes are mutually exclusive, so no
configuration escapes.

### Root cause

1. **The state root topic is prepended to the discovery topic.** The two are
   independent namespaces in MQTT discovery — that is the entire purpose of the
   `~` field, which lets a config published under `homeassistant/` point at
   state living under `mydevice/`. Conflating them is the primary bug.
2. **An empty root topic produces leading-slash topics in the payload**
   (`"/shades/1"`) while the publisher writes to `shades/1`. Different topics.
3. **Empty segments are not collapsed**, so an empty discovery topic yields
   `homeassistant//cover/…`.

For reference, every well-behaved integration (Zigbee2MQTT, ESPHome, Tasmota,
Shelly) publishes discovery to the standard prefix while keeping state under its
own root. Nothing else needs the prefix moved. Upstream's own wiki steers HA
users to a HACS plugin instead, which is consistent with discovery never having
been exercised against HA.

### The HA contract, as verified

Confirmed empirically by publishing both shapes to a live broker and observing
which one HA acted on:

```
<discovery_prefix>/<component>/[<node_id>/]<object_id>/config

homeassistant/espsomfyrts/cover/1/config   -> IGNORED (component must be first)
homeassistant/cover/espsomfyrts/1/config   -> entity created, read live position
```

- `<component>` **must** be the segment immediately after the prefix.
- `<node_id>` is optional and unused by HA, but must match `[a-zA-Z0-9_-]`.
- `<object_id>` same character class; does not influence the entity_id.
- HA supports exactly **one** discovery prefix. It is global. A device that
  forces it to be changed taxes every other MQTT device on the network forever.

## Requirements

### R1 — Discovery and state namespaces are independent (MUST)

`discovery_prefix` and `state_root` are two separate configuration values and
**must never be concatenated**. The discovery topic is built from
`discovery_prefix` alone; the state topics from `state_root` alone; the payload
links them with `~`.

This single requirement is the whole reason MQTT is unusable on the C++ build.

### R2 — Discovery topic construction (MUST)

```
{discovery_prefix}/cover/{node_id}/{object_id}/config
```

- `discovery_prefix` default `homeassistant`; **never** shipped as anything else.
- `component` is a literal from HA's supported set, emitted by the firmware, not
  user-configurable.
- `node_id` and `object_id` MUST be sanitised to `[a-zA-Z0-9_-]`. A shade named
  `Salon / Porte-fenêtre` must not produce topic segments.
- No empty segment may ever be emitted. Building a topic containing `//` is a
  bug, not a configuration outcome.

### R3 — Configuration validation rejects, never degrades (MUST)

Invalid MQTT configuration must be **refused at the point of entry** with a
message naming the field — not accepted and silently published to a topic
nobody reads. Specifically reject: empty `discovery_prefix`, empty
`state_root`, any value containing `#`, `+`, leading/trailing `/`, or `//`.

The C++ failure mode is precisely that every bad combination was accepted and
looked like it had worked.

### R4 — Payload topics (MUST)

- `~` set to the shade's state base under `state_root`, absolute, no leading `/`.
- `availability_topic` absolute, **under `state_root`** — never under
  `discovery_prefix`. Putting it at `{prefix}/status` collides with Home
  Assistant's own birth/will topic (`homeassistant/status` by default), which
  makes availability meaningless: HA's birth message would mark the device
  available while it is offline.
- All other topics relative to `~`.
- `unique_id` stable across reboots, config changes and firmware updates.

### R5 — Lifecycle (MUST)

- LWT registered on CONNECT, publishing `offline` retained to
  `availability_topic`; `online` published retained after CONNACK.
- Discovery configs published **retained**, so a broker restart or an HA restart
  re-populates entities without touching the device.
- **Removing an entity means publishing a zero-length retained payload to its
  config topic.** Deleting a shade, or disabling discovery, MUST do this for
  every entity it owns. Otherwise the estate accumulates orphaned entities that
  can only be cleared by hand — cleaning up after the experiments behind this
  document required deleting 49 retained topics manually.
- Changing `state_root` or `discovery_prefix` MUST delete the old retained
  configs before publishing the new ones.

### R6 — Command topics are never retained (MUST)

Commands (`.../set`) must be published and subscribed with `retain = false` and
QoS 0 or 1. A retained command replays on every reconnect — the upstream wiki
warns about this explicitly, and a shade that closes itself on every broker
restart is a support nightmare.

### R7 — Publish the full entity set, not just covers (SHOULD)

The C++ MQTT path exposes **covers only**. Its HACS plugin exposes roughly 24
entities for the same device — sun/wind binary sensors, sun-sensor switches, ten
diagnostic sensors, buttons, a firmware update entity. That gap is the stated
reason upstream tells HA users to avoid MQTT.

somfy-rs should close it: publish discovery for sensors, binary sensors and
diagnostics alongside covers, so **MQTT alone is a complete integration** and no
custom component is required. This is the difference between "MQTT works" and
"MQTT is enough".

### R8 — Tilt (MUST, where supported)

Tilt-capable shades expose `tilt_command_topic` / `tilt_status_topic` per
§7.4. Non-tilt shades must omit them rather than publish dead topics.

### R9 — Degraded operation (MUST)

A broker that is down, unreachable, or rejecting credentials must not affect
radio control, the web UI, or the REST API. Reconnect with bounded backoff.
Per §11 of the design spec, MQTT is a degradable service.

## Acceptance criteria

Host-testable, in keeping with the project's existing culture:

1. **Topic construction is a pure function, unit-tested.** Given config and a
   shade, assert the exact discovery topic and the exact payload topics. Table
   test including the three C++ failure combinations above, asserting somfy-rs
   produces a *valid* topic for each input or refuses the config.
2. **Property test:** for any valid config and any shade name (including
   Unicode, slashes, spaces), the generated topic matches
   `^[a-zA-Z0-9_\-]+(/[a-zA-Z0-9_\-]+)*$` and contains no `//`.
3. **Round-trip:** every topic referenced in a discovery payload (after `~`
   expansion) is a topic the firmware actually publishes to or subscribes to.
   This is the check that would have caught the C++ leading-slash bug — the
   payload and the publisher disagreed and nothing noticed.
4. **Config rejection:** each invalid input in R3 returns a typed error naming
   the field.
5. **Integration (manual, documented):** against a real HA — entities appear
   without YAML, report correct position, and a command **moves a shade**.
   Appearing is not working; the C++ build produced three entities that were
   permanently `unavailable`.

## Non-goals

- Compatibility with the C++ topic layout. It is not salvageable, and no
  deployment depends on it — discovery has never worked, so no HA installation
  can be relying on those topics.
- Supporting a user-configurable `component` segment.

## Open questions

1. Does the migration path (`somfy-migrate`) carry the C++ MQTT settings across?
   If so, `rootTopic`/`discoTopic` must map onto the new independent fields with
   the concatenation *undone*, and a migrated `discoTopic` of `homeassistant`
   should become `discovery_prefix`, not part of the state root.
2. Device-level discovery (`homeassistant/device/<id>/config`, one payload for
   all components) is now supported by HA and would cut publish volume
   substantially for a multi-shade device. Worth evaluating against the
   per-entity form before Plan 5 implementation.

## Unrelated finding worth carrying into Plan 7

The C++ web server caps concurrent clients at **5** and holds a websocket per
open UI tab. Exceeding it returns "Too many clients connected" and locks the
operator out of the device — including out of the network settings needed to
fix whatever caused it. During this investigation, a polling HA integration
whose target address had gone stale exhausted the limit on its own.

Whatever Plan 7 serves, it must not be possible for a misbehaving client to
lock an operator out of network configuration.
