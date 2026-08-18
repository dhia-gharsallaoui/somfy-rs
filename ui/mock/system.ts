/**
 * The mock device's account of its own past: what it is, how it started, how
 * much memory it has spent, every line it has printed — and what the last
 * backup uploaded to it did.
 *
 * The restore half is at the foot of the file. It starts at `none`, which is
 * what a device that has never been restored reports and therefore the state the
 * screen has to handle before any other; the export, the staging rules and the
 * pretend reboot that settles an upload are documented beside them.
 *
 * ## The fixture is the interesting state, not the healthy one
 *
 * A mock that booted clean and logged nothing would leave the two things this
 * screen exists for — a panic and a full log ring — visible only on a board
 * that has actually fallen over. So the seed is a device that **panicked and
 * restarted**: {@link SEED_PANIC} is recorded with `bootsSince: 0`, which is
 * the firmware's way of saying "this boot is the one the panic caused", and the
 * log opens with the tail of the boot *before* it.
 *
 * `dropped` is non-zero for the same reason, and it is not a hand-written
 * number: {@link Ring} is a real ring, {@link CAPACITY_BYTES} is smaller than
 * the seed, and the eviction that produces the figure is the same eviction the
 * device performs. A literal would have been a number the screen could not
 * disagree with.
 *
 * ## Nothing here is anybody's
 *
 * Every identifier is synthetic and stays inside a documentation range:
 * `example-network` (the same SSID `settings.ts` seeds), `192.0.2.x` from
 * RFC 5737's TEST-NET-1, and a host name whose MAC half is `0011223344ff` —
 * ascending nibbles, which no vendor was ever assigned.
 */
import type { ApiErrorCode } from '../src/api/generated/ApiErrorCode.ts';
import type { LogDto } from '../src/api/generated/LogDto.ts';
import type { PanicDto } from '../src/api/generated/PanicDto.ts';
import type { RestoreReportDto } from '../src/api/generated/RestoreReportDto.ts';
import type { SystemDto } from '../src/api/generated/SystemDto.ts';

/**
 * The ring's size, in bytes.
 *
 * 2 KiB, chosen so the seed below **overflows it**. The real device's ring is
 * larger; what matters for UI work is that `dropped` is reachable from a
 * browser without waiting for a fault, and the only honest way to reach it is
 * to make the ring too small for what goes into it — which is exactly the
 * condition the screen is meant to report.
 */
const CAPACITY_BYTES = 2048;

/**
 * How long this device claims to have been up when the mock starts, in seconds.
 *
 * Three days and change, and the figure is picked to exercise the coarse end of
 * the duration formatter — a screen developed only against "42 s" would ship a
 * raw `276220` the first time somebody left a device alone for a weekend. The
 * panic's own uptime is twelve seconds, so both ends of the formatter are on
 * screen at once.
 */
const SEED_UPTIME_S = 3 * 86_400 + 4 * 3_600 + 37 * 60;

/**
 * The panic the seeded device is recovering from.
 *
 * Synthetic, and deliberately the shape a real one has: `core::panic::PanicInfo`
 * renders as a source location and a message, and the message is an assertion
 * because that is what most of them are.
 */
const SEED_PANIC: PanicDto = {
  text: 'panicked at src/tasks.rs:412:9: assertion failed: plan.repeats > 0',
  truncated: false,
  uptimeS: 12,
  // Zero: this boot is the one the panic caused. It is the state worth
  // developing against, because it is the one where the screen has to say so
  // rather than merely print a count.
  bootsSince: 0,
};

/**
 * The seeded output, oldest first.
 *
 * The first four lines are the tail of the boot that panicked; everything after
 * `--- reset ---` is this one. That ordering is what makes the ring's eviction
 * mean something on screen: the lines it throws away are the ones explaining
 * *why* the device restarted, which is the argument for reporting `dropped`
 * loudly rather than as a statistic.
 *
 * The `stack:` line is quoted verbatim from `crates/firmware/src/main.rs`'s
 * `report_stack_use`, with the figures `docs/provenance.md` records, so the
 * screen's own rendering of the same three numbers can be read against it.
 */
const SEED_LINES: readonly string[] = [
  'mqtt: shade 2 position 64',
  'rts: queueing Up at 0x8ACE01, rolling 0x0195, 80-bit',
  'rts: transmit failed — plan built with no repeats',
  'panicked at src/tasks.rs:412:9: assertion failed: plan.repeats > 0',
  '--- reset ---',
  'boot: somfy-rs 0.1.0 (esp32s3), reset reason software',
  'diag: a panic is recorded from the previous boot, 12 s in',
  'heap: 200704 bytes of internal DRAM',
  'config: RTSC seq 41 — 1 network, 1 broker',
  'config: RTSS seq 17 — 4 shades',
  'config: RTSE seq 6 — 2 rooms, 1 group',
  'radio: CC1101 found, 433.42 MHz, asynchronous serial OOK',
  'radio: receiver armed on GPIO 4',
  'wifi: associating with example-network',
  'wifi: associated, channel 6, rssi -58 dBm',
  'net: dhcp lease 192.0.2.37/24, gateway 192.0.2.1',
  'mdns: responder up as somfy-0011223344ff.local',
  'sntp: 192.0.2.1 answered, clock moved forward 0.412 s',
  'mqtt: connected to 192.0.2.10:1883 as somfy',
  'mqtt: fresh session — publishing 4 discovery configs under homeassistant/',
  'mqtt: retained availability online at somfyrs/status',
  'stack: 54064 bytes used at the deepest point of boot, of 55792 required — 1728 bytes of the requirement unspent',
  'http: listening on 0.0.0.0:80',
  'rx: 0x8ACE01 rolling 0x0193, command Up, 80-bit',
  'shade 2: Up — estimate 0% climbing to 100% over 22.4 s',
  'rx: 0x8ACE01 rolling 0x0194, command My, 80-bit',
  'shade 2: My while moving — stopping, estimate 64%',
  'mqtt: shade 2 position 64',
  'rx: 0x8ACE02 rolling 0x0041, command Down, 56-bit',
  'shade 3: Down — estimate 100% falling to 0% over 19.8 s',
  'shade 3: reached the lower limit — estimate exact again',
  'mqtt: shade 3 position 0',
  'http: GET /api/v1/shades 200',
  // Three days of a device doing its job. This tail exists to make the seed
  // larger than the ring — without it nothing is ever evicted, `dropped` reads
  // zero, and the state this screen most needs to render would be reachable
  // only from a board that had been left running for a week.
  'mqtt: keepalive — 3 pings, 0 missed',
  'rx: 0x8ACE03 rolling 0x0112, command Down, 80-bit',
  'shade 4: Down — estimate 100% falling to 0% over 24.1 s',
  'shade 4: reached the lower limit — estimate exact again',
  'mqtt: shade 4 position 0',
  'rx: 0x8ACE01 rolling 0x0196, command Up, 80-bit',
  'shade 2: Up — estimate 64% climbing to 100% over 8.1 s',
  'shade 2: reached the upper limit — estimate exact again',
  'mqtt: shade 2 position 100',
  'rx: 0x8ACE02 rolling 0x0042, command Up, 56-bit',
  'shade 3: Up — estimate 0% climbing to 100% over 21.2 s',
  'mqtt: shade 3 position 100',
  'sntp: 192.0.2.1 answered, clock moved forward 0.008 s',
  'mqtt: keepalive — 6 pings, 0 missed',
  'rx: 0x8ACE01 rolling 0x0197, command Down, 80-bit',
  'shade 2: Down — estimate 100% falling to 0% over 22.4 s',
  'shade 2: reached the lower limit — estimate exact again',
  'mqtt: shade 2 position 0',
  'http: GET /api/v1/shades 200',
  'http: GET /api/v1/system 200',
];

/**
 * A byte-bounded line ring, evicting oldest-first — `firmware`'s model.
 *
 * The eviction is the whole content of this class: `dropped` is only a truthful
 * number if the lines it counts were really thrown away, and the alternative —
 * a fixture that simply states a figure — would let the screen and the log
 * disagree without anything noticing.
 */
class Ring {
  private lines: string[] = [];
  private usedBytes = 0;
  private droppedLines = 0;

  push(line: string): void {
    this.lines.push(line);
    this.usedBytes += cost(line);
    while (this.usedBytes > CAPACITY_BYTES && this.lines.length > 0) {
      const evicted = this.lines.shift();
      if (evicted === undefined) break;
      this.usedBytes -= cost(evicted);
      this.droppedLines += 1;
    }
  }

  /** Oldest line first, every line newline-terminated, as the endpoint sends. */
  text(): string {
    return this.lines.map((line) => `${line}\n`).join('');
  }

  stats(): LogDto {
    return {
      capacity: CAPACITY_BYTES,
      bytes: this.usedBytes,
      lines: this.lines.length,
      dropped: this.droppedLines,
    };
  }

  empty(): void {
    this.lines = [];
    this.usedBytes = 0;
    // Reset with the ring rather than kept: `dropped` counts evictions "since
    // the ring was last empty", so carrying the old figure past an emptying
    // would report a loss with no missing lines behind it.
    this.droppedLines = 0;
  }
}

// ---------------------------------------------------------------------------
// Backup and restore
// ---------------------------------------------------------------------------

/**
 * The `RTSB` container's length, in bytes.
 *
 * Not a round number and not invented: `somfy_backup::BACKUP_LEN` is
 * `HEADER_LEN + SHADE_RECORD_LEN + ESTATE_RECORD_LEN + CRC_LEN`, which is
 * 320 + 2048 + 2048 + 4. The header's own 320 is 64 bytes of metadata plus
 * 32 code entries of 8 bytes. The figure is here rather than in a comment on the
 * export because a screen that shows a file size should be shown the real one.
 */
const BACKUP_LEN = 320 + 2048 + 2048 + 4;

/**
 * The container's first four bytes: `R`, `T`, `S`, `B`.
 *
 * Written by the export and looked for by the import, so a file taken out of
 * this mock and put straight back into it is recognised — which is the one round
 * trip a person developing this screen will actually perform.
 */
const MAGIC = new Uint8Array([0x52, 0x54, 0x53, 0x42]);

/**
 * The largest upload the staging region holds — `firmware::restore`'s
 * `STAGE_MAX_BYTES`, sized for the worst C++ backup a supported controller
 * writes.
 */
const STAGE_MAX_BYTES = 16_384;

/**
 * How long the mock device pretends to be restarting.
 *
 * Three seconds: long enough that the screen's "the device is restarting" state
 * is on screen for a moment rather than flickering past, and short enough that
 * somebody iterating on this page is not waiting on it. A real board takes
 * longer — its own seeded log walks through boot, association and a DHCP lease —
 * so this is a floor for the UI's patience, not a model of the device.
 */
const APPLY_DELAY_MS = 3_000;

/**
 * The C++ backup versions `somfy_migrate::parse_header` accepts.
 *
 * Below 19 the record layouts differ; above 25 is a format that could have
 * appended fields and silently misaligned every record parser.
 */
const CPP_VERSION_MIN = 19;
const CPP_VERSION_MAX = 25;

/**
 * What a successful restore reports having written.
 *
 * A fixture, and chosen to agree with the seeded log above rather than picked
 * freely: `config: RTSS seq 17 — 4 shades` and `config: RTSE seq 6 — 2 rooms,
 * 1 group` are lines this same mock already serves, and a restore of this
 * device's own configuration that reported different counts would let a screen
 * ship that nobody could read against the log.
 *
 * `warnings` is 2 for the opposite reason — nothing in the seed implies it, and
 * it is here so the warning sentence and its link to the log are developable
 * without editing this file. A real one comes from a record accepted with a
 * caveat.
 */
const APPLIED_SHADES = 4;
const APPLIED_ROOMS = 2;
const APPLIED_GROUPS = 1;
const APPLIED_WARNINGS = 2;

/** A device that has never been sent a backup — `firmware::restore`'s `State::nothing`. */
function noRestore(): RestoreReportDto {
  return {
    outcome: 'none',
    format: null,
    shades: 0,
    rooms: 0,
    groups: 0,
    warnings: 0,
    error: null,
    row: null,
    contents: null,
  };
}

/**
 * Whether these bytes begin like something the device can read — the mock's copy
 * of `firmware::restore::recognisable`.
 *
 * Two formats. An `RTSB` container announces itself with a magic. A C++
 * ESPSomfy-RTS backup is text whose first field is the format version, so it
 * begins with an ASCII digit possibly preceded by the space padding that
 * firmware's `%3u` writes.
 */
function recognisable(body: Uint8Array): 'somfyRs' | 'espSomfyRts' | undefined {
  if (MAGIC.every((byte, at) => body[at] === byte)) return 'somfyRs';
  const first = [...body].find((byte) => !isAsciiSpace(byte));
  return first !== undefined && first >= 0x30 && first <= 0x39 ? 'espSomfyRts' : undefined;
}

const isAsciiSpace = (byte: number): boolean =>
  byte === 0x20 || (byte >= 0x09 && byte <= 0x0d);

/** The version field a C++ backup opens with, or `undefined` if it has no digits. */
function cppVersion(body: Uint8Array): number | undefined {
  let at = 0;
  while (at < body.length && isAsciiSpace(body[at] ?? 0)) at += 1;
  let value = 0;
  let seen = false;
  while (at < body.length) {
    const byte = body[at] ?? 0;
    if (byte < 0x30 || byte > 0x39) break;
    value = value * 10 + (byte - 0x30);
    seen = true;
    at += 1;
  }
  return seen ? value : undefined;
}

/**
 * The device's self-report, and the one destructive action on it.
 *
 * The uptime is computed from wall-clock rather than stored, so it advances
 * while the dev server runs — a screen developed against a frozen number is a
 * screen nobody notices has stopped refreshing.
 */
export class System {
  private readonly startedMs = Date.now();
  private readonly ring = new Ring();
  private panic: PanicDto | undefined = SEED_PANIC;
  /**
   * What the last upload did.
   *
   * Starts as `none`, which is what a device that has never been restored
   * reports and therefore the state the screen must handle first.
   */
  private restore: RestoreReportDto = noRestore();
  private applying: ReturnType<typeof setTimeout> | undefined;

  constructor() {
    for (const line of SEED_LINES) this.ring.push(line);
  }

  read(): SystemDto {
    return {
      chip: 'esp32S3',
      firmware: '0.1.0',
      host: 'somfy-0011223344ff',
      uptimeS: SEED_UPTIME_S + Math.floor((Date.now() - this.startedMs) / 1000),
      resetReason: 'software',
      // The three figures the boot line prints, as `docs/provenance.md` records
      // them for the ESP32-S3, so the screen's arithmetic can be checked
      // against the `stack:` line in the seeded log rather than against itself.
      stack: { available: 57_344, required: 55_792, used: 54_064 },
      // `peak` above `used` because the Wi-Fi association is the high-water
      // mark and it is already over; a mock where the two were equal would hide
      // the distinction the screen is built around.
      heap: { size: 200_704, used: 121_856, peak: 148_992 },
      log: this.ring.stats(),
      lastPanic: this.panic ?? null,
    };
  }

  log(): string {
    return this.ring.text();
  }

  /**
   * Forget the panic and empty the log.
   *
   * One method because it is one endpoint: `DELETE /api/v1/system` clears both,
   * and a mock offering them separately would let a screen ship that assumed it
   * could keep one.
   */
  forget(): void {
    this.panic = undefined;
    this.ring.empty();
  }

  // -------------------------------------------------------------------------
  // Backup and restore
  // -------------------------------------------------------------------------

  /**
   * The exported container.
   *
   * The right length and the right magic, and **not a decodable container** —
   * everything after the first four bytes is zero, where a real export carries
   * two 2 KiB flash records, a block of address-and-code pairs and a CRC-32 over
   * the lot. That is enough for what this mock is for: the browser saves a file
   * of the size the device would have sent, and putting it back through
   * {@link stage} exercises the import path, because the magic is what that path
   * looks at.
   *
   * Filling it with anything more would mean a second implementation of the
   * container format in TypeScript, which is exactly the drift this mock's whole
   * design avoids by only ever handling generated types.
   */
  backup(): Uint8Array {
    const bytes = new Uint8Array(BACKUP_LEN);
    bytes.set(MAGIC, 0);
    return bytes;
  }

  /**
   * Take an uploaded file, or refuse it — the mock's half of `POST
   * /api/v1/system/backup`.
   *
   * The refusals mirror `firmware::restore::Staging::begin` and `page`, which
   * are the only checks the device makes *before* the reboot: a length of zero
   * or one past the staging region is [`backupTooLarge`], a first page that
   * begins like neither format is [`backupNotRecognised`], and a file already
   * staged is [`restoreInProgress`] rather than a silent replacement — the
   * staged one is about to be applied, and overwriting it would discard
   * something the operator has already been told was accepted.
   *
   * Everything else is settled after the pretend reboot, by {@link applyLater}.
   */
  stage(body: Uint8Array): { ok: true } | { error: ApiErrorCode } {
    if (this.restore.outcome === 'staged') return { error: 'restoreInProgress' };
    // `declared == 0 || declared > STAGE_MAX_BYTES` on the device, both as
    // `BackupTooLarge`. An empty body is the odd one under that name, and it is
    // the device's rule rather than this mock's: the check is on the declared
    // `Content-Length`, before a byte has been read.
    if (body.length === 0 || body.length > STAGE_MAX_BYTES) return { error: 'backupTooLarge' };

    const format = recognisable(body);
    if (format === undefined) return { error: 'backupNotRecognised' };

    this.restore = { ...noRestore(), outcome: 'staged', format };
    this.applyLater(body, format);
    return { ok: true };
  }

  /** What the last upload did, for `GET /api/v1/system/restore`. */
  restoreReport(): RestoreReportDto {
    return this.restore;
  }

  /**
   * Become the boot that reads the staged file.
   *
   * **The one input that is refused is a C++ backup declaring a version outside
   * 19..=25**, which is `somfy_migrate::parse_header`'s window and the real
   * reason a real device refuses one after a reboot. It is reachable in about
   * five seconds — save a text file whose first line starts `1,` and upload it —
   * so the refused state can be developed against without editing this file.
   *
   * Note what that refusal does *not* carry: a `row`. An unsupported version is
   * a statement about the file rather than about a record in it, and
   * `RestoreReportDto.row` is documented as null in exactly that case. A
   * row-bearing refusal comes from a record the parser rejected, which this mock
   * does not parse — so the screen's "record N" line is written and reviewed but
   * is not reachable from here.
   */
  private applyLater(body: Uint8Array, format: 'somfyRs' | 'espSomfyRts'): void {
    if (this.applying !== undefined) clearTimeout(this.applying);
    this.applying = setTimeout(() => {
      this.applying = undefined;
      const version = format === 'espSomfyRts' ? cppVersion(body) : undefined;
      if (version !== undefined && (version < CPP_VERSION_MIN || version > CPP_VERSION_MAX)) {
        this.restore = {
          ...noRestore(),
          outcome: 'refused',
          format,
          error: { code: 'backupUnsupportedVersion' },
        };
        return;
      }
      this.restore = {
        outcome: 'applied',
        format,
        shades: APPLIED_SHADES,
        rooms: APPLIED_ROOMS,
        groups: APPLIED_GROUPS,
        warnings: APPLIED_WARNINGS,
        error: null,
        row: null,
        // Null for a C++ backup, and that is the format's own property rather
        // than a gap in the mock: it keeps network credentials in NVS, so the
        // file says nothing about which network or which broker the old device
        // used. A `somfy-rs` backup names both — the same two the settings mock
        // seeds, so a person can read one screen against the other.
        contents:
          format === 'somfyRs'
            ? {
                ssid: 'example-network',
                pskWasSet: true,
                broker: '192.0.2.10',
                brokerPasswordWasSet: false,
              }
            : null,
      };
    }, APPLY_DELAY_MS);
  }
}

/**
 * What one line costs the ring, in bytes.
 *
 * UTF-8, because the bound is in bytes and the boot line's `—` costs three of
 * them; plus one for the newline the ring stores with it.
 */
function cost(line: string): number {
  return new TextEncoder().encode(line).length + 1;
}
