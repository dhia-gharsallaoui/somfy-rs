# somfy-rs — Rust Rewrite of ESPSomfy-RTS: Design Specification

**Date:** 2026-07-15
**Status:** Approved (brainstorming complete)
**Reference implementation:** ESPSomfy-RTS (this repository, C++ / Arduino, `src/`)

## 1. Goal & Scope

### 1.1 Goal

A ground-up Rust firmware for controlling Somfy RTS shades from ESP32-class
hardware, intended as:

1. **A daily-driver replacement** for the author's own installation
   (ESP32-S3-DevKitC-1-N8R8 + CC1101 433 MHz module).
2. **A community-adoptable project**: supports the ESP32 chip family across both
   its instruction sets (ESP32-S3, ESP32-C3) so existing ESPSomfy-RTS users can
   migrate.

   **This list has shrunk twice, and the criterion each time was the same:
   whether the claim could be backed.** The ESP32-S2 went on 2026-08-17 — too
   little DRAM to hold the Wi-Fi driver's heap and a bootable stack at once. The
   ESP32 went on 2026-08-18: it could not hold the web server at all, and in its
   only buildable configuration its Wi-Fi heap cleared the measured announcement
   peak by 1,700 bytes, which is inside that peak's own ~2,000-byte boot-to-boot
   spread. **Neither chip had ever booted this firmware**, so both were listed
   support rather than demonstrated support, and both were withdrawn rather than
   maintained. `docs/provenance.md` carries both sets of arithmetic.

   The ESP32-C3 remains, with `mdns` and `sntp` refused on it for the same kind
   of measured reason — so it is reached by IP rather than by name and has no
   wall clock. It is still build-only: no C3 has been booted either, and what it
   earns its place with is the RISC-V code path, which has caught a fault an
   Xtensa-only matrix would have shipped.

### 1.2 v1.0 scope (must ship)

- Somfy **RTS protocol**, 56-bit and 80-bit frames (all commands in the C++
  `somfy_commands` enum that apply to RTS, including StepUp/StepDown,
  Favorite, Stop extensions).
- Up to **32 shades, 16 groups, 16 rooms**, 7 linked remotes per shade
  (same limits as C++).
- Shade types: roller, blind, shutter, awning, left/right/center drapery,
  with tilt modes (none, tiltmotor, integrated, tiltonly, euromode).
- **Position & tilt dead-reckoning** from configured travel times, including
  go-to-position and "my" favorite.
- **WiFi** (station + captive-portal setup AP fallback).
- **Web UI** (new, Preact) served from firmware flash.
- **MQTT + Home Assistant discovery** (cover entities with position & tilt).
- **OTA**: GitHub-release-based updates and manual upload, with A/B
  partitions and automatic rollback.
- **Migration**: import configuration backups produced by C++ ESPSomfy-RTS
  (shades, addresses, rolling codes, groups, rooms, MQTT settings).
- mDNS (`hostname.local`), SNTP, optional password auth.

### 1.3 Explicitly deferred (post-1.0)

- RTW / RTV protocol variants (frequency stays configurable so these only
  need settings + command mapping later).
- GPIO relay-driven "wired motor" support and the shade types that depend on
  it (garage, gate, dry-contact variants).
- Wired Ethernet (LAN8720 et al.). The network layer is trait-abstracted so
  this can be added without touching services.
- SSDP discovery, repeaters, sun/wind sensor handling.
- Automated hardware-in-the-loop testing.

### 1.4 Non-goals

- API compatibility with the C++ firmware's REST/WebSocket surface (the UI is
  new; the API is designed clean).
- IO HomeControl support.

## 2. Architecture Decision

**Chosen: pure-Rust `no_std` firmware on `esp-hal` (1.0+) + Embassy.**

Alternatives considered:

| Option | Verdict |
|---|---|
| **A. esp-hal + Embassy (no_std)** | **Chosen.** Vendor-backed, pure Rust, RMT-based radio timing, fast C-free builds, where the ecosystem is heading. Accepted risk: `esp-radio` (WiFi) is pre-1.0 with API churn; it is Espressif's declared next stabilization target. |
| B. std on ESP-IDF (`esp-idf-svc`) | Lower infrastructure risk, but Rust-glue-over-C, slow embuild CI, and a dead end for a pure-Rust project. |
| C. Dual shell (B now, A later) | Safest but two shells to build and maintain; rejected as a tax the layering below makes unnecessary. |

The workspace layering (Section 3) keeps option B available as a fallback
shell if `esp-radio` becomes a blocker: only the `firmware` crate would be
replaced.

## 3. Workspace Layout

Cargo workspace with strict dependency direction; everything below
`firmware` is `no_std`, hardware-free, and host-testable.

```
somfy-rs/
├── crates/
│   ├── somfy-rts/        # Protocol engine (no_std; heapless only)
│   ├── somfy-domain/     # Shades/groups/rooms + position engine (no_std)
│   ├── somfy-api/        # REST/WS/MQTT DTOs (serde; compiles on host too)
│   ├── somfy-migrate/    # C++ ESPSomfy-RTS backup parser
│   └── firmware/         # Only hardware-aware crate (esp-hal + Embassy)
├── ui/                   # Preact + Vite + TypeScript app
└── xtask/                # Build glue: UI build, asset gzip/embed, flash, release
```

### 3.1 `somfy-rts` — protocol engine

- Frame model: 56-bit and 80-bit RTS frames; encode/decode with the RTS
  obfuscation (XOR chaining) and checksum.
- Rolling-code state machine (increment-on-send semantics identical to C++).
- Pulse layer: frame ⇄ OOK pulse train (level + duration pairs: wakeup,
  hardware sync, software sync, 640 µs (SYMBOL) Manchester half-symbols
  (erratum: earlier draft said 604 µs — folklore; Somfy.cpp SYMBOL=640 is
  authoritative), inter-frame gap, repeat frames with reduced sync). Pure data
  in/out — no GPIO or timer knowledge.
- RX decoder: the C++ `somfy_rx_t` state machine reimplemented
  (sync detection → Manchester decode → checksum → repeat dedupe), consuming
  duration sequences from any capture source.

### 3.2 `somfy-domain` — domain model

- Fixed-capacity registries (heapless): 32 shades, 16 groups, 16 rooms.
- `Shade`: 24-bit address, rolling code, name, shade type, tilt mode,
  up/down/tilt travel times (ms), current + target position (0–100),
  tilt position, "my" position, up to 7 linked remote addresses.
- Position engine: dead-reckoning with an injected clock. Movement start
  records a timestamp; ticks integrate elapsed/travel-time into position;
  go-to-position computes travel duration and schedules the stop command.
  Linked-remote frames overheard on RX drive the same estimator so external
  wall remotes keep tracked position honest.
- Command orchestration: target-position requests become RTS command
  sequences; emits typed state-change events consumed by web/MQTT layers.

### 3.3 `somfy-api` — shared contract

- All REST/WebSocket/MQTT payload types, `serde`-derived.
- Compiles on the host; `ts-rs` derives generate TypeScript types into
  `ui/src/api/` at build time. UI/firmware drift is a compile error.

### 3.4 `somfy-migrate`

- Parser for the C++ ESPSomfy-RTS backup file format (as implemented in
  `ConfigFile.cpp` / `ConfigSettings.cpp` of the reference repo).
- Maps shades, addresses, rolling codes, groups, rooms, and MQTT settings
  into the Rust schema. Network credentials are re-entered by the user
  (captive portal), not imported.
- Validated against real backups exported from the author's running device.

### 3.5 `firmware`

- One binary crate; target chip selected by Cargo feature (`chip-s3`,
  `chip-c3`). `chip-esp32` was dropped 2026-08-18 and `chip-s2` 2026-08-17; see
  §1.2.
- Drivers: minimal CC1101 SPI driver (own module, `embedded-hal` traits,
  ~15 registers actually used) + RMT OOK TX/RX.
- Embassy tasks (Section 4).

## 4. Runtime Model (Embassy tasks)

Statically allocated tasks communicating over bounded channels:

| Task | Responsibility |
|---|---|
| **radio** | Sole owner of CC1101 + RMT channels. Consumes `TransmitRequest`s from a bounded channel (replaces C++ TX buffer queue); publishes decoded RX frames. Radio timing never blocks on network work. |
| **state manager** | Owns the `somfy-domain` registry; applies commands and RX frames; runs position-estimator ticks; broadcasts state deltas via Embassy watch/pubsub. |
| **http** | `picoserve`: static Preact assets + REST + WebSocket event stream. |
| **mqtt** | `rust-mqtt` client: HA discovery, state publication, command subscription, LWT availability. |
| **ota** | GitHub release polling + manual upload handling; writes inactive partition. |
| **persistence** | Debounced config writes; synchronous rolling-code writes. |

**Critical invariant (carried from C++):** the incremented rolling code is
persisted to flash **before** the frame transmits. A crash after TX with an
unsaved code de-syncs the motor pairing.

## 5. Radio Subsystem

### 5.1 CC1101

- Configured over SPI, then run in **asynchronous serial mode**: the CC1101
  is a dumb 433.42 MHz OOK modem; the ESP32 supplies/reads the raw bitstream
  on a GDO pin. Same approach as the C++ firmware.
- Own minimal driver module (the `cc1101` crate v0.1.x lacks async-serial
  support; it serves as a register reference only). Written against
  `embedded-hal` SPI traits, unit-mockable.
- Frequency is configuration (433.42 MHz default) so RTx variants later are
  settings, not code.

### 5.2 TX — RMT peripheral

`somfy-rts` renders the pulse train; an RMT TX channel replays it into the
CC1101 data pin in hardware. Microsecond-exact regardless of WiFi/web load —
removes the C++ version's interrupt-timing fragility.

### 5.3 RX

An RMT RX channel captures pulse durations; the `somfy-rts` decoder consumes
them. **Contingency:** if RMT RX idle-threshold handling proves awkward for
Somfy's long frames, fall back to GPIO-interrupt timestamping for RX only;
TX stays on RMT either way.

Per-chip constraint recorded: RMT channels — ESP32: 8, S3: 4 TX + 4 RX,
S2: 4, C3: 2 TX + 2 RX. One TX + one RX channel needed; all targets fit.

### 5.4 Correctness — golden captures

Before any Rust TX reaches a real motor:

1. Capture pulse trains from the running C++ firmware (logic analyzer on the
   data pin and/or the firmware's raw pulse dump — `somfy_rx_t` records
   pulses).
2. Store as fixture files in `somfy-rts/tests/fixtures/`.
3. Host tests assert: encoder output is byte-identical to captured frames
   (56 & 80-bit, repeats, sync counts); decoder round-trips real captures.
4. Rolling-code compatibility: import a C++ backup, verify the next
   generated frame matches what the C++ version would send.

## 6. Persistence

Two flash regions with distinct write patterns (`sequential-storage` crate,
`postcard` serialization, schema-versioned records):

- **Config** (shades, groups, rooms, network, MQTT, security): map store,
  debounced writes.
- **Rolling codes**: append-style wear-leveled region, written synchronously
  before every TX. ~2 writes per command against 100k-cycle NOR endurance ×
  wear-leveled pages outlives the motors.

**UI assets are embedded in the firmware image** (`include_bytes!`,
pre-gzipped by `xtask`). No filesystem. One image = firmware + UI, so OTA is
atomic and always self-consistent.

## 7. Network Services

### 7.1 WiFi & provisioning

- `esp-radio` station mode, exponential-backoff reconnect.
- First boot or prolonged connect failure → **setup AP + captive portal**
  serving the Preact app in onboarding mode (scan/join network, optional
  restore-from-backup).
- Network interface behind a small trait so Ethernet can be added post-1.0.

### 7.2 HTTP API

- REST under `/api/v1/`: `shades`, `groups`, `rooms`, `settings`, `system`
  resources; command endpoints like `POST /api/v1/shades/{id}/command`.
- One WebSocket `/api/v1/events` streaming JSON state deltas (movement,
  position ticks, config changes).
- All payloads typed in `somfy-api`; TS types generated via `ts-rs`.

### 7.3 Auth

- Optional single-credential password (off by default). Login endpoint
  issues a random session token (cookie); mutating endpoints require it;
  auth attempts rate-limited. No user database.

**Deferred by the owner, 2026-08-17: no authentication for now.** The remaining
features come first. Recorded here rather than dropped, with the exposure stated
so the decision is revisited on evidence and not rediscovered:

- **The device serves an unauthenticated API on the LAN.** Once the settings
  screen exists it will also *serve* the Wi-Fi PSK and MQTT password, which
  turns an open API from an actuation risk into a credential-disclosure one.
  That is the point at which this must be reconsidered.
- **"LAN-only" is weaker than it sounds.** Any page in any browser tab can issue
  requests to the device's address; reachability does not require being on the
  network. This is the classic router attack.
- **Two mitigations are not authentication and should ship regardless**, since
  they need no password, no session and no login screen:
  - **Origin/Host validation** — reject requests whose `Origin` is not the
    device. This, not the password, is the actual defence against the item
    above.
  - **Rate limiting per shade.** Every command commits a rolling code to flash
    *before* transmitting, and that ordering is a correctness guarantee that
    cannot be dropped. So a request loop causes flash wear and makes the
    receiver deaf while it writes — a physical-damage path that authentication
    would not close anyway, since an authenticated client can loop too.

**Both shipped 2026-08-18.** Authentication is still deferred and nothing below
changes that; what follows is where each of the two now lives.

- **Origin/Host** is `somfy_api::origin`, applied by
  `firmware::api::origin::FromThisDevice`, an extractor every `/api/v1` handler
  takes — including the `/api/v1/events` WebSocket upgrade, which the browser's
  same-origin policy does *not* restrain the way it restrains `fetch`. Two
  rules: `Host` must be an IPv4 literal or `somfy-<mac>[.local]`, and `Origin`,
  when present, must be `http://` and the same authority the request was
  addressed to. An **absent** header is admitted on every method, because a
  browser cannot be made to omit one where it sends one — so absence means the
  caller is not a browser, and the attack needs a browser. Refusals are `403`
  with `hostNotThisDevice` / `originNotThisDevice`. The static asset routes and
  the SPA fallback are deliberately outside it: they serve the compiled UI,
  which discloses nothing and actuates nothing.
- **The rate limit** is `somfy_tasks::CommandLimiter`, consulted from
  `StateMachine::apply` — the one function an HTTP command and an MQTT command
  both arrive at, so there is one limit rather than two that agree today. Twelve
  commands per shade back to back, then one per twenty seconds. It is
  deliberately *not* consulted from `StateMachine::tick`, which is where the
  second and third frames of a vent are planned: those are due at a time the
  clock picked, and refusing them would leave a shade closed with no vent
  coming. Refusals are `429` with `commandRateLimited`.

**What is still open, and it is the residual rather than an oversight.** The
rolling-code region is shared by every shade, so a per-shade bound multiplies by
the shade count: thirty-two shades driven flat out at once wear it out in days
rather than the years one shade takes. A device-wide cap would close that and
would also let one abused shade starve the whole house, which is the lockout
this codebase spends `api::REST_TASKS_RESERVED` to make impossible.
`somfy_tasks::REFILL_INTERVAL_MS` carries the arithmetic and is the place to
reopen it.

TLS was considered and declined for now: `esp-mbedtls` handshake buffers want
32–64 KB against the S3's ~11 KB of heap headroom over the measured announcement
peak, it would certainly end C3 support as it already ended the ESP32's, and
self-signed certificates train users to click through warnings. The C3 has a
second reason of its own: `sntp` is refused there, so it has no wall clock, and
certificate validity is a wall-clock question. Revisit if the heap picture
changes.

### 7.4 MQTT + Home Assistant

- `rust-mqtt` over `embassy-net`. LWT availability topic; per-shade state
  topics (position, tilt, direction); command topics.
- **HA MQTT discovery**: `cover` entities with position + tilt, zero-YAML
  onboarding. Topic layout is new/clean; the discovery payloads make the HA
  experience identical to the C++ version.

### 7.5 OTA

Two paths, both writing the inactive A/B partition
(`esp-bootloader-esp-idf` partition layout):

1. **GitHub releases**: fetch release manifest over HTTPS (`reqwless` +
   `esp-mbedtls`), compare versions, stream the chip-matching binary,
   verify SHA-256 from the manifest before marking bootable.
2. **Manual upload** from the web UI (no-internet fallback).

Boot-side: new image runs a self-test (radio SPI alive, config loads,
network up within a window) then marks itself valid; otherwise the
bootloader rolls back. A daily driver must not brick from a bad release.

### 7.6 Discovery & time

- mDNS responder (`edge-mdns`) → `http://<hostname>.local`.
- SNTP client for wall-clock time (log timestamps, TLS validation).

## 8. Web UI

- **Stack:** Preact + Vite + TypeScript. Budget ≤ 200 KB gzipped total.
- **Screens:** dashboard (rooms → groups → shade tiles, up/my/down +
  position slider), shade detail (tilt, travel-time calibration, linked
  remotes), pairing assistant, settings (WiFi, MQTT, security, OTA),
  backup/restore (including C++ import), diagnostics (log buffer, last
  panic), captive-portal onboarding mode.
- **i18n from day one:** English + French in v1.0.
- **Mock-driven development:** a Vite dev-server plugin serves a fake REST +
  WebSocket API built on the `ts-rs`-generated types; UI work needs zero
  hardware and the mock cannot drift from the firmware contract.

## 9. Error Handling

- Steady-state firmware never panics: fallible paths return typed
  per-subsystem error enums; degraded services (e.g., MQTT broker down)
  retry with backoff and surface status — blinds keep working.
- Panic handler persists message + location to flash, then reboots via
  watchdog; diagnostics page shows the last panic.
- Every Embassy task feeds a soft watchdog; a hung task triggers recovery
  rather than a silently dead subsystem.
- Logging: `defmt` over USB/UART in development; in-memory ring buffer
  exposed in the UI (replaces the C++ log viewer).

## 10. Testing Strategy

| Layer | Tests |
|---|---|
| `somfy-rts` | Golden-capture fixtures + encode/decode round-trip property tests. Highest-value tests in the project. |
| `somfy-domain` | Property tests on the position estimator: interruptions, reversals, tilt, overheard remote frames. |
| `somfy-migrate` | Fixtures from real backups exported from the author's C++ device. |
| `somfy-api` | Serialization snapshots; TS generation checked in CI. |
| UI | `vitest` for logic; small Playwright suite against the mock server (control a shade, pair, restore backup). |
| Hardware | Manual smoke checklist per release: pair, move, position accuracy, OTA cycle, rollback. Automated HIL out of scope. |

## 11. CI & Releases

- **Every PR (GitHub Actions):** `cargo fmt --check`, `clippy -D warnings`,
  host tests, UI build + type generation check, firmware build for all four
  chips (Espressif Rust toolchain via `espup`).
- **Tagged release:** per-chip binaries + `manifest.json` (version, per-chip
  SHA-256, URLs) — the exact artifact the firmware's GitHub-OTA path
  consumes — plus an `esp-web-tools` web-flasher page for browser-based
  first-time flashing.

## 12. Key Risks

| Risk | Mitigation |
|---|---|
| `esp-radio` pre-1.0 API churn | Pin versions; network behind a trait; worst case swap the shell for esp-idf (Section 2). |
| RMT RX unsuitable for long Somfy frames | Contingency: GPIO-interrupt RX (Section 5.3); TX unaffected. |
| Rolling-code corruption bricks pairings | Persist-before-TX invariant + wear-leveled region + golden-capture compatibility tests + backup export in UI. |
| C++ backup format quirks | Parser written against `ConfigFile.cpp` as source of truth; validated with real device backups. |
| Xtensa toolchain friction (S3) | `espup`-managed toolchain in CI and documented setup; C3 (RISC-V) remains the friction-free dev target, which is also why `crates/firmware/rust-analyzer.toml` names it. |

## 13. Success Criteria

1. Author's ESP32-S3 + CC1101 controls their blinds daily with position
   tracking as accurate as the C++ version.
2. Migration from a C++ ESPSomfy-RTS device via backup import requires no
   re-pairing at the motors.
3. Home Assistant shows the same cover entities/behavior as before.
4. An OTA update (and a forced-bad-image rollback) completes without
   physical access.
5. A new adopter can flash from the browser and onboard via captive portal
   without reading source code.
