/**
 * Vite plugin serving a fake `/api/v1/` REST + WebSocket device (design spec
 * §8, "mock-driven development"), so UI work needs zero hardware.
 *
 * ## How this cannot drift from the firmware
 *
 * Every payload it reads or writes is one of the `ts-rs`-generated types under
 * `src/api/generated/`, regenerated from `crates/somfy-api` by
 * `cargo test -p somfy-api --features ts` and gated in CI by
 * `git diff --exit-code ui/src/api/generated`. Nothing here re-declares a wire
 * shape, so the chain is: change a Rust DTO → regenerate → the mock either
 * still typechecks or `bun run typecheck` fails.
 *
 * Four of those checks are load-bearing rather than incidental:
 *
 * 1. `World.command`'s `switch` is exhaustive over `CommandDto` and ends in
 *    `assertNever`, so a new firmware action cannot be silently ignored.
 * 2. {@link KNOWN_ACTIONS} is a total `Record<CommandAction, true>`, so a new
 *    action must also be admitted by the request parser.
 * 3. The fixtures are typed `ShadeDto[]` / `RoomDto[]` / `GroupDto[]`, so a
 *    renamed or added field is a compile error in the data, not a `undefined`
 *    in the browser.
 * 4. {@link ERROR_STATUS} is a total `Record<ApiErrorCode, number>`, so a
 *    rejection added in Rust must be given an HTTP status here — a new code
 *    cannot quietly become a 500. (`src/api/errors.ts` holds the other half:
 *    the same code must also be given a translated message.)
 *
 * The plugin is mounted on both the dev server and `vite preview`, so the
 * production bundle can be exercised against the same fake device.
 */
import type { IncomingMessage, ServerResponse } from 'node:http';
import type { Duplex } from 'node:stream';

import type { Connect, Plugin, PreviewServer, ViteDevServer } from 'vite';
import { WebSocketServer, type WebSocket } from 'ws';

import type { ApiErrorCode } from '../src/api/generated/ApiErrorCode.ts';
import type { ApiErrorDto } from '../src/api/generated/ApiErrorDto.ts';
import type { CalibrationStepDto } from '../src/api/generated/CalibrationStepDto.ts';
import type { CommandDto } from '../src/api/generated/CommandDto.ts';
import type { CreateShadeDto } from '../src/api/generated/CreateShadeDto.ts';
import type { MqttUpdateDto } from '../src/api/generated/MqttUpdateDto.ts';
import type { PatchShadeDto } from '../src/api/generated/PatchShadeDto.ts';
import type { SecretDto } from '../src/api/generated/SecretDto.ts';
import type { TrialDecisionDto } from '../src/api/generated/TrialDecisionDto.ts';
import type { WifiUpdateDto } from '../src/api/generated/WifiUpdateDto.ts';
import { Settings, type Rejection } from './settings.ts';
import { World } from './world.ts';

const API_PREFIX = '/api/v1';
const EVENTS_PATH = `${API_PREFIX}/events`;

/** Every action tag the generated {@link CommandDto} union carries. */
type CommandAction = CommandDto['action'];

/**
 * The request-parser half of the drift gate. Adding an action in Rust makes
 * `CommandAction` grow a member this record does not have, and `tsc` fails.
 */
const KNOWN_ACTIONS: Record<CommandAction, true> = {
  up: true,
  down: true,
  my: true,
  stepUp: true,
  stepDown: true,
  goTo: true,
  setMy: true,
  vent: true,
};

/**
 * How each rejection reaches the client. Total over {@link ApiErrorCode}, so a
 * code added in Rust and regenerated fails `tsc` here until somebody decides
 * what it means over HTTP.
 *
 * Two of these choices are worth defending:
 *
 * - **`registryFull` is 409, not 507.** The device is not out of storage in any
 *   sense the client can wait out; it is at its shade limit, and the fix is to
 *   remove a shade. 409 says "the state of this collection conflicts with what
 *   you asked", which is exactly the situation.
 * - **`addressNotAllocated` is 409, not 400.** The request is perfectly
 *   well-formed. What makes it inapplicable is a property of the shade — its
 *   address belongs to another controller — so it is a conflict with resource
 *   state rather than a malformed body, and a UI that highlighted a form field
 *   over it would be pointing at nothing.
 * - **`ventBandNotMeasured` is 409 for the same reason.** `{"action":"vent"}` is
 *   a well-formed request; what makes it inapplicable is that the shade's
 *   slat-separation band has never been measured, and the vent position *is*
 *   that number.
 * - **`notCalibrating` is 409, not 400.** Marking or finishing a run that is not
 *   running is a conflict with the shade's state, not a bad body — and it is
 *   what a stale browser tab produces, so it must not read as a client bug.
 */
const ERROR_STATUS: Record<ApiErrorCode, number> = {
  nameEmpty: 400,
  nameTooLong: 400,
  invalidKind: 400,
  invalidTiltMode: 400,
  travelTimeZero: 400,
  invalidAddress: 500,
  registryFull: 409,
  notFound: 404,
  addressNotAllocated: 409,
  invalidDeadBand: 400,
  ventBandNotMeasured: 409,
  notCalibrating: 409,
  calibrationImplausible: 400,
  commandNotAtThisWidth: 409,
  // Settings. Every validation refusal is a 400 and the two trial-state ones
  // are 409, matching `ApiErrorCode::http_status` — the argument for each is
  // there, beside the variant.
  valueEmpty: 400,
  valueTooLong: 400,
  valueTooShort: 400,
  valueInteriorNul: 400,
  brokerAddressMalformed: 400,
  brokerAddressUnroutable: 400,
  brokerPortZero: 400,
  passwordWithoutUsername: 400,
  topicWildcard: 400,
  topicLeadingSlash: 400,
  topicTrailingSlash: 400,
  topicEmptySegment: 400,
  topicIllegalCharacter: 400,
  namespacesOverlap: 400,
  secretNotSet: 400,
  noTrialInProgress: 409,
  trialInProgress: 409,
  trialNotAssociated: 409,
  settingsUnwritable: 500,
};

export function mockApi(): Plugin {
  const world = new World();
  const settings = new Settings();

  const attach = (server: ViteDevServer | PreviewServer) => {
    server.middlewares.use(restMiddleware(world, settings));
    server.httpServer?.on('upgrade', upgradeHandler(world));
  };

  return {
    name: 'somfy-rs:mock-api',
    apply: () => true,
    configureServer: attach,
    configurePreviewServer: attach,
  };
}

// ---------------------------------------------------------------------------
// REST
// ---------------------------------------------------------------------------

function restMiddleware(world: World, settings: Settings): Connect.NextHandleFunction {
  return (request, response, next) => {
    const url = new URL(request.url ?? '/', 'http://device.invalid');
    if (!url.pathname.startsWith(API_PREFIX) || url.pathname === EVENTS_PATH) {
      next();
      return;
    }

    const segments = url.pathname.slice(API_PREFIX.length).split('/').filter(Boolean);
    handle(world, settings, request, response, segments).catch((error: unknown) => {
      sendJson(response, 500, { error: String(error) });
    });
  };
}

async function handle(
  world: World,
  settings: Settings,
  request: IncomingMessage,
  response: ServerResponse,
  segments: string[],
): Promise<void> {
  const [collection, rawId, action] = segments;
  const method = request.method ?? 'GET';

  if (collection === 'settings') {
    return handleSettings(settings, request, response, segments, method);
  }

  if (segments.length === 1 && collection === 'shades' && method === 'POST') {
    const body = parseCreateShade(await readJson(request));
    if (!body) return sendJson(response, 400, { error: 'malformed body' });

    const created = world.createShade(body);
    if ('error' in created) return sendError(response, created.error);
    // 201 + Location, because a create that answered 200 with a body would
    // leave the client to guess the id out of it.
    response.setHeader('location', `${API_PREFIX}/shades/${created.ok.id}`);
    return sendJson(response, 201, created.ok);
  }

  if (method === 'GET' && segments.length === 1) {
    switch (collection) {
      case 'shades':
        return sendJson(response, 200, world.listShades());
      case 'groups':
        return sendJson(response, 200, world.listGroups());
      case 'rooms':
        return sendJson(response, 200, world.listRooms());
      default:
        return sendJson(response, 404, { error: 'no such collection' });
    }
  }

  const id = Number(rawId);
  if (!Number.isInteger(id)) return sendJson(response, 404, { error: 'bad id' });

  if (method === 'GET' && collection === 'shades' && segments.length === 2) {
    const shade = world.getShade(id);
    return shade
      ? sendJson(response, 200, shade)
      : sendJson(response, 404, { error: 'no such shade' });
  }

  if (method === 'PATCH' && collection === 'shades' && segments.length === 2) {
    const body = parsePatchShade(await readJson(request));
    if (!body) return sendJson(response, 400, { error: 'malformed body' });

    const patched = world.patchShade(id, body);
    return 'error' in patched
      ? sendError(response, patched.error)
      : // 200 with the whole shade, not 204: the client needs the recomputed
        // calibration sources back, and a PATCH that answered "no content"
        // would make the UI guess at them.
        sendJson(response, 200, patched.ok);
  }

  if (method === 'DELETE' && collection === 'shades' && segments.length === 2) {
    return world.deleteShade(id) ? sendNoContent(response) : sendError(response, 'notFound');
  }

  if (method === 'POST' && action === 'pair') {
    // Shades only. `Controller::command_group` refuses a group `Pair` with
    // `NotAGroupCommand`, so there is deliberately no `/groups/{id}/pair` to
    // fall through to: fanned across a group it is a `Prog` burst at every
    // shade in the house with nobody standing at any of them.
    if (collection !== 'shades') return sendJson(response, 404, { error: 'no such route' });

    const result = world.pairShade(id);
    if (result !== 'accepted') return sendError(response, result.error);
    // 202, never 200. The device has queued a `Prog` burst and will never learn
    // whether the motor took it — the only acknowledgement in this protocol is
    // the shade jogging, watched by a person standing at it.
    response.statusCode = 202;
    return void response.end();
  }

  if (method === 'POST' && action === 'confirm-pairing') {
    if (collection !== 'shades') return sendJson(response, 404, { error: 'no such route' });

    const confirmed = world.confirmPairing(id);
    // 200 with the whole shade, not 204: `pairingState` has changed and the UI
    // has to stop presenting the shade as an unfinished setup. Unlike `/pair`
    // this may honestly say 200 — the device recorded the report and published
    // the entities before answering, and neither of those is a claim about a
    // motor.
    return 'error' in confirmed
      ? sendError(response, confirmed.error)
      : sendJson(response, 200, confirmed.ok);
  }

  if (method === 'POST' && action === 'command') {
    const command = parseCommand(await readJson(request));
    if (!command) return sendJson(response, 400, { error: 'malformed command' });

    const applied =
      collection === 'shades'
        ? world.command(id, command)
        : collection === 'groups'
          ? world.commandGroup(id, command)
          : undefined;

    if (applied === undefined) return sendJson(response, 404, { error: 'no such collection' });
    if (applied) return sendNoContent(response);
    // A shade that exists refuses a vent only when its slat-separation band has
    // never been measured, which is the one refusal this route has that is not
    // "no such target".
    return collection === 'shades' && command.action === 'vent' && world.getShade(id) !== undefined
      ? sendError(response, 'ventBandNotMeasured')
      : sendJson(response, 404, { error: 'no such target' });
  }

  if (method === 'POST' && collection === 'shades' && action === 'calibrate') {
    const step = parseCalibrationStep(await readJson(request));
    if (!step) return sendJson(response, 400, { error: 'malformed calibration step' });
    const outcome = world.calibrate(id, step);
    return 'error' in outcome ? sendError(response, outcome.error) : sendNoContent(response);
  }

  return sendJson(response, 404, { error: 'no such route' });
}

/**
 * The settings surface: four paths, all plain literals, exactly as the
 * firmware's router declares them.
 *
 * Answers `202` where the device answers `202`, and the distinction carries
 * meaning on both: a Wi-Fi trial has been *accepted*, not applied, and a broker
 * save is followed by a restart. A mock that answered `200` would let a screen
 * ship that treated either as finished.
 */
async function handleSettings(
  settings: Settings,
  request: IncomingMessage,
  response: ServerResponse,
  segments: string[],
  method: string,
): Promise<void> {
  const [, half, action] = segments;

  if (segments.length === 1 && method === 'GET') {
    return sendJson(response, 200, settings.read());
  }

  if (half === 'wifi' && action === undefined && method === 'PUT') {
    const body = parseWifiUpdate(await readJson(request));
    if (!body) return sendJson(response, 400, { error: 'malformed body' });
    return settle(response, settings.startWifiTrial(body), 202);
  }

  if (half === 'wifi' && action === 'trial' && method === 'POST') {
    const decision = parseTrialDecision(await readJson(request));
    if (!decision) return sendJson(response, 400, { error: 'malformed body' });
    return decision.decision === 'confirm'
      ? settle(response, settings.confirmWifi(), 204)
      : settle(response, settings.cancelWifiTrial(), 202);
  }

  if (half === 'mqtt' && action === undefined && method === 'PUT') {
    const body = parseMqttUpdate(await readJson(request));
    if (!body) return sendJson(response, 400, { error: 'malformed body' });
    return settle(response, settings.saveMqtt(body), 202);
  }

  if (half === 'mqtt' && action === undefined && method === 'DELETE') {
    return settle(response, settings.clearMqtt(), 202);
  }

  return sendJson(response, 404, { error: 'no such route' });
}

/** A settings outcome, as a response. */
function settle(
  response: ServerResponse,
  outcome: { ok: true } | Rejection,
  status: number,
): void {
  if ('error' in outcome) return sendRejection(response, outcome);
  response.statusCode = status;
  return void response.end();
}

/**
 * Read a write-only secret.
 *
 * An **absent** `secret` is a malformed body rather than a guess, exactly as
 * `somfy_api::SecretDto`'s hand-written `Deserialize` treats it: "keep" and
 * "clear" are the two things an omission might have meant, and picking between
 * them silently is the whole thing this type exists to prevent.
 */
function parseSecret(value: unknown): SecretDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const { secret, value: text } = value as { secret?: unknown; value?: unknown };
  switch (secret) {
    case 'keep':
      return { secret: 'keep' };
    case 'clear':
      return { secret: 'clear' };
    case 'set':
      return typeof text === 'string' ? { secret: 'set', value: text } : undefined;
    default:
      return undefined;
  }
}

/**
 * Read a trial decision.
 *
 * The `KNOWN_*` records elsewhere in this file are total over a generated
 * union; here the union is two fieldless variants, so the switch *is* the
 * totality check — a third decision added in Rust makes `TrialDecisionDto`
 * grow a member and this function stops returning it, which `tsc` catches at
 * the call site.
 */
function parseTrialDecision(value: unknown): TrialDecisionDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const { decision } = value as { decision?: unknown };
  if (decision === 'confirm') return { decision: 'confirm' };
  if (decision === 'cancel') return { decision: 'cancel' };
  return undefined;
}

function parseWifiUpdate(value: unknown): WifiUpdateDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const { ssid, psk } = value as { ssid?: unknown; psk?: unknown };
  if (typeof ssid !== 'string') return undefined;
  const secret = parseSecret(psk);
  return secret ? { ssid, psk: secret } : undefined;
}

function parseMqttUpdate(value: unknown): MqttUpdateDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const body = value as Record<string, unknown>;
  const { address, port, username, discoveryPrefix, stateRoot } = body;
  if (
    typeof address !== 'string' ||
    typeof port !== 'number' ||
    !Number.isInteger(port) ||
    port < 0 ||
    port > 65535 ||
    typeof username !== 'string' ||
    typeof discoveryPrefix !== 'string' ||
    typeof stateRoot !== 'string'
  ) {
    return undefined;
  }
  const password = parseSecret(body['password']);
  return password
    ? { address, port, username, password, discoveryPrefix, stateRoot }
    : undefined;
}

/**
 * Validate an inbound command the way the firmware does.
 *
 * `somfy-api`'s hand-written `Deserialize` (`commands.rs`) rejects a `goTo`
 * with no `position` — "a missing one is a malformed request, not a silent
 * default" — while `setMy` treats a missing or null position as "clear the
 * favourite". Reproducing that here is what stops the UI shipping a request
 * shape the real device would refuse.
 */
function parseCommand(value: unknown): CommandDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const { action, position } = value as { action?: unknown; position?: unknown };
  if (typeof action !== 'string' || !(action in KNOWN_ACTIONS)) return undefined;

  switch (action as CommandAction) {
    case 'goTo':
      return typeof position === 'number' ? { action: 'goTo', position } : undefined;
    case 'setMy':
      if (position === undefined || position === null) return { action: 'setMy', position: null };
      return typeof position === 'number' ? { action: 'setMy', position } : undefined;
    default:
      return { action: action as Exclude<CommandAction, 'goTo' | 'setMy'> };
  }
}

/** Every step tag the generated {@link CalibrationStepDto} union carries. */
type CalibrationStep = CalibrationStepDto['step'];

/**
 * The same drift gate as {@link KNOWN_ACTIONS}, for the calibration
 * conversation: a step added in Rust grows this union and fails `tsc` here.
 */
const KNOWN_STEPS: Record<CalibrationStep, true> = {
  begin: true,
  mark: true,
  finish: true,
  cancel: true,
};

/**
 * Shape-check an inbound calibration step, as `somfy-api`'s hand-written
 * `Deserialize` does: `begin` without a leg and `mark` without a mark are
 * malformed rather than defaulted — guessing a direction would drive a shade the
 * wrong way across its whole range.
 */
function parseCalibrationStep(value: unknown): CalibrationStepDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const { step, leg, mark } = value as { step?: unknown; leg?: unknown; mark?: unknown };
  if (typeof step !== 'string' || !(step in KNOWN_STEPS)) return undefined;

  switch (step as CalibrationStep) {
    case 'begin':
      return leg === 'up' || leg === 'down' ? { step: 'begin', leg } : undefined;
    case 'mark':
      return mark === 'motionBegan' || mark === 'curtainMoved' ? { step: 'mark', mark } : undefined;
    default:
      return { step: step as Exclude<CalibrationStep, 'begin' | 'mark'> };
  }
}

/**
 * Shape-check a create body — the JSON-parsing half only.
 *
 * The split mirrors Rust: serde decides whether the bytes *are* a
 * `CreateShadeDto`, and `to_config` then decides whether that shade may exist.
 * So a missing field or a string where a number belongs is a malformed request
 * (400, no code), while a name that is too long or a kind the firmware does not
 * model is a *typed* rejection from `mock/validate.ts`. Collapsing the two
 * would mean the UI could not tell "your JSON is broken" from "your name is
 * three characters too long", and only one of those is the user's to fix.
 */
function parseCreateShade(value: unknown): CreateShadeDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const body = value as Record<string, unknown>;
  if (typeof body['name'] !== 'string') return undefined;

  const numbers = ['kind', 'tiltMode', 'upTimeMs', 'downTimeMs', 'tiltTimeMs'] as const;
  for (const field of numbers) {
    const candidate = body[field];
    if (typeof candidate !== 'number' || !Number.isInteger(candidate) || candidate < 0) {
      return undefined;
    }
  }

  return {
    name: body['name'],
    kind: body['kind'] as number,
    tiltMode: body['tiltMode'] as number,
    upTimeMs: body['upTimeMs'] as number,
    downTimeMs: body['downTimeMs'] as number,
    tiltTimeMs: body['tiltTimeMs'] as number,
  };
}

/**
 * Shape-check a patch body.
 *
 * The distinction that matters here is **absent versus present**: an omitted
 * field means "leave it alone", so the parser must not fill in defaults. A
 * field that is present but the wrong type is still malformed, exactly as in
 * the create parser — and unknown keys are ignored rather than refused, which
 * is what keeps a client sending back a whole `ShadeDto` (address, id and all)
 * from being rejected outright while still not being able to change them.
 */
function parsePatchShade(value: unknown): PatchShadeDto | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const body = value as Record<string, unknown>;
  const patch: PatchShadeDto = {};

  if ('name' in body && body['name'] !== undefined) {
    if (typeof body['name'] !== 'string') return undefined;
    patch.name = body['name'];
  }

  const numbers = ['kind', 'tiltMode', 'upTimeMs', 'downTimeMs', 'tiltTimeMs'] as const;
  for (const field of numbers) {
    if (!(field in body) || body[field] === undefined) continue;
    const candidate = body[field];
    if (typeof candidate !== 'number' || !Number.isInteger(candidate) || candidate < 0) {
      return undefined;
    }
    patch[field] = candidate;
  }

  return patch;
}

async function readJson(request: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  if (chunks.length === 0) return undefined;
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch {
    return undefined;
  }
}

function sendJson(response: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  response.statusCode = status;
  response.setHeader('content-type', 'application/json; charset=utf-8');
  response.setHeader('cache-control', 'no-store');
  response.end(payload);
}

function sendNoContent(response: ServerResponse): void {
  response.statusCode = 204;
  response.end();
}

/** A typed rejection: the status from {@link ERROR_STATUS}, the code as body. */
function sendError(response: ServerResponse, code: ApiErrorCode): void {
  const body: ApiErrorDto = { code };
  sendJson(response, ERROR_STATUS[code], body);
}

/**
 * The same, plus the settings field it is about.
 *
 * The field is **omitted** when there is none rather than sent as `null`, which
 * is what `#[serde(skip_serializing_if)]` does on the device — so the bytes a
 * screen parses are the same from both.
 */
function sendRejection(response: ServerResponse, rejection: Rejection): void {
  const body: ApiErrorDto = rejection.field
    ? { code: rejection.error, field: rejection.field }
    : { code: rejection.error };
  sendJson(response, ERROR_STATUS[rejection.error], body);
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------

/**
 * `noServer: true` plus a path check: Vite runs its own HMR WebSocket on the
 * same HTTP server, so we must claim only `/api/v1/events` and leave every
 * other upgrade for Vite's handler.
 */
function upgradeHandler(world: World) {
  const wss = new WebSocketServer({ noServer: true });

  wss.on('connection', (socket: WebSocket) => {
    const send = (event: unknown) => {
      if (socket.readyState === socket.OPEN) socket.send(JSON.stringify(event));
    };
    // A freshly connected client has no state; hand it the whole world once,
    // then stream deltas — the same contract a real device owes a page reload.
    for (const event of world.snapshotEvents()) send(event);
    const unsubscribe = world.subscribe(send);
    socket.on('close', unsubscribe);
    socket.on('error', unsubscribe);
  });

  return (request: IncomingMessage, socket: Duplex, head: Buffer) => {
    const url = new URL(request.url ?? '/', 'http://device.invalid');
    if (url.pathname !== EVENTS_PATH) return;
    wss.handleUpgrade(request, socket, head, (client) => wss.emit('connection', client, request));
  };
}
