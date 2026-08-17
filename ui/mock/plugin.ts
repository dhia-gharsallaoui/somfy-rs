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
import type { CommandDto } from '../src/api/generated/CommandDto.ts';
import type { CreateShadeDto } from '../src/api/generated/CreateShadeDto.ts';
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
};

export function mockApi(): Plugin {
  const world = new World();

  const attach = (server: ViteDevServer | PreviewServer) => {
    server.middlewares.use(restMiddleware(world));
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

function restMiddleware(world: World): Connect.NextHandleFunction {
  return (request, response, next) => {
    const url = new URL(request.url ?? '/', 'http://device.invalid');
    if (!url.pathname.startsWith(API_PREFIX) || url.pathname === EVENTS_PATH) {
      next();
      return;
    }

    const segments = url.pathname.slice(API_PREFIX.length).split('/').filter(Boolean);
    handle(world, request, response, segments).catch((error: unknown) => {
      sendJson(response, 500, { error: String(error) });
    });
  };
}

async function handle(
  world: World,
  request: IncomingMessage,
  response: ServerResponse,
  segments: string[],
): Promise<void> {
  const [collection, rawId, action] = segments;
  const method = request.method ?? 'GET';

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
    return applied
      ? sendNoContent(response)
      : sendJson(response, 404, { error: 'no such target' });
  }

  return sendJson(response, 404, { error: 'no such route' });
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
