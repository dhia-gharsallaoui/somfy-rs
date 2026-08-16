# Hardware bring-up checklist

How to put somfy-rs firmware on a board and prove it is actually working.
Written from the first successful bring-up on 2026-08-15/16, including the
mistakes, because most of the time here was lost to things that are obvious
only in hindsight.

Two independent procedures, because they need different equipment and carry
different risk:

- **[Transmit path](#transmit-path)** — needs a second radio, and puts RF on
  the air.
- **[Rolling-code store](#rolling-code-store)** — needs nothing but the board,
  and touches only flash.

## Transmit path

Proving the firmware is actually transmitting.

## What you need

- An ESP32 board with a CC1101 wired to the pins in `chip::pins` for its chip.
  Only the ESP32-S3 map is hardware-verified; the other three are unverified
  defaults (see `docs/provenance.md`).
- `espflash` ≥ 4.x. **Not** installed by `espup`; install it separately. On
  Fedora, `cargo install espflash` may fail to build `openssl-sys` — take the
  prebuilt musl binary from the espflash releases page instead.
- A **second, independent radio** to verify against. A transmitter reporting
  its own success proves nothing: if the pulse train is built wrongly, the
  firmware's account of what it sent is wrong in exactly the same way.

## 0. Identify the board before every flash

If you have two similar boards, one of them is probably the one you care about.
Flashing the wrong one can destroy a working device and your only reference
receiver in a single action, and they look identical.

```bash
MAC=$(espflash board-info --port /dev/ttyUSB0 2>&1 \
  | sed -n 's/.*MAC address:[[:space:]]*\([0-9a-fA-F:]*\).*/\1/p' | tr 'A-Z' 'a-z')
echo "$MAC"
```

Check it against the board you intend to flash. Do this every time, not once.

## 1. Build and flash

```bash
source ~/export-esp.sh                      # required before any ESP build
cd crates/firmware
cargo build --release --features chip-s3 --target xtensa-esp32s3-none-elf
espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/firmware
```

Run espflash from `crates/firmware`, not from the repo root. `espflash.toml`
there points it at this crate's `partitions.csv`, which is the only table
containing the `rollcode` partition the store needs; espflash only looks for
that config in the current directory and its parent. Flashing without it is not
silently wrong — the store reports `PartitionMissing` and stops — but it is an
avoidable trip.

**`espflash flash` does not erase data partitions**, which is what makes the
rolling code survive a reflash. Use `espflash erase-parts rollcode` when you
actually want it gone.

If espflash reports **"ESP-IDF App Descriptor missing"**, the image lacks
`esp_bootloader_esp_idf::esp_app_desc!()`. Note that nothing in the build
catches this: the descriptor has no runtime behaviour, so the compiler, clippy
and the entire four-chip CI matrix stay green on a binary that cannot be put on
a device at all.

If it reports **"Error while running FlashEnd command"**, drop `--no-stub`.
The flash stub is the default and works; the ROM loader path was seen to fail
this way on an ESP32-S3 on 2026-08-16.

## 2. Watch it boot

```bash
espflash monitor --port /dev/ttyUSB0 --non-interactive
```

Expected:

```
tx: address=0x00C0DE command=Up rolling_code=10 kind=First then 2 x Repeat
tx bring-up complete
```

`espflash monitor` resets the board on connect, which is how you re-trigger a
transmission. It also holds the serial port — nothing else can use it meanwhile.

Anything other than `tx bring-up complete` names its own failure
(`BringUpError`), including which pin disagreed with `chip::pins`.

## 3. Prove it is on the air

Start an independent receiver **before** triggering, and leave it running.

For an ESPSomfy-RTS device as the reference: connect to `ws://<device>:8080`,
send `join:0` (room 0 is `ROOM_EMIT_FRAME`), and parse `42[remoteFrame,{...}]`.
The event name is emitted **unquoted**, so the payload is not valid JSON as a
whole — split at the first comma before parsing. The payload includes a
`pulses` array, so this route is also how raw captures are taken.

Expected for a 56-bit frame:

| Field | Expected |
|---|---|
| `address` | the transmitting address |
| `command` | the command sent |
| `bits` | 56 |
| `sync` | **4** on a first frame, **14** on a repeat |
| `valid` | true |

`sync` is the highest-value assertion — the hardware-sync count is the part of
the pulse train most likely to be wrong and the least visible any other way.

A real capture from first bring-up:

```
FRAME  addr=49374 cmd=Up rcode=10 bits=56 sync=4  valid=True rssi=-74
FRAME  addr=49374 cmd=Up rcode=10 bits=56 sync=14 valid=True rssi=-74
```

`49374` = `0x00C0DE`, the synthetic bring-up address — not paired to anything,
so it proves the frame is on the air without moving hardware.

## 4. Only then, transmit at a motor

**Read the rolling code immediately before transmitting.** Any number written
down anywhere is stale; the reference device keeps burning codes through normal
use.

```bash
curl -s 'http://<reference>:8081/shade?shadeId=1' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["lastRollingCode"])'
```

The field is `lastRollingCode`, and it appears **only** on the per-shade
endpoint — the `/controller` shade list omits it entirely. Transmit at that
value plus a small margin. A code behind the motor's stored value is rejected
as a replay and looks exactly like a broken transmitter, which sends debugging
in precisely the wrong direction.

Prefer `Up` from a closed shade: it starts from a physical limit, so the
motor's true position is known rather than dead-reckoned, and the movement is
unambiguous. A `Down` command to an already-closed shade produces no visible
motion — indistinguishable from a failed transmission.

## Measured values

| Quantity | Value |
|---|---|
| Worst-case symbols, 80-bit first frame | **95** (94 packed + end marker), vs `MAX_SYMBOLS` 96 |
| Worst-case payload | all-zeros (nothing merges), **not** all-ones |
| 56-bit burst duration, first + 2 repeats | **0.405 s** measured, 0.414 s predicted |
| RSSI at the reference receiver | −72 to −75 dBm |
| `transmit_frame` stack usage | ~6.5 KB |

## Diagnosing "nothing was received"

Work outward from the radio; each step rules out one layer.

1. **Is the radio configured and keyed?** Read back the config registers and
   `MARCSTATE` (0x35). `0x13` = TX. Correct readbacks plus `MARCSTATE=0x13`
   means the control path is fine and the fault is in the data path.
2. **Is the timing right?** Time the burst. A 56-bit first-plus-two-repeats
   burst is ~0.41 s. Wildly longer means a clock-divider error — 80× too slow
   is the classic one.
3. **Is the receiver actually subscribed?** Confirm the room, and that the
   event is emitted at all, from the receiver's own source rather than by
   assumption.
4. **Is the pad carrying the waveform?** Sample the GPIO input register in a
   tight loop during transmission and timestamp the edges. Validate the
   measurement with two controls: disable the output driver and confirm the
   input reads 0 while the output register says high (proves you are reading
   the pad), and drive the line from the radio side (proves it is wired).

### Sample size — the mistake that cost the most time here

**A single burst that decodes nothing proves nothing.** During this bring-up
the reference link decoded only ~4–12% of frames, so a 3-frame burst yields
zero decodes most of the time *even from a perfectly working transmitter*.

That produced a confident, wrong diagnosis: one RMT burst (0 decodes) against
one bit-bang burst (4 decodes) was read as "RMT is broken". It is not — the
pad carries the correct pulse train to ±2 µs, and the committed firmware
decodes correctly when you run enough trials. Run **at least 10 bursts** per
configuration before concluding anything, and treat any A/B on a link this
marginal as uninformative until the link is fixed.

Note also that a poor link to the *reference receiver* says nothing about the
link to the *motor*: throughout the above, the motor responded first try in
both directions while the reference decoded ~1 frame in 10.

---

## Rolling-code store

Proving a committed rolling code really is durable. **This procedure needs no
radio and transmits nothing** — `store-check` is a separate binary from
`firmware` for exactly that reason, so proving persistence never involves
keying a transmitter.

The claim being tested is the one the whole persist-before-transmit design
rests on: a code that `commit` accepted survives losing power and survives
reflashing. Neither can be checked on the host, and neither can be checked by
the store reporting on its own success.

## 1. Flash the store harness

Step 0 above — **identify the board** — applies here too, every time.

```bash
source ~/export-esp.sh
cd crates/firmware
cargo build --release --features chip-s3 --target xtensa-esp32s3-none-elf
espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/store-check
espflash monitor --port /dev/ttyUSB0 --non-interactive
```

Each boot prints the region, a survey of every slot, the stored code, one
commit, and the read-back:

```
store: partition 'rollcode' at 0x00200000, 32 slots of 256 bytes
store: survey slots=32 valid=3 blank=29 damaged=0 newest_seq=Some(2) addresses=1
store: load(0x00C0DE) = Some(RollingCode(3))
store: commit(0x00C0DE, 4) ok
store: load(0x00C0DE) = Some(RollingCode(4))
store check complete
```

## 2. The four things worth checking

`espflash monitor` resets the board on connect, so each run is one more boot
and one more commit.

| Check | How | Expected |
|---|---|---|
| Survives a reset | Reconnect the monitor | The code continues; it never restarts from 1 |
| Survives a reflash | `espflash flash …` again, then monitor | Same — `flash` does not touch data partitions |
| Wraps the ring safely | ~35 resets | At the 33rd commit `valid` drops from 32 to 17 as a sector is erased, `damaged` stays 0, the code keeps counting |
| Reports a missing region | Flash with a table lacking `rollcode` | `store check failed: PartitionMissing` — never a silent default |
| Refuses to reseed over damage | Erase the region, then write a lone torn record into it (below) | `store check failed: Unreadable { damaged: 1, slots: 32 }` — a region that is damaged and holds no record is **not** a fresh one |
| Seeds a genuinely blank region | `espflash erase-parts … rollcode` | `region is blank — seeding`, then `RollingCode(1)` |

Measured on an ESP32-S3 on 2026-08-16: all six behaved as above, over a run
from `RollingCode(1)` to `RollingCode(43)` spanning a ring wrap, a reflash, and
a partition-table swap and restore.

The last two are a pair, and the pairing is the point. Refusing costs a person
one `erase-parts` command; reseeding over lost codes costs a physical
re-pairing procedure at every shade. So the store refuses on anything it cannot
positively identify as blank, and only an erased region seeds.

## 3. Simulating a torn write

The interesting failure is losing power part-way through a commit. You can
plant one rather than waiting for it, which is worth doing because the recovery
path is otherwise never exercised: erase the region, write a sector image whose
first slot holds a complete record and whose second holds only the first 64
bytes of one, and boot.

```bash
espflash erase-parts --port /dev/ttyUSB0 --partition-table partitions.csv rollcode
espflash write-bin --port /dev/ttyUSB0 0x200000 sector-with-a-torn-record.bin
```

Expected, and observed on 2026-08-16:

```
store: survey slots=32 valid=1 blank=30 damaged=1 newest_seq=Some(0) addresses=1
store: load(0x00C0DE) = Some(RollingCode(5))     <- the last durable code, not the torn one
store: commit(0x00C0DE, 6) ok                    <- stepped over the wreckage
```

`damaged=1` persists until the ring laps round and erases that sector. That is
correct: the torn record is inert, and the commit that would have overwritten
it steps past instead — writing into it would only clear more bits and produce
another unreadable record, wedging the ring for good.

Plant the torn record **alone**, with no complete record anywhere, and the
store refuses rather than treating the region as fresh:

```
store: survey slots=32 valid=0 blank=31 damaged=1 newest_seq=None addresses=0
store check failed: Unreadable { damaged: 1, slots: 32 }
```

Clear it with `espflash erase-parts … rollcode`, which is the only state the
store will seed into.

### What this does not catch

The store distinguishes a torn write from a good record; it cannot distinguish
a torn write from a *completed* record that was destroyed afterwards. Both
leave damaged slots ahead of a valid one, and there is no second copy to check
against, so a failing sector makes `load` fall back to an older code with no
error. `damaged` above zero on a device nobody power-cut is the only signal
there is — treat it as one. Redundancy belongs with the Plan 6 rewrite.

## Region layout

| Quantity | Value |
|---|---|
| Partition | `rollcode`, data/undefined, 0x200000, 8 KB |
| Record | 256 bytes: header, up to 30 `(address, code)` entries, CRC-32 |
| Slots | 32, in 2 erase sectors of 16 |
| Erases | one sector per full lap — 32 commits |
| Endurance | 100k cycles x 32 commits per cycle = **3.2M commits** |
