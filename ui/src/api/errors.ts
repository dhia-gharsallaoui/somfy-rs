/**
 * The UI half of the typed-rejection contract.
 *
 * The device answers a refused request with `{"code":"nameTooLong"}` and no
 * prose, because it ships no French and a permanently-English sentence on a
 * French screen is not something the UI can fix afterwards
 * (`crates/somfy-api/src/errors.rs` carries the argument). This file is where
 * the code becomes a sentence, in whichever language is selected.
 *
 * {@link ERROR_MESSAGE} is **total** over the generated {@link ApiErrorCode}.
 * That is the drift gate: a rejection added in Rust and regenerated makes the
 * union grow a member this record does not have, and `bun run typecheck` fails
 * until it is translated — in both catalogues, because `fr.ts` is itself a
 * total `Record<MessageKey, string>`. A new failure mode therefore cannot reach
 * a user as a blank space or an English word.
 *
 * {@link ApiError} lives here rather than in `client.ts` so that the class and
 * the code it carries are declared together, and so the two files do not have
 * to import each other.
 */
import type { MessageKey } from '../i18n/en';
import type { ApiErrorCode } from './generated/ApiErrorCode';
import type { SettingsFieldDto } from './generated/SettingsFieldDto';

export const ERROR_MESSAGE: Record<ApiErrorCode, MessageKey> = {
  nameEmpty: 'error.nameEmpty',
  nameTooLong: 'error.nameTooLong',
  invalidKind: 'error.invalidKind',
  invalidTiltMode: 'error.invalidTiltMode',
  travelTimeZero: 'error.travelTimeZero',
  invalidAddress: 'error.invalidAddress',
  registryFull: 'error.registryFull',
  notFound: 'error.notFound',
  addressNotAllocated: 'error.addressNotAllocated',
  invalidDeadBand: 'error.invalidDeadBand',
  ventBandNotMeasured: 'error.ventBandNotMeasured',
  notCalibrating: 'error.notCalibrating',
  calibrationImplausible: 'error.calibrationImplausible',
  commandNotAtThisWidth: 'error.commandNotAtThisWidth',
  // Settings. Each of these is a *rule*; the field it broke arrives beside it
  // as `ApiErrorDto.field`, so the sentences below are written to read with a
  // field name interpolated into them — `{field}` — and the form highlights the
  // input at the same time. See `crates/somfy-api/src/errors.rs` for why the
  // two are separate axes.
  valueEmpty: 'error.valueEmpty',
  valueTooLong: 'error.valueTooLong',
  valueTooShort: 'error.valueTooShort',
  valueInteriorNul: 'error.valueInteriorNul',
  brokerAddressMalformed: 'error.brokerAddressMalformed',
  brokerAddressUnroutable: 'error.brokerAddressUnroutable',
  brokerPortZero: 'error.brokerPortZero',
  passwordWithoutUsername: 'error.passwordWithoutUsername',
  topicWildcard: 'error.topicWildcard',
  topicLeadingSlash: 'error.topicLeadingSlash',
  topicTrailingSlash: 'error.topicTrailingSlash',
  topicEmptySegment: 'error.topicEmptySegment',
  topicIllegalCharacter: 'error.topicIllegalCharacter',
  namespacesOverlap: 'error.namespacesOverlap',
  secretNotSet: 'error.secretNotSet',
  noTrialInProgress: 'error.noTrialInProgress',
  trialInProgress: 'error.trialInProgress',
  trialNotAssociated: 'error.trialNotAssociated',
  settingsUnwritable: 'error.settingsUnwritable',
};

/**
 * The operator-facing name of each settings field.
 *
 * Total over the generated {@link SettingsFieldDto}, so a field added in Rust
 * fails `tsc` here rather than rendering as a blank in the middle of a
 * sentence. The same keys label the inputs on the settings screen, so the
 * message and the field it points at cannot disagree.
 */
export const FIELD_LABEL: Record<SettingsFieldDto, MessageKey> = {
  ssid: 'settings.wifiSsid',
  psk: 'settings.wifiPsk',
  brokerAddress: 'settings.mqttAddress',
  brokerPort: 'settings.mqttPort',
  brokerUsername: 'settings.mqttUsername',
  brokerPassword: 'settings.mqttPassword',
  discoveryPrefix: 'settings.mqttDiscoveryPrefix',
  stateRoot: 'settings.mqttStateRoot',
};

/**
 * A non-2xx response, carrying whatever the device said about it.
 *
 * `code` is the typed rejection when the device sent one and `undefined` when
 * it did not — a transport failure, something in the way, or a firmware newer
 * than this UI. Screens branch on it to name what is actually wrong, and fall
 * back to {@link errorMessageKey}'s generic answer otherwise.
 */
export class ApiError extends Error {
  readonly code: ApiErrorCode | undefined;
  /**
   * Which settings value the rejection is about, when it is about one.
   *
   * Absent for every rejection that names no field, which is all of them
   * outside the settings screen. A form uses it to highlight the input the
   * operator has to fix — spec R3 asks that an invalid value be refused "with
   * the field named", and pointing at it is that requirement kept rather than
   * described.
   */
  readonly field: SettingsFieldDto | undefined;

  constructor(
    readonly status: number,
    readonly path: string,
    detail: string,
  ) {
    super(`${path} failed (${status}): ${detail}`);
    this.name = 'ApiError';
    this.code = parseApiErrorCode(detail);
    this.field = parseApiErrorField(detail);
  }
}

/**
 * Pull the code out of an error body.
 *
 * Deliberately forgiving: an unparseable body, or one carrying a code from a
 * newer firmware, yields `undefined` and the caller falls back to a generic
 * message. The alternative — throwing while handling an error — would replace a
 * useful message with a worse one.
 */
export function parseApiErrorCode(body: string): ApiErrorCode | undefined {
  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return undefined;
  }
  if (typeof value !== 'object' || value === null) return undefined;
  const code = (value as { code?: unknown }).code;
  if (typeof code !== 'string' || !(code in ERROR_MESSAGE)) return undefined;
  return code as ApiErrorCode;
}

/**
 * Pull the settings field out of an error body.
 *
 * As forgiving as {@link parseApiErrorCode}, and for the same reason: a body
 * that does not name a field is the ordinary case, not a fault.
 */
export function parseApiErrorField(body: string): SettingsFieldDto | undefined {
  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return undefined;
  }
  if (typeof value !== 'object' || value === null) return undefined;
  const field = (value as { field?: unknown }).field;
  if (typeof field !== 'string' || !(field in FIELD_LABEL)) return undefined;
  return field as SettingsFieldDto;
}

/**
 * What to tell the user about a failed request.
 *
 * Falls back to "the device did not say why", which is both true and more
 * useful than inventing a cause — a screen claiming the name was too long when
 * the Wi-Fi dropped sends somebody to edit a field that was fine.
 */
export function errorMessageKey(cause: unknown): MessageKey {
  return cause instanceof ApiError && cause.code ? ERROR_MESSAGE[cause.code] : 'error.unknown';
}
