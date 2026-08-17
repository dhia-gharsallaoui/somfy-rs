# Hardware bring-up checklist

How to put somfy-rs firmware on a board and prove it is actually working.
Written from the first successful bring-up on 2026-08-15/16, including the
mistakes, because most of the time here was lost to things that are obvious
only in hindsight.

Six independent procedures, because they need different equipment and carry
different risk:

- **[Transmit path](#transmit-path)** — needs a second radio, and puts RF on
  the air.
- **[Receive path](#receive-path)** — needs a second radio to transmit at you;
  this board stays silent.
- **[Rolling-code store](#rolling-code-store)** — needs nothing but the board,
  and touches only flash.
- **[Wi-Fi provisioning](#wi-fi-provisioning)** — needs the board and the
  network's passphrase; touches only flash and puts nothing on the 433 MHz
  band.
- **[Shade provisioning](#shade-provisioning)** — touches only flash, and is
  where each shade's radio address is chosen.
- **[Pairing a shade](#pairing-a-shade)** — needs you standing at the motor,
  and **transmits at a real motor**. It is the only procedure here that can
  change what a motor in somebody's house responds to.
- **[Controller](#controller)** — the real firmware: receives, tracks, and
  transmits when commanded.

Which binary is which matters here, because they differ in whether they key a
transmitter:

| Binary | Keys the radio | What it is for |
|---|---|---|
| `tx-check` | **yes**, at a synthetic address | proving the transmit path |
| `store-check` | no — flash only | proving the rolling-code store |
| `config-check` | no — flash only | proving the Wi-Fi config region |
| `firmware` | no, until commanded — and a connected broker is a command source | the controller itself |

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
espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/tx-check
```

`tx-check`, not `firmware`: the transmit harness is a binary of its own, and
the controller in `firmware` deliberately keys nothing by itself.

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

## Receive path

Proving the firmware decodes a real frame off the air. **This board transmits
nothing** — the frames come from a second device.

### Get a transmitter you can trigger on demand

The single biggest improvement to this procedure. An ESPSomfy-RTS device will
transmit a real Somfy frame over HTTP:

```bash
curl -s "http://<reference>:8081/shadeCommand?shadeId=1&command=Up"
curl -s "http://<reference>:8081/shadeCommand?shadeId=1&command=Down"
```

**Verify the shade position actually changed** — that is the proof a frame was
really transmitted, and it is the only reliable one:

```bash
curl -s "http://<reference>:8081/shade?shadeId=1" \
  | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["position"],d["target"])'
```

0 = fully open, 100 = fully closed. Alternate Up/Down so the shade is not driven
against a limit, and allow ~10 s of travel between commands. `command=My` is
**not** a reliable trigger: with no my-position set it appears to be a no-op.

This removes a human from the loop, which matters more than convenience — it is
what makes ten trials feasible, and single trials are how this project has twice
reached a confident wrong conclusion.

### Expected output

Frames surface on the internal frame channel, which the state task consumes. With
an empty registry the state task prints nothing, so during bring-up substitute a
probe task that drains the channel and prints. A good run:

```
rx[1]: address=0x0FC115 command=Up   rolling_code=177
rx[2]: address=0x0FC115 command=Down rolling_code=178
rx[3]: address=0x0FC115 command=Up   rolling_code=179
rx[4]: address=0x0FC115 command=Down rolling_code=180
```

Check the command matches what you fired and the rolling code advances. An
address alone is not enough — a decoder can produce a plausible address from
noise.

### Diagnosing "nothing was received"

Work outward from the radio. Each step rules out one layer, and each of these
was a live hypothesis at some point:

1. **Is the radio in receive?** Read `MARCSTATE` (0x35): `0x0D` is RX, `0x01` is
   IDLE. The driver must strobe `SRX` — in asynchronous serial mode GDO2 is only
   driven while receiving, so an unstrobed radio is indistinguishable from a
   quiet band.
2. **Is the pad carrying anything?** Sample the GDO2 GPIO directly in a tight
   loop, bypassing RMT. This separates "no signal" from "signal the RMT or
   decoder mishandles", and they need opposite fixes.
3. **What does the band look like idle?** With a correct AGC the line should rest
   **low** with near-zero edges. Hundreds of edges per second is the AGC slicing
   its own noise floor, and it makes RMT reception impossible: a reception only
   ends after `IDLE_THRESHOLD_US` of quiet, so constant noise fills the buffer
   and esp-hal discards the whole transaction.
4. **Does a real frame arrive whole?** Fire a transmission and look for a burst
   with a `longest_high` near **10,000 µs** — the wake-up pulse. Wake-up present
   but body missing means the frame is arriving and being sliced away.

### Judge captures by decode, not by edge count

The trap that cost the most here. **Idle edge count does not predict whether a
frame survives.** An AGC configuration was once chosen because it produced the
quietest idle band, and that setting is precisely the one that loses the frame
body under any attenuation. Another candidate reaches near-zero edges by pinning
the line *high*, which is just as undecodable.

Two rules follow:

- A capture counts only if it produces a **checksum-valid frame**. Everything
  else is a proxy.
- Also confirm the line **rests low** when idle, not merely that it is quiet.

And note that a whole 56-bit frame is about **89 merged edges**, not ~180 — the
larger figure counts half-symbols. The committed golden captures are the
reference: `up_56bit_1.pulses` is 89 pulses, `down_56bit_1.pulses` is 97.

### Creating discriminating conditions

Unattenuated, at short range, every sane configuration decodes everything and
nothing tells settings apart. To compare them, walk the signal out of the
101.6 kHz channel filter by writing `FSCTRL0.FREQOFF` — a calibrated stand-in
for distance. Test-time only; it must never reach `init`.

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

---

## Wi-Fi provisioning

Putting a network's credentials on a board. **This procedure touches flash and
the 2.4 GHz radio only** — nothing goes out on 433 MHz, and the controller's
Somfy behaviour is unchanged by it either way.

### The passphrase never enters this repository

There is no constant to edit, no environment variable, and no build-time
credential anywhere in the firmware. The device reads its credentials from the
`wificfg` flash region and from nowhere else, and the region is written by a
host-side tool that takes both values on **standard input** — so they exist
only in the operator's terminal, in the file it writes, and in the board's
flash. Not in git, not in a shell history, not in `ps` output, not in a build
cache.

### These credentials are not protected

Flash encryption is not enabled. **Anyone who can hold the board can read the
passphrase off it** with `espflash read-flash`, and the same is true of the
file the tool writes. That is stated rather than mitigated: an obfuscation
scheme would need its key in the same flash and would protect nothing. The only
real fix is ESP32 flash encryption with the key in eFuse, which is a
device-provisioning decision, not a firmware one.

Delete the intermediate file once the board has it.

### 1. Write the region image

```bash
cargo run -p somfy-config --example provision -- wificfg.bin
# SSID (empty for no network): <typed>
# passphrase (empty for an open network): <typed>
# broker IPv4 address (empty for no broker): <typed>
# broker port [1883]: <return>
# broker username (empty for anonymous): <typed>
# broker password: <typed>
# discovery_prefix [homeassistant]: <return>
# state_root [somfyrs]: <return>
```

It validates before writing — an SSID over 32 bytes, a passphrase under 8
characters, an embedded NUL, a broker address no TCP connection could reach, a
port of zero, a topic root with a wildcard or a trailing slash, or two roots
that name the same namespace — so a typo is refused here rather than three
flashes later as a board that silently will not associate or an integration
that publishes where nothing reads.

Both halves are optional and independent. An empty SSID writes "no network
configured"; an empty broker address writes "no broker configured". A board
carrying either still receives and decodes.

**The broker address is an IPv4 address, not a host name**, and that is
deliberate: `embassy-net` is built here without its `dns` feature, so a name
would be a value the tool accepts, the flash stores, and the network layer can
do nothing with.

**Prefer the broker's address on the ESP's own subnet** where it is dual-homed:
it removes any dependency on inter-VLAN firewall rules from the path.

### 2. Put it on the board

Step 0 above — **identify the board** — applies here too, every time.

```bash
cd crates/firmware
espflash erase-parts --port /dev/ttyUSB0 --partition-table partitions.csv wificfg
espflash write-bin   --port /dev/ttyUSB0 0x202000 wificfg.bin
rm wificfg.bin
```

The erase is **not optional when re-provisioning**. The tool writes sequence
number 0, so an existing record with a higher sequence number stays newest and
the new credentials are simply ignored — a board that looks provisioned and
joins the old network.

### 3. Confirm it landed

```bash
espflash monitor --port /dev/ttyUSB0 --non-interactive   # reset the board
```

```
config: partition 'wificfg' at 0x00202000, 16 slots of 512 bytes
config: survey slots=16 valid=1 blank=15 damaged=0 newest_seq=Some(0)
config: broker 192.0.2.10:1883 (authenticated), discovery_prefix='homeassistant' state_root='somfyrs'
wifi: joining '<your ssid>'
wifi: associated on channel 6 (-52 dBm)
net: address 10.0.0.57/24 gateway Some(10.0.0.1)
```

`wifi: associated` without a following `net: address` means the station joined
and DHCP did not answer — a different fault from a wrong passphrase, and the
reason the two are separate lines.

A wrong passphrase looks like this, and keeps looking like it:

```
wifi: association failed — Disconnected(... reason: NoAccessPointFound ...)
wifi: retrying in 1000 ms
```

with the delay doubling to a 60 s ceiling. **The delay sequence is the check**:
1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000. A delay that never grows
is a busy retry; one that grows past 60 s means a rebooted router will not be
rejoined without a power cycle.

Two further things to expect, both deliberate:

- **The log goes quiet once the delay stops changing.** Failures 1–7 are
  logged, 8 and 9 are not, 10 is. A log line is written with interrupts
  disabled, and the receiver has about 5 ms to re-arm between a frame and its
  repeat, so an absent access point must not be allowed to spend that budget
  twice a second forever.
- **An access point that associates and *then* drops you does not reset the
  backoff.** The firmware prints `the link lasted N ms, under the 10000 ms it
  takes to count as working` and keeps escalating. Captive portals, MAC policy
  checks and networks with no DHCP server all look like success followed
  immediately by failure, and resetting on association alone would pin the
  retry at one second forever.

### The region, and exercising the write path

`config-check` mounts the region, surveys it, writes a **placeholder**
credential (`SSID_NOT_PROVISIONED`) and reads it back. It is the only hardware
evidence that the write path works, because the controller itself only ever
reads.

```bash
espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/config-check
```

**It overwrites a provisioned credential**, exactly as `store-check` advances a
rolling code. Re-provision afterwards.

| Quantity | Value |
|---|---|
| Partition | `wificfg`, data/undefined, 0x202000, 8 KB |
| Record | 512 bytes: magic `RTSW`, version **2**, flags, lengths, seq, broker address+port, SSID(32), PSK(64), username(32), password(64), discovery_prefix(32), state_root(32), CRC-32 |
| Slots | 16, in 2 erase sectors of 8 |
| Rolling codes | untouched — `rollcode` keeps its 0x200000 offset in the new table |

**A board provisioned before the MQTT settings existed must be re-provisioned.**
Version 2 moved every field and changed the record length, so a version 1 record
read as 512 bytes fails its checksum and is reported as a damaged slot rather
than as an old format. The survey line at boot says so; the remedy is the
`erase-parts` + `write-bin` above.

---

## Shade provisioning

Putting shades on a board. **This procedure touches flash only** — nothing goes
out on 433 MHz, and it is the step that turns a listening controller into a
commandable one.

### The rolling code is the field that can break a pairing

Every other value here is a preference. The rolling code is not: a motor stores
the last code it accepted and **rejects anything at or below it as a replay**,
which looks exactly like a broken transmitter and is undone only by re-pairing
at the motor.

So the record's code is a **seed**, and the firmware applies it *only* when its
rolling-code store holds nothing for that address. On the second boot, and every
boot after, the stored code wins and the record's value is ignored — the serial
line says which of the two happened, per shade. What to enter:

- **A motor another controller has driven:** a value **above** the last code
  that controller sent. `somfy-migrate` recovers it from a C++ backup as
  `next_code`, already corrected from the stored last-sent value.
- **A motor you will pair fresh:** anything. The pairing procedure teaches the
  motor whatever the transmitter sends.

If you are unsure, **enter a value comfortably above your best guess.** Skipping
codes forward is free — the motor accepts any code ahead of its stored one — and
landing below it is not.

**If you are replacing a controller, do not guess at all — import its backup.**
Step 1 below has a `--from-backup` form that reads every field of every shade,
rolling codes included and already corrected, out of the file that controller
exports. It is the same tool writing the same 2048 bytes; the only difference is
that nobody transcribes the one number that costs a walk to the shade.

### Order is identity

Shade ids come from the order entered: the first is `ShadeId(0)`, which Home
Assistant sees as `shade_0`. **Appending is safe. Reordering or removing
renumbers everything after the change**, which in Home Assistant means new
entities, and the old ones left behind as retained orphans nothing will clear.

### 1. Write the region image

```bash
cargo run -p somfy-config --example provision_shades -- shades.bin
# controller MAC, to allocate radio addresses from — ...: <typed, or return>
#   addresses will be allocated from 0x8XXXXX — check that against the board's
#   boot line before flashing. Two boards printing the same value is a bug.
#
# [0] name (empty to finish): <typed>
#   radio address [0x8XXXXX, this controller's own]: <return>
#   kind [roller] (roller, blind, drapery-left, awning, shutter, ...): <return>
#   tilt mode [none] (none, tilt-motor, integrated, tilt-only, euro): <return>
#   full travel up time in ms [10000]: <return>
#   full travel down time in ms [10000]: <return>
#   full tilt time in ms [7000]: <return>
#   next rolling code to send — ...: <typed>
# [1] name (empty to finish): <return to finish>
```

### The address decides whose remote this controller is

A Somfy motor learns **remotes**, not controllers: every frame carries a 24-bit
remote address, and the motor stores the last rolling code it accepted *per
address*. So two controllers transmitting at one address is not a cosmetic
clash. Each keeps its own counter, neither knows what the other has sent, and
the first to fall behind starts sending codes the motor has already accepted and
rejects as replays. That motor stops answering it, and stays that way until
somebody re-synchronises at the shade.

That is exactly what an imported table produces if it is flashed and left: the
addresses in it belong to **the controller it was exported from**, and if that
controller is still powered, both are now one remote with two counters.

So the first prompt asks for this board's MAC and allocates each shade an
address derived from the *device-unique* half of it — a space no other
controller's scheme can reach. Pressing return at the address prompt takes it.
Three things follow:

- **Check the allocated base against the board's own boot line**
  (`pairing: this controller's remote addresses start at ...`). The tool and the
  firmware derive it the same way from the same MAC, so they must agree, and two
  different boards printing the same value would be a bug worth reporting.
- **An address already in the table is stepped over**, so a table part imported
  and part allocated cannot collide with itself.
- **A newly allocated address means the motor does not know it yet.** Every
  shade given one has to be paired — [Pairing a shade](#pairing-a-shade) — and
  until it is, that shade will not move. Leaving the imported address instead
  keeps the shade working *and* keeps the two-controllers-one-identity problem;
  there is no third option that does neither.

It validates before writing — a sentinel address (0 or 0xFFFFFF), a name over
32 bytes, a travel time of zero, a repeated address — so a typo is refused here
rather than as a shade that silently never appears. Travel times are the
factory defaults unless measured; they are what the position estimate is
computed from, and `docs/specs/2026-08-15-position-accuracy-requirements.md` is
the argument for calibrating them.

### 1b. Or read it out of the controller you are replacing

Export a backup from that controller's web UI (**Settings → Backup**) and hand
the file over. Export it **immediately** before importing: any command the old
controller sends afterwards advances the real rolling code past the file's, and
the file is the only source this has.

```bash
cargo run -p somfy-config --example provision_shades -- --from-backup device.backup shades.bin
# read 5 shades from device.backup (backup format version 25).
#   1 room not written here — this region holds shades only.
#   1 group not written here — this region holds shades only.
#   2 'my' favourites not written here — there is no field to provision one into; ...
#
#   ShadeId(0) 'Kitchen' address ... seed rolling code 42
#   ...
#
# 3 value(s) could not be carried across as they stand:
#   !! ShadeId(1) 'Garage': shade kind 0x05 is not one this firmware models — ...
#   !! ShadeId(2) 'Terrace': the old controller drove this shade with 80-bit frames — ...
#
# wrote 2048 bytes to shades.bin
```

The same validation applies, plus four things worth reading rather than
scrolling past:

- **Every shade is imported or none is.** A backup with one bad field is refused
  whole and the message names the shade, because dropping the third shade
  renumbers the fourth and fifth (see *Order is identity*).
- **Whatever the backup held that this region cannot is counted and named** —
  rooms, groups, linked remotes, and each shade's "my" favourite. A favourite in
  particular is real behaviour lost: the motor keeps its own, but this
  controller will not know about it, so its position estimate drifts after a
  `My` press until you set one here.
- **Anything that could not be carried across is printed per shade, with `!!`,
  below the table.** A kind this firmware does not model becomes a roller. A
  shade the old controller drove with 80-bit frames, or with a radio protocol
  other than the one this firmware speaks, has nowhere to record that and **will
  be provisioned and will not respond** — check those two before you conclude
  the radio is broken.
- **If the backup's records did not all line up, it stops and asks.** A comma
  inside a shade's name shifts every field after it, which can produce a
  perfectly plausible wrong rolling code. In that case the tool prints the whole
  table and writes nothing unless you type `yes`.

The backup carries your real radio addresses and rolling codes. Treat the file
like a key: it is what a nearby attacker would need to forge commands to your
motors. Do not commit it or paste it anywhere. For the same reason the tool
**refuses to write the image over a backup** — including `--from-backup
*.backup` when the glob matches more than one file, which would otherwise put a
real backup in the output slot.

Rooms, groups and network settings are **not** imported by this step — the
region holds shades only.

### 2. Put it on the board

Step 0 above — **identify the board** — applies here too, every time.

```bash
cd crates/firmware
espflash erase-parts --port /dev/ttyUSB0 --partition-table partitions.csv shades
espflash write-bin   --port /dev/ttyUSB0 0x204000 shades.bin
```

The erase is **not optional when re-provisioning**, for the same reason as
`wificfg`: the tool writes sequence number 0, so an existing record with a
higher sequence number stays newest and the new table is ignored.

A board flashed before this region existed has no `shades` partition at all and
says so (`region unavailable (PartitionMissing)`); reflash the firmware from
this directory so espflash writes the current `partitions.csv`. `rollcode` and
`wificfg` keep their offsets, so rolling codes and credentials survive it.

### 3. Confirm it landed

```bash
espflash monitor --port /dev/ttyUSB0 --non-interactive   # reset the board
```

```
shades: partition 'shades' at 0x00204000, 4 slots of 2048 bytes
shades: survey slots=4 valid=1 blank=3 damaged=0 newest_seq=Some(0)
shades: ShadeId(0) address 0x00C0DE — entry 0
shades: 0x00C0DE had no stored rolling code; seeded 42 from the shade record. This happens once.
controller: 1 shades provisioned
```

**Reset the board again and the last line must change**, to:

```
shades: 0x00C0DE keeps its stored rolling code 42 — the provisioned starting
 value 42 is ignored, which is what every boot after the first looks like
```

That is the check that matters. A board that prints `seeded` on every boot is
walking its rolling code backwards, and every shade on it will stop responding.

| Quantity | Value |
|---|---|
| Partition | `shades`, data/undefined, 0x204000, 8 KB |
| Record | 2048 bytes: magic `RTSS`, version 1, count, seq, then 56 bytes per shade — address, seed code, kind, tilt mode, up/down/tilt times, name(32) — CRC-32 |
| Slots | 4, in 2 erase sectors of 2 |
| Capacity | 32 shades, which is the registry's own limit |
| Written by | the host tool only — the firmware has no write path for this region |

---

## Pairing a shade

Teaching a motor that this controller's remote address is one to obey. **This is
the only procedure here that transmits at a real motor and changes what it
responds to.** Read the whole section before starting one.

### What can go wrong, in one paragraph

The frame this sends is `Prog`, and `Prog` is the same frame a physical remote's
PROG button sends. On a remote, a **tap** adds a remote to the motor and a
**hold** removes one — and a controller has no button, so the *length of the
burst* is what stands in for how long the button was held. somfy-rs pins the
pairing burst to a tap (`somfy_domain::PAIR_REPEATS`) and offers no unpairing
command at all, so the dangerous case is not reachable from the button. What
*is* reachable: pairing the wrong shade, because you pressed the wrong button
while a motor was in programming mode. The motor a `Prog` frame reaches is
whichever one is listening, not whichever one the entity is named after.

### Before you start

- [ ] The shade has an address this controller allocated — see
      [The address decides whose remote this controller is](#the-address-decides-whose-remote-this-controller-is).
      Pairing a motor to an address another controller also transmits at fixes
      nothing; it re-creates the problem with an extra step.
- [ ] The board is on the broker and the shade's entities are in Home
      Assistant: a cover, and a `<name> pairing` button filed under the device's
      configuration entities rather than on the room card.
- [ ] You have a working remote for this shade — a physical wall remote, or
      another controller that still drives it. **Something has to put the motor
      into programming mode, and this controller cannot: a motor that has never
      heard of it ignores everything it sends, including a `Prog`.**
- [ ] You can see the shade from where you will press the button. The only
      confirmation this procedure has is the motor jogging; RTS is one-way and
      the controller never learns whether the motor accepted anything.
- [ ] **Ideally the shade is stationary.** A mid-range seek ends with the
      controller transmitting a `My` when its estimate says the target is
      reached, and in programming mode `My` does not stop anything — it *stores
      a favourite position* inside the motor. The controller handles this:
      pairing, and any overheard PROG press from a linked remote, drops the
      pending stop so it is never transmitted. The cost is that the shade
      carries on to its physical limit and the position estimate is wrong until
      the next move reaches a limit, which any Open or Close corrects. Pairing a
      stationary shade avoids paying that at all.

### The sequence

1. **At the shade**, press and hold the PROG button on the existing remote for
   about two seconds, until the motor **jogs** (a short up-down movement). It is
   now in programming mode, for roughly two minutes.
   - A multi-channel remote must be on this shade's channel first.
   - On most remotes PROG is a recessed button on the back, needing a pen.
2. **Within that window**, press the shade's `… pairing` button in Home
   Assistant.
3. **The motor jogs again.** That is the acknowledgement, and it is the whole
   of it — nothing appears in Home Assistant, because there is nothing for the
   controller to report.
4. Programming mode ends on its own. Wait it out, or press PROG on the existing
   remote again.
5. **Test it**: open and close the shade from Home Assistant's cover entity.

### If it does not jog

In this order, because the cheap checks come first:

- **The window expired.** Two minutes is generous but not unlimited, and a slow
  MQTT reconnect eats it. Put the motor back into programming mode and press
  again.
- **Nothing was transmitted.** The serial monitor prints a line per burst:

  ```
  radio: sent 3 frame(s), rolling_code=...
  ```

  Three is the pairing burst — one frame plus `PAIR_REPEATS`. **No line at all**
  means the command never reached the state task: check the broker, and check
  that the button published `PRESS` (this firmware matches that exactly and
  ignores anything else, deliberately — a lenient parse would let a stray
  retained message transmit `Prog` at a motor). **A count other than three**
  means something resolved the repeat policy wrongly, and that is worth stopping
  for rather than pressing again: a long `Prog` burst removes a remote.
- **The frame was transmitted and not heard.** Range or antenna — the
  [Transmit path](#transmit-path) procedure is what separates those — or the
  motor was never actually in programming mode, which on a multi-channel remote
  usually means the PROG press was made on a different channel.
- **The shade has no stored rolling code** — the serial line says
  `will refuse to transmit`. Re-provision the shade region; nothing goes on the
  air without a committed code.

### If the shade stops responding afterwards

The motor accepted the pairing and something else is wrong — almost always the
rolling code. A motor rejects any code at or below the last it accepted, so a
seed that was too low leaves a paired motor that ignores everything. Re-provision
that shade with a higher seed code, **after** erasing its stored code, or pair it
again: pairing teaches the motor whatever the transmitter is sending.

### What this does not do

- **It does not remove anything.** A motor holds several remotes at once, so
  pairing this controller leaves every existing wall remote working. That is the
  intended end state, not a compromise: `somfy-domain` tracks overheard frames
  from linked remotes precisely so a wall remote and this controller can drive
  one shade without the position estimate drifting.
- **It does not unpair.** There is no command for it here. Removing a remote
  from a motor is a hold on that motor's own PROG button, done at the shade, and
  the reason it is absent is that the cost of getting it wrong is paid there too.

---

## Controller

The real firmware: the radio task, the state task, the three flash stores and —
when credentials are present — Wi-Fi and the TCP/IP stack, wired together.
**It keys the transmitter only when commanded, and a command names a shade the
`shades` region provisioned.** So flashing it onto a board with that region
erased produces a controller that listens and tracks and puts nothing on the
433 MHz band, which is deliberate — no boot of an unprovisioned image can move
a shade — and it is also why the harnesses above still exist.

Step 0 above — **identify the board** — applies here too, every time.

```bash
source ~/export-esp.sh
cd crates/firmware
cargo build --release --features chip-s3 --target xtensa-esp32s3-none-elf
espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/firmware
espflash monitor --port /dev/ttyUSB0 --non-interactive
```

A healthy boot on a board with **no Wi-Fi credentials** — which is what a
freshly flashed one is — prints this, and nothing about it is an error:

```
stack: 176876 bytes available, 8192 required
config: partition 'wificfg' at 0x00202000, 16 slots of 512 bytes
config: survey slots=16 valid=0 blank=16 damaged=0 newest_seq=None
shades: partition 'shades' at 0x00204000, 4 slots of 2048 bytes
shades: survey slots=4 valid=0 blank=4 damaged=0 newest_seq=None
shades: none provisioned — the controller receives, decodes and tracks, and has
 nothing to command
store: partition 'rollcode' at 0x00200000, 32 slots of 256 bytes
store: survey slots=32 valid=3 blank=29 damaged=0 newest_seq=Some(2) addresses=1
controller: no shades provisioned — receiving and tracking only, and nothing can
 be commanded until a shade table is flashed. Build one with `cargo run -p
 somfy-config --example provision_shades`.
network: no credentials provisioned — running radio-only. This board still
 receives and decodes; see docs/hardware-checklist.md to provision one.
heap: controller started — 0 of 57344 bytes used, peak 0
controller: running
```

Each line is there because nothing else can establish it:

- **stack** — `RmtTx::transmit_frame` needs roughly 6.5 KB, and Embassy tasks
  have no stacks of their own, so that comes off the main stack. esp-hal's
  linker script gives the main stack whatever DRAM is left after the statics,
  which moves every time a static is added — and the Wi-Fi heap is a static, so
  this figure dropped from 304,652 to 193,980 the moment Wi-Fi arrived, and to
  176,876 when the broker session's 14,824-byte task future joined it. The
  check refuses to start rather than let a future Plan's buffers turn into a
  corrupted pulse train.
- **config survey** — same distinction as the store's: a region that has never
  held credentials versus one whose credentials are gone.
- **store survey** — "this device has never stored a code" versus "this
  device's codes are gone". `damaged` above zero on a board nobody power-cut
  deserves a look.
- **no shades provisioned** — a silent empty controller and a broken one look
  identical from the serial line, and from Home Assistant, where both announce
  availability and no entity. On a board that *does* have a table, this is
  where the per-shade `seeded`/`keeps its stored rolling code` lines appear;
  see **Shade provisioning** above for why the second boot's wording is the
  check that matters.
- **network: no credentials** — the ordinary state of a new board, said out
  loud so it is not mistaken for a failure. The radio is unaffected.
- **mqtt: no broker provisioned** — the same, one layer up, and it is a
  supported configuration rather than a fault. On a board that *does* have one,
  the first connect prints
  `mqtt: broker accepted an MQTT v5 CONNECT and answered CONNACK (Connected)`,
  which is the observation that turns "the add-on ships Mosquitto 2.x, so it
  speaks v5" into something read off a wire.
- **heap** — `peak 0` with no network is the check that the heap belongs to
  `esp-radio` alone: nothing else in the firmware allocates a byte. With Wi-Fi
  running the peak was **46,660 of 57,344**, and it is printed again whenever
  the network comes up so the margin can be watched rather than assumed.
- **controller: running** — both radio tasks spawned. Anything else names its
  own failure (`StartError`), including which pin disagreed with `chip::pins`.

### Proving the network cannot take the radio down

Spec R9 says the network is a degradable service. Five checks, in increasing
strength:

1. **No credentials at all.** The boot above. The network is never attempted;
   `controller: running` still appears.
2. **Credentials that cannot associate.** Provision `SSID_NOT_PROVISIONED` with
   `config-check`, then watch the backoff run 1 s → 60 s for several minutes.
   The controller must keep running throughout — no panic, no reset, no hang.
3. **A frame received while Wi-Fi is retrying.** The one that actually settles
   it. Fire the reference transmitter (see [Receive path](#receive-path)) while
   the board is in state 2 and confirm `state: heard … from 0x…` still appears.
   **Ten trials, not one** — the link decodes a minority of frames, so a single
   silent trial proves nothing.
4. **A broker that is not there.** Provision a real network and the
   `config-check` placeholder broker (`192.0.2.10`, a TEST-NET-1 address that
   is never routed). Wi-Fi must associate and get an address while the broker
   retry runs 1 s → 60 s, with `mqtt: reconnecting in …` and nothing else
   changing. The delay sequence is the check, exactly as it is for Wi-Fi.
5. **A broker that is killed mid-session.** Stop the Mosquitto add-on while the
   device is connected. Expect one `mqtt: session at … ended after … ms` line
   and a reconnect; the radio must be unaffected throughout, and on restart the
   entities must repopulate from the **retained** discovery configs without the
   device being touched.

Checks 1 and 2 were run on the spare ESP32-S3 on 2026-08-16/17. **Checks 3, 4
and 5 have not been run** — 3 needs a transmitter on the owner's network, and 4
and 5 need the real broker.

### What the boot prints about the heap and the stack, and what to expect

**The ESP32-S2 was dropped on 2026-08-17** and the `mqtt` cargo feature went
with it — it existed only to compile the broker session out for that one chip,
which could not hold it. There is no build of this firmware without a broker
session any more. `crates/firmware/Cargo.toml` keeps the arithmetic that
justified it.

Since then the heap is **per chip**, derived by subtracting a fixed stack
budget from the DRAM each chip has to divide, so both numbers below move
together and both are printed on every boot:

| chip | heap | main stack | required |
|---|---|---|---|
| ESP32 | 60 KiB = 61,440 | 66,908 | 49,592 |
| ESP32-S3 | 163 KiB = 166,912 | 66,788 | 49,592 |
| ESP32-C3 | 150 KiB = 153,600 | 66,856 | 49,592 |

Read on 2026-08-17 from the release ELFs; the ESP32-S3 row was confirmed on the
spare board, which printed `stack: 66788 bytes available, 49592 required` —
the ELF figure exactly. `crates/firmware/src/heap.rs` carries the derivation
and the commands that regenerate the table.

**The line worth reading is `heap: session announced`.** It is printed one line
after the burst of retained discovery configs that produces the heap's peak;
the two earlier `heap:` lines both run before that burst, so neither shows the
figure the sizing is checked against. One clean ESP32-S3 boot at 163 KiB:

```
stack: 66788 bytes available, 49592 required
heap: controller started — 46440 of 166912 bytes used, peak 46676
heap: network up — 47364 of 166912 bytes used, peak 49068
heap: session announced — 47464 of 166912 bytes used, peak 51212
```

If `heap:` ever reports a peak within a few kilobytes of the size, that is the
thing to act on; `heap: … too little DRAM for the radio …` at boot means the
heap has been squeezed below the driver's measured working set and association
is expected to end in a panic.

A fourth thing worth knowing rather than checking: **the controller reboots on
a panic; the bring-up harnesses halt.** Wi-Fi brings in panics this firmware
neither writes nor can catch (`esp-radio` panics on a status code it does not
recognise; a failed allocation reaches `handle_alloc_error`), and a halted
board is a dead radio until somebody power-cycles it. So `firmware` prints the
message, waits 100 ms for the serial line to drain, and resets — expect
`PANIC: …` followed by `rst:0x3 (RTC_SW_SYS_RST)`. A deterministic panic
therefore shows up as a reboot loop with a readable message, which is the
intended trade. `tx-check`, `store-check` and `config-check` still freeze,
because there a person is watching and the frozen state is worth more.

After that the only output is what the radio hears:

```
state: heard Up from 0x0FC0D5 (code 4213)
```

one line per frame, so a single button press on a wall remote produces one line
for its first frame and one for each repeat. The state task prints it, not the
radio task, on purpose: a repeat frame follows the previous one by about 5 ms
after the receiver has finished with it, and a serial line at 115200 baud would
eat most of that window before the receiver could be re-armed.

Frames from addresses the controller does not know are printed and then
dropped by the domain, which is what makes this readable at all with an empty
registry.

### What this procedure does not establish

Everything about reception on the air. The RF link between the two boards
decoded ~4–12% of frames during transmit bring-up, and a receiver validated
against a marginal link is exactly how this project already produced one
confident wrong diagnosis. Fixing the link and validating reception — decoded
address, command, `bits`, and the sync counts — belongs to on-air bring-up, not
here.
