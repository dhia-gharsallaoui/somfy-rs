# somfy-rs Plan 6 — Persistence and OTA

Design source: [`docs/specs/2026-07-15-rust-rewrite-design.md`](../specs/2026-07-15-rust-rewrite-design.md)
§7.5, §7.6, and the persistence rows of §6; plus the four migration obligations
recorded in [`README.md`](../../README.md).

Plan 5 put the controller on the network and into Home Assistant. This plan
makes it a daily driver: configuration that survives, an update path that does
not need a USB cable, and a rollback that means a bad release cannot brick it.

## What is already done

Do not re-plan these:

- **Rolling-code store** — flash-backed, wear-levelled, `seed_if_absent` makes
  overwriting an existing code inexpressible, and a damaged region is refused
  rather than reseeded. Hardware-verified across reboots.
- **Stopgap config store** — Wi-Fi credentials and MQTT settings in `wificfg`,
  shades in `shades`. Explicitly a stopgap this plan replaces; the *seams*
  (`RollingCodeStore`, the record/CRC discipline) survive.
- **Migration obligations (3) and (4)** — unknown shade kinds default to
  `Roller` with a per-shade warning, and a nonzero `skipped_resyncs` gates the
  write behind confirmation. Discharged by `provision_shades --from-backup`.

## The constraint that shapes everything here

**The current partition table has one `factory` app partition and three 8 KB
data regions immediately after it:**

```
factory,  app,  factory,   0x10000,  0x1F0000
rollcode, data, undefined, 0x200000, 0x2000
wificfg,  data, undefined, 0x202000, 0x2000
shades,   data, undefined, 0x204000, 0x2000
```

A/B OTA needs **two** app partitions plus `otadata`. On 8 MB there is room, but
the app slots must grow past 0x200000 — which moves `rollcode`, `wificfg` and
`shades`.

**Moving `rollcode` destroys rolling codes on every already-provisioned board,
and a lost rolling code costs a physical re-pairing at each shade.** That is the
single most expensive failure this plan can cause, and it will happen silently
on the first flash of the new layout unless it is planned for.

So Task 1 is the layout, and it is not a formality.

## Heap: read the current numbers before designing anything

The GitHub-release OTA path wants HTTPS (`reqwless` + `esp-mbedtls`). **TLS is
heap-hungry** — a handshake buffer plus certificate parsing, on a device whose
worst observed heap headroom is ~2.3 KB at 56 KiB.

Per-chip heap sizing is in flight and should change this materially (the S3 has
~176 KB of main stack against a ~6.5 KB deepest call). **Re-read
`crates/firmware/src/heap.rs` before sizing anything** — and note that its
figures are measured, boot-to-boot variance is ~4.2 KB, and a single
before/after reading has already misled this project once.

If TLS does not fit even after resizing, the manual-upload path still delivers
OTA. Say so rather than shrinking something else to make it fit.

---

## Task 1 — Partition layout, and migrating existing boards

The riskiest task in the plan, so it goes first and alone.

- A/B app slots sized for **firmware + embedded UI** (Plan 7 puts UI assets in
  the image via `include_bytes!`, so one image is firmware and UI together).
  Size for what Plan 7 will need, not for today's binary.
- `otadata`, plus the three data regions.
- **A migration path for boards already carrying rolling codes.** Options
  include reading the old region before repartitioning and writing it back, or
  choosing new offsets that leave `rollcode` where it is. **Prefer the layout
  that does not move `rollcode` at all** — the cheapest migration is the one
  that is unnecessary.
- Whatever happens, the hardware checklist must state exactly what an operator
  does to a provisioned board, and what happens if they get it wrong.

## The acceptance test for the whole system

Set by the owner, 2026-08-17, and it is a better definition of done than any
per-task criterion because it exercises the seams rather than the parts:

> **Remove the shades, pair them through the UI, add them back — and at some
> point delete the imported configuration entirely.**

The import from the C++ backup was a bootstrap, not the destination. A shade
this controller *paired itself* owns its own virtual remote, which is what
finally ends the identity collision: today our board transmits as
the imported addresses — the same virtual remote the C++ controller uses, with two
independent rolling-code counters.

Three things currently block that test, and they are all in this plan:

1. **The shades region is host-provisioned only.** There is no runtime write
   path, so the firmware cannot add or remove a shade — only replay a table
   written by `provision_shades`. Task 2 owns this.
2. **`retire_shade` has no caller.** It is written and tested; `inventory.rs`
   records that the firmware "cannot retire". So removing a shade today would
   leave its retained discovery config on the broker forever, and the user would
   see a permanently-unavailable entity in Home Assistant. Task 2 owns this too:
   with stable, bounded ids the announced set is **a `u32` bitmap — 4 bytes** —
   small enough to live in an existing record.
3. **Pairing itself**, in flight separately, plus the UI's pairing assistant
   (Plan 7 §8).

Note the ordering constraint: **deleting the imported configuration must retire
its entities first.** Clearing config and then discovering the orphans is the
failure the spec was written from — cleaning up after its experiments meant
deleting 49 retained topics by hand.

## Task 2 — The real config store

Replaces the stopgap. Per §6: shades, groups, rooms, network, MQTT, security —
**debounced writes**, unlike rolling codes which stay synchronous-before-TX.

Carry forward, because they are already-known defects rather than new work:

- **Stable shade ids.** Being fixed in parallel; the persisted record becomes the
  authority for a shade's id rather than its insertion order.
- **A record of what was *announced*, not just what exists.** Without it
  `MqttConfig::retire_shade` has no caller, and a shade deleted between boots
  leaves an orphaned retained config in the user's broker forever.
- **Frame width and protocol are currently reported and dropped** — a shade the
  old controller drove another way imports looking healthy and never moves.
  Closing it needs fields on `ShadeConfig` *and* a record-format change, which
  is exactly this task.

## Task 3 — Apply `somfy-migrate` output

Migration obligations (1) and (2), the two still open:

1. Persist `MigrationData` — shades, rooms, groups — and **surface v19–v22
   groups and linked remotes whose rolling codes could not be recovered**, so
   the user re-pairs or sets them by hand rather than discovering it at a shade.
2. **Import MQTT settings.** `somfy-migrate` parses the C++ settings record
   cleanly but deliberately defers it, because Plan 3 had nowhere to put it.
   This task is that somewhere. Recorded as a deviation from spec §3.4, not a
   dropped requirement.

## Task 4 — OTA: manual upload

The simpler path, and it needs no TLS. Write the inactive slot from a web upload,
verify, mark bootable.

Do this **before** the GitHub path: it proves the A/B mechanics, the image
verification and the rollback independently of any network or certificate
problem, and it is the fallback when there is no internet.

## Task 5 — Boot self-test and rollback

Per §7.5: a new image runs a self-test — **radio SPI alive, config loads,
network up within a window** — then marks itself valid. Otherwise the bootloader
rolls back.

This is the task that makes OTA safe rather than exciting. Two things matter:

- **The self-test must be able to fail.** Test it by shipping a deliberately bad
  image and confirming the rollback, not by reasoning that it would work. This
  project has a rule about proving a guard fires.
- **Radio SPI alive is a real check and a weak one.** `Cc1101::init` succeeding
  proves the control path only — it says nothing about GDO0/GDO2 or whether
  anything radiates. Its own crate docs say so. Do not let a passing self-test
  imply more than it establishes.

## Task 6 — OTA: GitHub releases

HTTPS fetch of a release manifest, version compare, stream the chip-matching
binary, **verify SHA-256 before marking bootable**.

Gated on the heap question above. The `xtask` that publishes the manifest is
part of this.

## Task 7 — mDNS and SNTP

`edge-mdns` for `http://<hostname>.local`, SNTP for wall-clock time (log
timestamps, and TLS certificate validity if Task 6 lands).

---

## Verification

The project's standing rules apply and have each been earned:

- **An independent receiver, not self-report.** A device that reports its own
  success cannot detect the failures that matter.
- **Ten trials minimum.** A single reading has twice sent this project down a
  wrong path — a 3-frame burst read as a broken RMT path, and a 364-byte AMPDU
  effect read as 3,820 inside a 4,216-byte noise band.
- **Match the CI matrix exactly.** `crates/firmware` is excluded from the root
  workspace, so every workspace-wide command silently skips it. Two CI holes
  have been found this way.
- **Prove guards fire.** Every compile-time assertion in this codebase has been
  verified by breaking it deliberately and restoring.

Hardware-specific to this plan: an OTA cycle **and a forced-bad-image rollback**
are acceptance criteria, per spec §12.

## Out of scope

- The web UI (Plan 7), though Task 1 must size the app slots for it.
- Group transmit — v1.0 fans a group command out per shade.
- The pre-public fixture obligation: the committed `.pulses` encode a real
  remote's address and rolling codes and must be removed or re-captured against
  a throwaway address before this repository goes public. Owner's decision,
  blocks open-sourcing and nothing else.

  **Scope correction, 2026-08-17.** This is larger than the fixtures. Three
  documents also carried a real shade address in plain text — two of them
  written during this work — and have been redacted. That fixes the working
  tree and **not the history**: the addresses remain in earlier commits, so
  going public needs a history rewrite or a fresh repository, not a patch.
  Treat "no real address in the repo" as an obligation with a known outstanding
  breach rather than an invariant currently holding.
