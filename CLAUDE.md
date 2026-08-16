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
| `somfy-cc1101` (radio driver) | [`cc1101`](https://crates.io/crates/cc1101) 0.1.3, [`cc1101-embassy`](https://crates.io/crates/cc1101-embassy) 0.1.0 | **Own — but this was never consciously evaluated, which was a process failure.** The high-level `cc1101` API is packet-oriented (sync words, address filtering, packet length); this project runs the chip in asynchronous-serial OOK where every one of those is switched off, so we would be using its `lowlevel` raw-register module and writing the same bytes by hand anyway. Revisit if that crate grows async-serial support. |

## Source comments must not name the reference implementation

See [`docs/provenance.md`](docs/provenance.md). Deriving from the C++ reference
is **required** — never invent protocol behaviour. Citing it *in source
comments* is what is forbidden; the citation belongs in `provenance.md`.
`crates/somfy-migrate/**` is the one documented exception, because that crate's
subject matter *is* the C++ backup format.

Do not read the reference to hunt for implementation ideas without a reason —
agents have exhausted their context window in it. Do read it, or ask for the
relevant extract, when a value would otherwise be invented.

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
