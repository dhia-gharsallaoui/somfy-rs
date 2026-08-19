# Going public: what has to be true first

Written 2026-08-18, when publishing stopped being hypothetical — OTA from
GitHub releases needs a public repository, so this is now on the critical path
rather than filed under "eventually".

Each item below is either **done**, or a **decision the owner has to make**.
Nothing here is a matter of tidying.

## 1. A real remote's address is in the git history — WORKING TREE DONE
##    2026-08-19; HISTORY STILL OUTSTANDING

### Outcome

**A fifth option was taken, which the table below did not list: the captures
were anonymised in place — payload substituted, measured timing kept.** The
three files are now `anonymised_{up,down,my}_56bit_1.pulses`; the originals were
deleted from the working tree and are gone from every machine but this
repository's history.

Why not the two options this document favoured:

- **Re-capturing from a spare remote** needs a second remote and a second radio,
  and neither exists. It remains the best answer if one ever does.
- **Deleting them** would have deleted the *derivation* of
  `somfy_rts::MEASURED_MAX_INTRA_FRAME_SEGMENT_US`, which `somfy-rts/tests/measured.rs`
  re-derives from these files with `==` on every run and which `somfy-rmt` uses
  in a compile-time assertion to size its RMT idle threshold. A shipping
  firmware constant would have become a number with no evidence behind it, which
  this project's own working rules treat as fabricated. The finding that
  constant exists for — that the design spec's original 12,000 µs threshold
  splits every real first frame, because a real remote's post-wake-up gap is
  ~17.7 ms against a transmit constant of 7357 µs — is not reproducible from
  rendered pulses at all.

**What is real in the new files:** the seven preamble durations verbatim (the
wake-up pulse, the gap, four hardware-sync halves, the software sync), the key
byte, the command, and every half-symbol deviation from nominal — each one a
number that same capture produced. **What is synthetic:** the address
(`0x00C0DE`), the rolling codes (1, 2, 3), and therefore the checksum, the
obfuscation chain, the 56 bits and the merged-segment structure.

**What was lost, and it is not nothing:** these files can no longer show that
this project's checksum and de-obfuscation agree with Somfy's. The bits are our
encoder's now. They still catch a *change* to the decode path — confirmed by
breaking `deobfuscate`, `checksum` and the decoder's tolerance window — but that
is regression cover, not interoperability evidence.

The method, the residual leakage, and the four break-it experiments that show
the fixtures still bite are in `crates/somfy-rts/tests/fixtures/README.md`. The
tool is `cargo run -p xtask -- anonymise-capture`.

### What remains: the history

**The originals are still reachable in this repository's history**, in commit
`244c93e` (2026-08-15), which is 150 commits behind `main`. Removing them needs
a rewrite the owner has not authorised, and **nothing in this task touched the
remote**. The procedure is below.

### The history-rewrite procedure, for the owner to run or authorise

`git-filter-repo` is **not installed on this machine** — `git filter-repo`
reports "not a git command". Install it first (`pipx install git-filter-repo`,
or your distribution's package). Do not use `filter-branch`: it is orders of
magnitude slower and its own documentation recommends against it.

**Step 0 — take a backup that is not this repository.**

```sh
git clone --mirror https://github.com/dhia-gharsallaoui/somfy-rs.git ~/somfy-rs-backup.git
```

**Step 1 — write the replacement list, and never commit it.** The literals go
in a scratch file rather than into this document, **which is itself published**:

```sh
cat > /tmp/somfy-redactions.txt <<'EOF'
<the wall remote's address>==>REDACTED
<the shade address redacted from docs/ on 2026-08-17>==>REDACTED
<the address redacted from README.md on 2026-08-18>==>REDACTED
EOF
```

Where to get each, without any of them being written down here:

- The **wall remote's** address, decimal, appeared in plain text in the
  pre-publication warning at the foot of
  `crates/somfy-rts/tests/fixtures/README.md` from `244c93e` until 2026-08-19.
  `git show 244c93e:crates/somfy-rts/tests/fixtures/README.md | grep -i address`
  prints it.
- The **shade** address is in the diff of `da26e93` — `git show da26e93` — which
  redacted it from `docs/plans/2026-08-15-…`, `docs/plans/2026-08-17-…` and
  `docs/provenance.md`.
- The third is in the `README.md` rewrite of 2026-08-18, in the receive log it
  removed.

Search each for other spellings before running: an address written as decimal in
one file may be hex in another, so add both forms to the list.

**Step 2 — rewrite, in a fresh clone.** One pass does both jobs: drop the three
capture blobs by path, and scrub the literals everywhere else.

```sh
git clone https://github.com/dhia-gharsallaoui/somfy-rs.git /tmp/somfy-rewrite
cd /tmp/somfy-rewrite
git filter-repo \
    --invert-paths \
    --path crates/somfy-rts/tests/fixtures/up_56bit_1.pulses \
    --path crates/somfy-rts/tests/fixtures/down_56bit_1.pulses \
    --path crates/somfy-rts/tests/fixtures/my_56bit_1.pulses \
    --replace-text /tmp/somfy-redactions.txt
```

The three paths were added in one commit and never modified, so their removal is
clean. `--invert-paths` with `--path` removes exactly those and keeps
everything else.

**Step 3 — verify before pushing anything.**

```sh
# For each literal in the list above — every one must print 0:
git log --all --oneline -S"$ADDRESS" | wc -l
# Only anonymised_* may appear:
git rev-list --all --objects | grep '_56bit_1.pulses'
# And the rewritten tree must still pass:
cargo test --workspace
```

**Step 4 — push.** `filter-repo` deletes the `origin` remote on purpose, so it
has to be re-added:

```sh
git remote add origin https://github.com/dhia-gharsallaoui/somfy-rs.git
git push --force --all
git push --force --tags
```

### The consequences, stated plainly

- **Every commit SHA from `244c93e` onward changes** — 150 commits, which is
  most of the project. Any link, issue reference or note that names a SHA breaks.
- **Every existing clone becomes incompatible.** Anyone who has one must
  re-clone; pulling will produce a divergent history. This includes the agent
  worktrees under `.claude/worktrees/`, which should be removed first.
- **`git push --force` on GitHub does not delete the old objects.** They stay
  reachable by SHA — `https://github.com/…/commit/<old-sha>` and the raw blob
  URLs keep working — until GitHub's garbage collector runs, which it does not
  do on request. The only reliable removals are asking GitHub Support to run
  `gc` on the repository, or **deleting the repository and recreating it** from
  the rewritten history. For a repository with no issues, no stars and no forks
  yet, delete-and-recreate is the cheaper and more certain of the two.
- **The exposure has already happened.** This repository is public *now*, so the
  originals have been fetchable by anyone who cloned it. A rewrite stops future
  discovery; it does not un-publish. The threat model in the next section is
  what makes that tolerable rather than urgent — but it is the reason not to
  describe a rewrite as "fixing" this.

## 1a. The exposure, as originally assessed — kept for the record

Written 2026-08-18, before the decision above. Left unedited, including the
options table that missed the option actually taken: a record of what was
considered is worth more than a tidy one.

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

> Still true, and still outstanding — see the procedure in item 1. What changed
> on 2026-08-19 is only that the working tree no longer carries them.

## 2. Real addresses in prose — DONE, but only in the working tree

Three documents carried a real shade address in plain text and were redacted on
2026-08-17. `README.md` carried one too, in a receive log, and was redacted on
2026-08-18 when it was rewritten.

**The history still holds all of them.** Same remedy as item 1, and item 1's
procedure covers both: its `--replace-text` pass is where these literals go.
That is why the pass exists at all — the fixtures themselves need only a path
removal.

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

- Item 1's working-tree half is done. **Run or decline its history rewrite** —
  and note that declining is a defensible answer given that the repository is
  already public, so long as it is a decision rather than an omission.
- A public repository is what OTA-from-releases needs; the manifest and the
  `xtask` that publishes it are Plan 6 Task 6, still unbuilt.
- Consider whether the issue tracker should say that **this firmware transmits
  at real motors** and that a mistake there costs a walk to the shade — the
  hardware checklist says it, and a newcomer reads the README first.
