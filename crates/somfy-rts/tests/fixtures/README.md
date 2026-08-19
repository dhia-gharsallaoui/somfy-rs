# Pulse fixtures

Four `.pulses` files, none of which is a raw capture, and the difference between
them matters:

| File | Timing | Payload |
|---|---|---|
| `anonymised_up_56bit_1.pulses` | measured, from a physical Somfy wall remote | substituted by this project |
| `anonymised_down_56bit_1.pulses` | measured, same remote | substituted |
| `anonymised_my_56bit_1.pulses` | measured, same remote | substituted |
| `synthetic_up_56bit.pulses` | this crate's own nominal constants | this crate's own |

The first three exist because the pulse train a real remote emits is **not** the
pulse train this crate emits, and a receiver has to cope with the former. The
last exists so the loader itself is exercised on every CI run without depending
on the other three.

> **Do not hand-edit any of them.** The three anonymised files cannot be
> regenerated: the captures they came from were destroyed deliberately (see
> below), so a wrong keystroke is not recoverable from anything. The synthetic
> one is reproducible from `../golden.rs`, and should be regenerated rather than
> edited.

---

## The anonymisation, 2026-08-19

### What was wrong with the originals

The three captures were durations-only text with no address in plain text —
**but the pulse train is the frame.** Run one through this project's own
`RxDecoder` and `decode56` and out comes the transmitting remote's 24-bit
address and its rolling code at the moment of capture. That is the same class of
secret `../../../somfy-migrate/tests/fixtures/` treats as private, and this
repository is public.

Sized honestly, because overstating it would be its own error: RTS is 433 MHz
and one-way, so that address is recoverable by anyone within radio range with an
SDR costing about the price of a meal. Publishing did not create the exposure;
it removed the need to stand near the house first. And the address is the
durable half — a receiver accepts codes ahead of its stored value, so the
captured rolling codes were never the point.

### Why they were not simply deleted

Deletion was a real option and it was the wrong one, for a reason that is
specific rather than sentimental:

`somfy_rts::MEASURED_MAX_INTRA_FRAME_SEGMENT_US` is **measured from these
files**. `../measured.rs` re-derives it from them on every test run and asserts
it with `==`, and `somfy-rmt` sizes its RMT idle threshold against it in a
compile-time assertion. Delete the evidence and a shipping firmware constant
becomes a number nobody can account for — which the project's own working rules
class as fabricated.

The finding that constant exists for is also not reproducible synthetically.
`somfy-rmt/tests/idle_threshold.rs` shows that the design spec's original
12,000 µs threshold would have split **every** real first frame in two, because
a real remote's post-wake-up silence is ~17.7 ms where this crate's
`TIMINGS::WAKEUP_LOW` says 7357 µs. Rendered pulses cannot demonstrate that;
only a measurement can.

### The method, exactly

Run by `cargo run -p xtask -- anonymise-capture`, whose source
(`xtask/src/anonymise.rs`) is the authority and carries the reasoning at length.
Per file:

1. **Parse** the durations and reconstruct levels by alternation from HIGH.
   A glitch entry (< 448 µs) is *refused* rather than filtered — none of the
   three contained one, and one appearing would break step 3's model.
2. **Split** at the software-sync HIGH. Everything up to and including it — the
   wake-up HIGH, the silence after it, four hardware-sync halves, the sync
   itself: seven segments — is the **preamble**, and is copied to the output
   byte for byte. Everything after it is the **body**.
3. **Model the body as half-symbols.** Each body segment spans one or two
   nominal 640 µs halves (`round(d / 640)`), and they sum to exactly 112: the
   software sync's LOW tail, then 56 bits of two halves each, *less* the final
   bit's second half — which no capture contains, because the receive ISR stops
   recording the moment the last bit is stored. The tool asserts that total; a
   file that does not have it is refused.
4. **Decode** the body through the shipping `RxDecoder` and `decode56`. Keep the
   **key byte** and the **command**. Discard the address and the rolling code —
   they are never returned, printed or written.
5. **Re-encode** through the shipping `encode56` with a substituted address and
   rolling code (below), which recomputes the checksum and the obfuscation
   chain.
6. **Measure the jitter.** The pool is the deviation from 640 µs of each of the
   body's **single**-half segments, in the order they were measured. Merged
   segments contribute nothing: only their pair *sum* was ever observed, and
   splitting it between the two halves would be an assumption. That costs
   nothing measurable — on all three files the merged pair-sums span the same
   range as the singles.
7. **Re-render.** The new bits give 112 half-symbol levels; half *i* is given
   `640 + pool[i mod pool.len()]` µs; adjacent same-level halves are merged into
   the edge-to-edge form a `CHANGE`-interrupt receiver produces.
8. **Verify** by reading the output back through the same decoder and checking
   it yields the frame the header claims, before the operator is told anything
   worked.

### Why the deviations are re-ordered rather than kept at their own index

This is the part that is easy to get wrong, and getting it wrong would have
anonymised nothing. It was tested rather than reasoned about.

The obvious method — keep each half-symbol's own deviation at its own position
and let the new bit pattern re-merge them — **puts the original payload back into
the file.** A merged segment is two halves whose *sum* alone was observed, so
keeping position means splitting that sum, and the two halves then differ by at
most one microsecond. Wherever the new bit pattern separates them, the output
carries two adjacent segments with near-identical deviations, and those twins are
a map of which halves used to be merged — which is the sequence of bit
transitions, which is the original 56 bits, which is the address.

**Measured on the `up` capture, both ways, with an attacker who has only the
anonymised file:** find every adjacent pair of single-half segments whose
deviations differ by ≤ 1 µs and call it a formerly-merged pair.

| Method | original merged pairs | recovered | false positives |
|---|---|---|---|
| position-preserving, even split | 30 | **13** | **0** |
| pool cycled (what shipped) | 30 | 0 | 0 |

Thirteen of thirty at **100% precision** — the attack is never wrong when it
fires. Recall is only 43% because the other seventeen pairs happen to stay merged
under the substituted payload too, so this particular attack cannot see them; an
attacker still walks away with thirteen certain bit-transition positions and a
search space collapsed far below the 2⁴⁰ the address and rolling code should
cost. That is not anonymisation.

Cycling a pool leaves nothing of the kind: a multiset has no positions in it, so
the attack finds no twins at all.

### What is real and what is not, per file

Real, in all three:

- the seven preamble durations, verbatim as measured;
- the key byte, which is `0xA0 | n` for a counter the remote increments per
  press and which names no remote;
- the command;
- every half-symbol deviation used in the body — each is a number this same
  capture produced.

| File | pool | deviation range | pulses (was) | body µs (was) |
|---|---|---|---|---|
| `anonymised_up_56bit_1.pulses` | 52 | −46 … +57 µs | 92 (89) | 72,027 (71,996) |
| `anonymised_my_56bit_1.pulses` | 42 | −26 … +45 µs | 77 (84) | 72,002 (72,006) |
| `anonymised_down_56bit_1.pulses` | 68 | −46 … +51 µs | 98 (97) | 71,929 (71,987) |

Synthetic, in all three:

- **address `0x00C0DE`** — this project's bring-up value, used by the synthetic
  fixture and by the firmware's own bring-up paths. Chosen because it is
  transparently not a remote's: it reads as "code" in hex and it is the value
  every other test in this workspace already uses, so no second fake constant
  had to be introduced;
- **rolling codes 1, 2 and 3**, assigned in the order the buttons were pressed
  (up, then my, then down — recovered from the originals, whose codes were
  consecutive). Small and consecutive so they are obviously assigned rather than
  observed;
- and therefore the checksum, the obfuscation chain, the 56 bits, and the
  merged-segment structure that follows from them — which is why the pulse
  counts moved.

The **captured order is preserved as evidence in its own right**: because the key
byte was kept and the three files came from three consecutive presses, the key
still increments once per file, and `../golden.rs` asserts it. Note that this
contradicts `somfy_rts::RollingCode`, which derives the key from the rolling
code's low nibble; the real remote ran two counters in lockstep at a constant
offset instead. Recorded in `docs/provenance.md`.

### What was lost, plainly

**These files can no longer show that our checksum and de-obfuscation agree with
Somfy's.** The bits are this project's encoder's now, so that particular claim —
which was the strongest thing a real capture had to offer — is gone, and a
previous version of this document was right to warn that re-encoding would cost
it.

What replaced it is weaker but not nothing: the bytes were **frozen** when the
files were written, so a later change to `deobfuscate`, to `checksum` or to the
bit order stops them decoding. All three were confirmed by breaking them. That
is regression cover over the *decode* path, not evidence that the path is
correct.

Be precise about where that cover ends: no test that reads these files calls
`encode56`, so a change to `obfuscate` **alone** leaves every one of them
passing — confirmed by reversing its direction, which broke only the synthetic
fixture's in-memory comparison. That is one of two reasons
`synthetic_up_56bit.pulses` is not redundant.

Also lost: the original merged-segment structure, and with it the pulse counts
(89 / 84 / 97). Those counts were cited as a reference for "what a whole frame
looks like on the air" in `docs/hardware-checklist.md` and `docs/provenance.md`;
both now carry the new figures with the old ones beside them.

### Do they still bite? — checked by breaking things, 2026-08-19

A fixture that cannot fail is decoration, so the decoder was deliberately
broken four ways and the results recorded:

| Break | Caught by the three anonymised files? | Caught by the synthetic one? |
|---|---|---|
| `RxDecoder::within` narrowed from ±25% to ±2% | **yes**, all four capture tests, plus both `somfy-rmt` idle-threshold tests | **no** — all three synthetic tests still passed |
| `MEASURED_MAX_INTRA_FRAME_SEGMENT_US` moved 17,738 → 17,800 | **yes**, `measured.rs` | n/a — it is excluded from that derivation on purpose |
| `deobfuscate` direction reversed | **yes**, five of seven golden tests | yes |
| `obfuscate` direction reversed | **no** | **yes** — nothing on the capture path encodes |

The first row is the one that justifies keeping these files. A tolerance window
is only exercised by durations that are *not* nominal, and this crate's renderer
emits nothing but nominal durations — so the synthetic fixture is structurally
incapable of noticing that the receiver stopped tolerating a real transmitter's
jitter. The last row is the mirror image and is why the synthetic fixture stays.

### Residual exposure

- **Git history still holds the originals.** A working-tree change does not
  remove a blob. `docs/pre-public-checklist.md` item 1 carries the rewrite
  procedure, for the owner to run or authorise.
- **The pool size leaks the original's edge count.** `pool.len()` is published in
  the table above, and from it the original's number of merged segments and
  therefore its number of bit transitions follow — roughly three bits of
  information about a 40-bit secret. It is recorded rather than pretended away.
- **The preamble is a fingerprint of the transmitter, not of the remote.** The
  wake-up and sync durations of a given Somfy remote model are as alike as its
  oscillators; they identify no unit.

### How to anonymise a future capture

```
cargo run -p xtask -- anonymise-capture \
    --in  path/to/raw.pulses \
    --out crates/somfy-rts/tests/fixtures/anonymised_<cmd>_56bit_<n>.pulses \
    --address 0x00C0DE --rolling-code <n> --captured YYYY-MM-DD
```

The raw capture must not be committed at any point, including in a branch that
is later rewritten. Stage it outside the repository, run the tool, delete it.

---

## What a capture actually records (read before capturing)

Source: the ESPSomfy-RTS C++ firmware, this project's reference device. Its RX
struct `somfy_rx_t` (`src/Somfy.h:95-116`) stores raw pulse **durations** in
`pulses[MAX_TIMINGS]` — `unsigned int pulses[]`, one entry per edge, **no level
bit**. Everything below follows from that fact.

The receive ISR is `Transceiver::handleReceive` (`src/Somfy.cpp:4384-4516`).
Two behaviours change how a raw `pulses[]` dump must be interpreted:

1. **Durations only, no levels.** Each `pulses[]` entry is
   `micros() - last_time`, the time since the previous recorded edge. The level
   of each segment is *not* stored. Because the receiver runs off a `CHANGE`
   interrupt, the physical line strictly alternates HIGH/LOW at every recorded
   edge, so levels can be reconstructed by **alternation from a known phase**.
   The first pulse of a transmission is the wake-up / first hardware-sync pulse,
   which is **HIGH** (`render_pulses`, `pulse.rs`), so:

   ```
   pulse[0] = HIGH, pulse[1] = LOW, pulse[2] = HIGH, ...
   ```

   The loader in `../support/mod.rs` performs exactly this reconstruction when it
   is given a durations-only file.

2. **Sub-448µs glitches are logged but ignored.** On a segment shorter than
   `bitMin` (`SYMBOL * TOLERANCE_MIN = 640 * 0.7 = 448µs`), the ISR does this
   (`Somfy.cpp:4388-4395`):

   ```c
   if (duration < bitMin) {
       if (pulseCount < MAX_TIMINGS && cpt_synchro_hw > 0)
           pulses[pulseCount++] = duration;   // logged into rx.pulses[]
       return;                                // <-- last_time NOT advanced
   }
   last_time = time;                          // only reached for real edges
   ```

   So a glitch **appears in `pulses[]`** even though the decoder skips it, and
   `last_time` is left untouched — meaning the *next* real duration is measured
   from before the glitch and already spans it. The loader therefore just
   **drops** every sub-448µs entry; it does **not** merge the glitch duration
   into the following pulse (the C++ already did that implicitly). Dropping
   also happens *before* level reconstruction, so the surviving edges keep their
   strict alternation and the HIGH/LOW phase stays correct.

## Capture procedure

> **Do not use `printBuffer`.** The `transceiver_config_t::printBuffer` flag is
> vestigial in v2.5.6 — its only assignment sits inside the commented-out block
> that closes at `Somfy.cpp:4766`, so it can never be set and nothing reads it.
> An earlier version of this document recommended it; that advice was wrong.

The working route needs **no firmware modification**. The device already emits
every decoded frame, with its full raw pulse array, over its websocket:

1. Connect a websocket client to `ws://<device>:8080` and send the text frame
   `join:0` to join `ROOM_EMIT_FRAME` (`Sockets.cpp:166-171`). `emitFrame` is
   gated on that room having at least one client (`Somfy.cpp:4603`).
2. Press a paired remote button near the device. Each decoded frame arrives as
   `42[remoteFrame,{...}]` — socket.io-like, but note the event name is emitted
   **unquoted** (`WResp.cpp:8`), so the message is not strict JSON and needs
   splitting at the first comma before the payload can be parsed.
3. The payload's `pulses` array is a verbatim `rx.pulses[]` dump; write one
   entry per line. It also carries `command`, `bits`, `sync`, `rssi`, `address`
   and `rcode`, so captures can be sorted by button automatically and each
   file's expected decode is known without guesswork.
4. Either supported format is accepted (see below); durations-only is the
   natural dump of `rx.pulses[]`.
5. Do **not** hand-edit durations. If a capture will not decode, capture again —
   the file is the authority, the code bends to it.
6. Then anonymise it before it goes anywhere near a commit.

## Supported file formats

The loader (`load_fixture` / `parse_pulses` in `../support/mod.rs`) accepts both:

- **Durations-only** — `<duration_us>` per line. Levels are reconstructed by
  alternation from HIGH. This is the direct dump of `rx.pulses[]`, and the format
  every committed file uses.

  ```
  2560
  2560
  4850
  1280
  ...
  ```

- **Level + duration** — `<level 0|1>,<duration_us>` per line. Levels are taken
  as written (no reconstruction). Useful when a capture tool already emits
  levels, or for hand-authored regression cases.

  ```
  1,2560
  0,2560
  1,4850
  ...
  ```

Common rules for both formats:

- Lines that are blank or start with `#` are skipped (comments).
- Any entry shorter than 448µs is dropped as a glitch, regardless of format.
- A file must be a single format; do not mix commas and bare durations.

## File naming

`anonymised_<command>_<bits>bit_<n>.pulses` for a file derived from a capture,
`synthetic_<command>_<bits>bit.pulses` for one rendered by this crate. `<n>`
disambiguates multiple captures of the same command. The prefix is not
decoration: the three anonymised files replaced identically-named
`<command>_<bits>bit_<n>.pulses` files on 2026-08-19, and a reader who finds the
old name in the history should see immediately that the content is not the same
thing. Each file's expected decode is asserted in `../golden.rs`.

## The capture session, for the record

Captured 2026-08-15 from a **physical Somfy wall remote** received by an
ESP32-S3 + CC1101 running ESPSomfy-RTS v2.5.6, via its `remoteFrame` websocket
event (`Somfy.cpp:4602-4625`). All three tests in `../golden.rs` passed
unmodified on the first attempt — the engine decoded genuine Somfy hardware with
no code changes, which is the strongest single result this project has and is
now a historical statement rather than one CI re-checks.

The session also independently confirmed the sync model: first frames reported
`hwsync == 4` and repeat frames `hwsync == 14`, exactly the 2-and-7
hardware-sync counts `pulse.rs` renders and `rx.rs` expects. The four
hardware-sync segments of a first frame are in the preamble, so the anonymised
files still carry them, and `../golden.rs` asserts the count.

`*_80bit_1.pulses` remains **deferred** — no 80-bit-capable remote is available.

## Synthetic fixtures (checked in, no hardware)

`synthetic_up_56bit.pulses` is **not** derived from a capture at all. It is
generated deterministically from the crate's own transmitter so the loader
itself (parsing, level reconstruction, glitch filtering, end-to-end decode) is
validated on every CI run. It carries a `# SYNTHETIC` header so it is never
mistaken for anything else.

Recipe (mirrored by `synthetic_up_pulses` in `../golden.rs`):

1. `render_pulses(&encode56(&f), FrameKind::Repeat, &mut out)` for
   `f = { key: 0xA7, command: Up, rolling_code: 0x000A, address: 0x00C0DE }`.
2. Merge adjacent same-level half-symbols into edge-to-edge segments — the shape
   a `CHANGE`-interrupt receiver produces. The merged stream strictly alternates
   from HIGH, matching a real `rx.pulses[]` dump.
3. Write one duration per line, durations-only, and inject one sub-448µs line so
   the checked-in file also exercises the glitch-drop path.

It is a *repeat* frame (seven hardware syncs, fourteen segments) where the
anonymised files are *first* frames (two, four segments), which is why
`../golden.rs` asserts the hardware-sync count only on the latter.
