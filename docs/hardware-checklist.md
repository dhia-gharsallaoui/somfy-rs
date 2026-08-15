# Hardware bring-up checklist — transmit path

How to put somfy-rs firmware on a board and prove it is actually transmitting.
Written from the first successful bring-up on 2026-08-15/16, including the
mistakes, because most of the time here was lost to things that are obvious
only in hindsight.

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

If espflash reports **"ESP-IDF App Descriptor missing"**, the image lacks
`esp_bootloader_esp_idf::esp_app_desc!()`. Note that nothing in the build
catches this: the descriptor has no runtime behaviour, so the compiler, clippy
and the entire four-chip CI matrix stay green on a binary that cannot be put on
a device at all.

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
