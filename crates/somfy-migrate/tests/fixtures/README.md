# Real device backup fixtures

This crate parses the C++ ESPSomfy-RTS backup file format. The synthetic
fixtures built inline in the test suite exercise every record path on every CI
run, but they are hand-authored from the discovered field map. The **real**
authority is a backup exported from a running C++ device: `../golden.rs` carries
an `#[ignore]`d test that parses one and asserts structural invariants, settling
any format detail the synthetic fixtures got wrong.

That real backup is **not committed** (see [Privacy](#privacy)); this README
explains how to produce one locally and where to drop it.

## Exporting a backup from a device

The `.backup` file this test consumes is exactly the on-flash `shades.cfg`
serialization — `ShadeConfigFile::backup` (`src/ConfigFile.cpp:315-383`) writes
the same header + record stream the migrator reads.

1. Open the C++ firmware's web UI on the running device.
2. Go to **Settings → Backup** and download the backup.
3. You get a `.backup` file. That file *is* the `shades.cfg` format — no
   conversion is needed.

> **Export the backup IMMEDIATELY before migrating.** A stale export replays
> rolling codes: any command the device sends between the export and the
> migration advances the real code past the value in the file, so a motor paired
> after a stale migration rejects the first frames as replayed. The C++ firmware
> papers over this by taking `max(nvs, file)` when it reloads, but a file-only
> migrator has no NVS to fall back on — the file is the only source of truth, so
> it must be fresh.

## Placement

Save the exported file here, under this exact name:

```
crates/somfy-migrate/tests/fixtures/real_device.backup
```

Then run the otherwise-ignored golden test:

```sh
cargo test -p somfy-migrate --test golden -- --ignored
```

It parses the file through `parse_backup` and asserts the structural invariants
documented in `../golden.rs`: supported version range (19..=25), at least one
shade, radio addresses in range 1..0xFFFFFF (exclusive of the 0/0xFFFFFF
sentinels `ShadeConfig::new` rejects), and non-empty shade/group/room names. No expected values are hard-coded — the test adapts to whatever your
device holds — so any committer with a device can validate the parser against
real data without editing the test.

The rolling-code `+1` migration contract is deliberately **not** asserted here:
it cannot be verified from the file alone (the stored last-sent code is the only
input, and a code that wrapped past 65535 is legitimately `0`). That contract is
pinned instead by the always-run pipeline-lock test in `../golden.rs`, which
checks exact `next_code` values against a known synthetic backup.

## Privacy

> **Do not publish `real_device.backup`.** It contains your shades' **radio
> addresses and rolling codes** — the exact secrets a nearby attacker would need
> to forge commands to your motors. Treat it like a key.

The path is gitignored so it can never be committed by accident:

```
# repo .gitignore
crates/somfy-migrate/tests/fixtures/*.backup
```

The ignore pattern covers `*.backup` only; this `README.md` stays tracked. If
you ever need to share a capture for debugging, scrub the addresses and rolling
codes first, or synthesize an equivalent fixture instead.
