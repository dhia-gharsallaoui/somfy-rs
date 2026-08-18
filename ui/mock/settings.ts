/**
 * The mock device's configuration region, and a simulated credential trial.
 *
 * ## Why the trial is simulated rather than stubbed out
 *
 * The Wi-Fi flow is the one part of this screen that cannot be checked against
 * a real device without losing the connection it is being checked over, and it
 * is also the part with a guard in it. So the mock runs the *same* shape: a
 * candidate is accepted, the device becomes unreachable, association either
 * happens or does not, and the credential is stored only if somebody confirms
 * — with the same two deadlines the firmware uses.
 *
 * What it deliberately does **not** do is make the mock server unreachable. It
 * cannot: it is the dev server, and killing it would kill the page. So the
 * unreachable half of the flow is exercised by stopping the dev server, or by
 * the device itself; what is exercised here is everything either side of it.
 *
 * ## The fixture conventions
 *
 * Two, both in the same spirit as `world.ts`'s "a `vent` on an uncalibrated
 * shade answers `ventBandNotMeasured`" — the behaviour is chosen by the input
 * so that every branch is reachable from a browser:
 *
 * - An SSID containing **`unreachable`** never associates, so the association
 *   deadline fires and the trial reverts with nothing stored.
 * - Every other SSID associates after {@link ASSOCIATE_AFTER_MS}.
 *
 * The third ending — associated, and nobody confirms — needs no convention:
 * start a trial and do not press the button.
 */
import type { ApiErrorCode } from '../src/api/generated/ApiErrorCode.ts';
import type { MqttSettingsDto } from '../src/api/generated/MqttSettingsDto.ts';
import type { MqttUpdateDto } from '../src/api/generated/MqttUpdateDto.ts';
import type { SecretDto } from '../src/api/generated/SecretDto.ts';
import type { SettingsDto } from '../src/api/generated/SettingsDto.ts';
import type { SettingsFieldDto } from '../src/api/generated/SettingsFieldDto.ts';
import type { WifiSettingsDto } from '../src/api/generated/WifiSettingsDto.ts';
import type { WifiUpdateDto } from '../src/api/generated/WifiUpdateDto.ts';

/**
 * The two deadlines, mirrored from `somfy_config::ASSOCIATE_DEADLINE_MS` and
 * `CONFIRM_DEADLINE_MS`.
 *
 * Mirrored rather than generated: `ts-rs` exports types, not constants, and a
 * mock whose clock ran faster than the device's would make this screen feel
 * like something it is not. If those figures move, these move with them — the
 * only cost of a drift here is a mock that counts down differently, which is
 * visible the first time anyone looks.
 */
const ASSOCIATE_DEADLINE_MS = 45_000;
const CONFIRM_DEADLINE_MS = 180_000;

/**
 * How long a candidate takes to join, when it is going to.
 *
 * Three seconds: long enough that the "joining…" state is actually seen, short
 * enough that nobody waits for it.
 */
const ASSOCIATE_AFTER_MS = 3_000;

/** An SSID carrying this never associates. See this module's docs. */
const NEVER_ASSOCIATES = 'unreachable';

/** Secrets, which never leave this object. */
interface Stored {
  wifi: { ssid: string; psk: string } | undefined;
  mqtt:
    | {
        address: string;
        port: number;
        username: string;
        password: string;
        discoveryPrefix: string;
        stateRoot: string;
      }
    | undefined;
}

interface Trial {
  ssid: string;
  psk: string;
  startedMs: number;
  /** When the station joined, or `undefined` while it has not. */
  joinedMs: number | undefined;
}

/** A typed rejection with the field it is about, exactly as the device sends. */
export interface Rejection {
  error: ApiErrorCode;
  field?: SettingsFieldDto;
}

export type Outcome = { ok: true } | Rejection;

/**
 * The device's persisted settings and whatever trial is live.
 *
 * The seed is the same synthetic pair the firmware's `config-check` writes, so
 * nothing here looks like a real credential.
 */
export class Settings {
  private stored: Stored = {
    wifi: { ssid: 'example-network', psk: 'PLACEHOLDER_PASSPHRASE' },
    mqtt: {
      address: '192.0.2.10',
      port: 1883,
      username: 'somfy',
      password: 'PLACEHOLDER_BROKER_PASSWORD',
      discoveryPrefix: 'homeassistant',
      stateRoot: 'somfyrs',
    },
  };

  private trial: Trial | undefined;

  /** What the settings screen reads. Secrets are replaced by whether they exist. */
  read(): SettingsDto {
    this.advance();
    return {
      wifi: this.stored.wifi
        ? ({ ssid: this.stored.wifi.ssid, pskSet: this.stored.wifi.psk.length > 0 } satisfies WifiSettingsDto)
        : null,
      mqtt: this.stored.mqtt
        ? ({
            address: this.stored.mqtt.address,
            port: this.stored.mqtt.port,
            username: this.stored.mqtt.username,
            passwordSet: this.stored.mqtt.password.length > 0,
            discoveryPrefix: this.stored.mqtt.discoveryPrefix,
            stateRoot: this.stored.mqtt.stateRoot,
          } satisfies MqttSettingsDto)
        : null,
      wifiTrial: this.trial
        ? {
            ssid: this.trial.ssid,
            phase: this.trial.joinedMs === undefined ? 'associating' : 'awaitingConfirmation',
            remainingMs: this.remainingMs(),
          }
        : null,
    };
  }

  /** Start a trial. Validates first, exactly as the device does. */
  startWifiTrial(body: WifiUpdateDto): Outcome {
    this.advance();
    if (this.trial) return { error: 'trialInProgress' };

    const psk = resolve(body.psk, this.stored.wifi?.psk);
    if ('error' in psk) return { error: psk.error, field: 'psk' };

    const refusal = checkCredential(body.ssid, psk.value);
    if (refusal) return refusal;

    this.trial = {
      ssid: body.ssid,
      psk: psk.value,
      startedMs: Date.now(),
      joinedMs: undefined,
    };
    return { ok: true };
  }

  /** Keep the candidate. Only possible once it has joined. */
  confirmWifi(): Outcome {
    this.advance();
    if (!this.trial) return { error: 'noTrialInProgress' };
    if (this.trial.joinedMs === undefined) return { error: 'trialNotAssociated' };
    this.stored.wifi = { ssid: this.trial.ssid, psk: this.trial.psk };
    this.trial = undefined;
    return { ok: true };
  }

  /** Throw the candidate away. The device would restart; the mock just forgets. */
  cancelWifiTrial(): Outcome {
    this.advance();
    if (!this.trial) return { error: 'noTrialInProgress' };
    this.trial = undefined;
    return { ok: true };
  }

  saveMqtt(body: MqttUpdateDto): Outcome {
    const password = resolve(body.password, this.stored.mqtt?.password);
    if ('error' in password) return { error: password.error, field: 'brokerPassword' };

    const refusal = checkMqtt(body, password.value);
    if (refusal) return refusal;

    this.stored.mqtt = {
      address: body.address,
      port: body.port,
      username: body.username,
      password: password.value,
      discoveryPrefix: body.discoveryPrefix,
      stateRoot: body.stateRoot,
    };
    return { ok: true };
  }

  clearMqtt(): Outcome {
    this.stored.mqtt = undefined;
    return { ok: true };
  }

  /** Milliseconds left in the phase in force. */
  private remainingMs(): number {
    if (!this.trial) return 0;
    const { startedMs, joinedMs } = this.trial;
    const [since, deadline] =
      joinedMs === undefined
        ? [startedMs, ASSOCIATE_DEADLINE_MS]
        : [joinedMs, CONFIRM_DEADLINE_MS];
    return Math.max(0, deadline - (Date.now() - since));
  }

  /**
   * Move the trial along, and revert it when a deadline passes.
   *
   * Called on every read and every write rather than from a timer: the mock has
   * no reason to run a clock nobody is watching, and every path into it is one
   * of these.
   */
  private advance(): void {
    if (!this.trial) return;
    const now = Date.now();

    if (this.trial.joinedMs === undefined) {
      if (!this.trial.ssid.includes(NEVER_ASSOCIATES) && now - this.trial.startedMs >= ASSOCIATE_AFTER_MS) {
        this.trial.joinedMs = now;
      } else if (now - this.trial.startedMs > ASSOCIATE_DEADLINE_MS) {
        // The revert. Nothing was stored, so there is nothing to undo — which
        // is the property the whole design rests on.
        this.trial = undefined;
      }
      return;
    }

    if (now - this.trial.joinedMs > CONFIRM_DEADLINE_MS) {
      this.trial = undefined;
    }
  }
}

/** Resolve a write-only secret against what is stored. */
function resolve(
  secret: SecretDto,
  stored: string | undefined,
): { value: string } | { error: ApiErrorCode } {
  switch (secret.secret) {
    case 'keep':
      return stored === undefined ? { error: 'secretNotSet' } : { value: stored };
    case 'set':
      return { value: secret.value };
    case 'clear':
      return { value: '' };
  }
}

/**
 * The Wi-Fi rules, in the order `somfy_config::WifiCredentials::new` applies
 * them: empty, then too long, then too short, then an interior NUL.
 *
 * A second implementation of a rule is exactly what this project's mock is for
 * — the same client code has to meet the same refusals from both sides — and
 * the order matters because it decides *which* refusal a doubly-bad value gets.
 */
function checkCredential(ssid: string, psk: string): Rejection | undefined {
  const ssidBytes = bytes(ssid);
  if (ssidBytes === 0) return { error: 'valueEmpty', field: 'ssid' };
  if (ssidBytes > 32) return { error: 'valueTooLong', field: 'ssid' };
  if (ssid.includes('\0')) return { error: 'valueInteriorNul', field: 'ssid' };

  const pskBytes = bytes(psk);
  if (pskBytes > 64) return { error: 'valueTooLong', field: 'psk' };
  if (pskBytes > 0 && pskBytes < 8) return { error: 'valueTooShort', field: 'psk' };
  if (psk.includes('\0')) return { error: 'valueInteriorNul', field: 'psk' };
  return undefined;
}

/** The broker rules, in `somfy_config::MqttSettings::new`'s order. */
function checkMqtt(body: MqttUpdateDto, password: string): Rejection | undefined {
  const octets = body.address.split('.');
  const numeric = octets.map((part) => Number(part));
  const malformed =
    octets.length !== 4 ||
    octets.some((part) => part.length === 0 || !/^\d+$/.test(part)) ||
    numeric.some((value) => !Number.isInteger(value) || value < 0 || value > 255);
  if (malformed) return { error: 'brokerAddressMalformed', field: 'brokerAddress' };

  const [a, b, c, d] = numeric as [number, number, number, number];
  const unspecified = a === 0 && b === 0 && c === 0 && d === 0;
  const loopback = a === 127;
  const multicast = a >= 224 && a <= 239;
  const broadcast = a === 255 && b === 255 && c === 255 && d === 255;
  if (unspecified || loopback || multicast || broadcast) {
    return { error: 'brokerAddressUnroutable', field: 'brokerAddress' };
  }

  if (body.port === 0) return { error: 'brokerPortZero', field: 'brokerPort' };

  if (bytes(body.username) > 32) return { error: 'valueTooLong', field: 'brokerUsername' };
  if (body.username.includes('\0')) return { error: 'valueInteriorNul', field: 'brokerUsername' };
  if (bytes(password) > 64) return { error: 'valueTooLong', field: 'brokerPassword' };
  if (password.includes('\0')) return { error: 'valueInteriorNul', field: 'brokerPassword' };
  if (body.username.length === 0 && password.length > 0) {
    return { error: 'passwordWithoutUsername', field: 'brokerUsername' };
  }

  for (const [value, field] of [
    [body.discoveryPrefix, 'discoveryPrefix'],
    [body.stateRoot, 'stateRoot'],
  ] as const) {
    if (bytes(value) > 32) return { error: 'valueTooLong', field };
    if (value.includes('\0')) return { error: 'valueInteriorNul', field };
    const refusal = checkNamespace(value, field);
    if (refusal) return refusal;
  }

  if (overlap(body.discoveryPrefix, body.stateRoot)) {
    // Named against the state root, because that is the one to move: the
    // discovery prefix is global to a whole Home Assistant installation.
    return { error: 'namespacesOverlap', field: 'stateRoot' };
  }
  return undefined;
}

/** One namespace's own rules, in `somfy_mqtt::StateRoot::new`'s order. */
function checkNamespace(value: string, field: SettingsFieldDto): Rejection | undefined {
  if (value.length === 0) return { error: 'valueEmpty', field };
  if (value.includes('#') || value.includes('+')) return { error: 'topicWildcard', field };
  if (value.startsWith('/')) return { error: 'topicLeadingSlash', field };
  if (value.endsWith('/')) return { error: 'topicTrailingSlash', field };
  if (value.includes('//')) return { error: 'topicEmptySegment', field };
  if (!/^[A-Za-z0-9_/-]+$/.test(value)) return { error: 'topicIllegalCharacter', field };
  return undefined;
}

/**
 * Whether two namespaces name the same place.
 *
 * **Segment-wise, not textual**: `home` is not inside `homeassistant`, and
 * refusing it would be repair by another name. `somfy_mqtt::namespaces_overlap`
 * draws the boundary in the same place.
 */
function overlap(prefix: string, root: string): boolean {
  if (prefix === root) return true;
  return root.startsWith(`${prefix}/`) || prefix.startsWith(`${root}/`);
}

/** UTF-8 length, because every limit the device enforces is in bytes. */
function bytes(value: string): number {
  return new TextEncoder().encode(value).length;
}
