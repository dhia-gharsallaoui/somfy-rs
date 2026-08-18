# Going public: what has to be true first

Written 2026-08-18, when publishing stopped being hypothetical — OTA from
GitHub releases needs a public repository, so this is now on the critical path
rather than filed under "eventually".

Each item below is either **done**, or a **decision the owner has to make**.
Nothing here is a matter of tidying.

## 1. A real remote's address is in the git history — DECISION NEEDED

The three committed captures under `crates/somfy-rts/tests/fixtures/` are
durations-only and contain no address in plain text. **They still encode one.**
Audited 2026-08-18 by running them through this project's own decoder:

- All three decode to a **single real address**, which is neither the synthetic
  bring-up address `0x00C0DE` nor any shade in this estate. It is the **wall
  remote's own address**, which is what a remote transmits.
- The **rolling codes at capture time** come out with it.

They are also load-bearing. They are the only thing pinning the decoder against
a real Somfy remote's actual timing rather than against our own encoder, and
this project's verification rule exists because a transmitter reporting its own
success proves nothing.

### How large is the risk, honestly

Smaller than "a secret is published", larger than nothing.

- RTS is 433 MHz and one-way. The address is recoverable **by anyone within
  radio range** with an SDR costing about the price of a meal. Publishing does
  not create the exposure; it removes the need to stand near the house first.
- **The address is the durable part, not the rolling code.** A receiver accepts
  codes ahead of its stored value, so an attacker who knows the address does not
  need the captured code.
- The attack still requires **physical proximity**. It is a neighbour-or-passer-by
  threat, not an internet threat.

### The options, with what each costs

| Option | Cost | Leaves history clean? |
|---|---|---|
| **Re-capture from an unpaired spare remote**, then rewrite history | A spare remote, one capture session | Yes |
| **Drop the real captures**, keep the synthetic one | Loses the only pin against real hardware timing — the thing the fixtures exist for | Yes, after a rewrite |
| **Publish as-is** | The exposure above, permanently | No |
| **Fresh repository, no history** | Loses the development record, which is unusually rich here | Yes |

**A working-tree deletion is not sufficient.** The blobs stay reachable in
history; removing them needs `git filter-repo` or a fresh repository.

## 2. Real addresses in prose — DONE, but only in the working tree

Three documents carried a real shade address in plain text and were redacted on
2026-08-17. `README.md` carried one too, in a receive log, and was redacted on
2026-08-18 when it was rewritten.

**The history still holds all of them.** Same remedy as item 1, and the same
rewrite would cover both.

## 3. `LICENSE` — DONE (2026-08-18)

The workspace declared `GPL-3.0-only` and shipped **no licence file**. This is
exactly the defect for which three candidate dependencies were rejected during
this project (`mcutie`, `esp-hal-mdns`, `embassy-ha` — a declared licence with
no LICENSE file, which GitHub reports as no licence at all).

Verbatim GPL-3.0 text is now at `LICENSE`.

## 4. The private backup — SAFE, verified

`crates/somfy-migrate/tests/fixtures/real_device.backup` contains a real user's
device data: radio addresses and rolling codes for the whole estate. It is
gitignored and **has never been committed** — verified 2026-08-18 with
`git rev-list --all --objects`, which returns zero objects referencing it.

Tests that use it skip when it is absent, so a public clone still passes.

## 5. Secrets — SAFE, by construction

No SSID, passphrase, broker password or MAC appears in the repository. Wi-Fi and
MQTT credentials live only in flash, are entered at provisioning time, and the
settings API is **structurally write-only** for them: no outbound type has a
field a secret could be written into, asserted both at byte level and against
the generated TypeScript.

## 6. Before the first release

- Decide item 1.
- A public repository is what OTA-from-releases needs; the manifest and the
  `xtask` that publishes it are Plan 6 Task 6, still unbuilt.
- Consider whether the issue tracker should say that **this firmware transmits
  at real motors** and that a mistake there costs a walk to the shade — the
  hardware checklist says it, and a newcomer reads the README first.
