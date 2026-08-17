/**
 * REST client for the firmware's `/api/v1/` surface (design spec §7.2).
 *
 * Every payload type here is imported from `./generated/`, which is written by
 * `cargo test -p somfy-api --features ts`. Nothing in this file re-declares a
 * wire shape, so a DTO change in Rust lands as a TypeScript error here rather
 * than as a runtime surprise.
 *
 * The same client talks to the mock dev server and to a real device: the mock
 * is mounted at the same paths (see `mock/plugin.ts`), so there is no "mock
 * mode" branch anywhere in the app.
 */

import { ApiError } from './errors';
import type { CommandDto } from './generated/CommandDto';
import type { CreateShadeDto } from './generated/CreateShadeDto';
import type { GroupDto } from './generated/GroupDto';
import type { PatchShadeDto } from './generated/PatchShadeDto';
import type { RoomDto } from './generated/RoomDto';
import type { ShadeDto } from './generated/ShadeDto';

export const API_BASE = '/api/v1';

async function request(path: string, init?: RequestInit): Promise<Response> {
  const response = await fetch(`${API_BASE}${path}`, init);
  if (!response.ok) {
    throw new ApiError(response.status, path, await response.text().catch(() => ''));
  }
  return response;
}

async function getJson<T>(path: string): Promise<T> {
  return (await request(path)).json() as Promise<T>;
}

async function sendJson<T>(method: string, path: string, body: unknown): Promise<T> {
  const response = await request(path, {
    method,
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  return response.json() as Promise<T>;
}

const postJson = <T>(path: string, body: unknown): Promise<T> => sendJson('POST', path, body);

async function postCommand(path: string, command: CommandDto): Promise<void> {
  await request(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(command),
  });
}

export const listShades = (): Promise<ShadeDto[]> => getJson('/shades');
export const listGroups = (): Promise<GroupDto[]> => getJson('/groups');
export const listRooms = (): Promise<RoomDto[]> => getJson('/rooms');

export const getShade = (id: number): Promise<ShadeDto> => getJson(`/shades/${id}`);

export const commandShade = (id: number, command: CommandDto): Promise<void> =>
  postCommand(`/shades/${id}/command`, command);

/**
 * Group commands are per-shade fan-out in v1.0 (README "Group commands stay
 * per-shade fan-out"): the device applies the command to each member, it does
 * not transmit a single group frame.
 */
export const commandGroup = (id: number, command: CommandDto): Promise<void> =>
  postCommand(`/groups/${id}/command`, command);

/**
 * Add a shade. The device assigns the id and allocates the remote address, so
 * the answer — a full {@link ShadeDto} — carries information the request did
 * not, and the caller needs it: the address it just invented is one no motor
 * knows yet.
 */
export const createShade = (body: CreateShadeDto): Promise<ShadeDto> =>
  postJson('/shades', body);

/**
 * Edit a shade that already exists. Fields left out are left unchanged.
 *
 * This is how a measured travel time gets in without an automatic sweep, which
 * the position-accuracy requirements make a MUST (R9): a sweep runs the shade
 * end to end twice per direction, which is not always acceptable, and an
 * operator who already has a stopwatch reading should not have to wait for one.
 * Deleting and re-adding is not an alternative — a re-added shade gets a new
 * address and has to be paired again at the window.
 *
 * Answers with the whole shade, because the calibration sources are recomputed
 * from the values and the caller needs the new ones.
 */
export const patchShade = (id: number, body: PatchShadeDto): Promise<ShadeDto> =>
  sendJson('PATCH', `/shades/${id}`, body);

/**
 * Remove a shade from this controller.
 *
 * **The motor is not told, and cannot be.** RTS has no "forget this remote"
 * that a controller may send safely — on a physical remote it is a *held* PROG
 * press, and the length of the burst is the only thing distinguishing it from a
 * pairing tap, so getting it wrong unpairs a working shade and costs a walk to
 * the window. The firmware therefore offers no unpair command at all
 * (`somfy_domain::PAIR_REPEATS` pins the burst to a tap), and neither does
 * this. Deleting here removes the controller's knowledge of the shade; the
 * motor keeps obeying every remote it has already learned.
 *
 * **It is also how a half-finished setup is abandoned**, and there it really
 * does leave nothing behind: a shade that was never confirmed has no Home
 * Assistant entities to orphan, so the device publishes nothing at all. The one
 * thing that survives is the address's rolling code, which is correct — a
 * counter that went backwards is what stops a motor obeying, and if the same
 * address is allocated again its code continues upward from where it was.
 */
export const deleteShade = (id: number): Promise<void> =>
  request(`/shades/${id}`, { method: 'DELETE' }).then(() => undefined);

/**
 * Ask the device to transmit a pairing frame at this shade's address.
 *
 * Resolving means **202 Accepted** — the request was taken — and nothing more.
 * It is not a report that the motor was paired, because no such report exists:
 * RTS is one-way and the controller never hears back. The only acknowledgement
 * is the motor jogging, and the only observer is a person standing at it. A UI
 * that renders this promise's resolution as success is lying, so the pairing
 * assistant asks the user what happened instead.
 */
export const pairShade = (id: number): Promise<void> =>
  request(`/shades/${id}/pair`, { method: 'POST' }).then(() => undefined);

/**
 * Tell the device that an operator commanded this shade and watched it move.
 *
 * **This is the only thing that gives a shade Home Assistant entities.** A
 * created shade has an address the device invented, which no motor has ever
 * heard, so announcing it would put a cover in Home Assistant that accepts
 * commands and drives nothing. The device therefore announces on this call and
 * on no other.
 *
 * Note what is being reported and what is not. The device cannot observe
 * pairing — RTS is one-way — so this carries no claim about the motor. It
 * carries a claim about a *person*: they pressed Open or Close, the shade
 * responded, and they said so. That is why the flow ends with a functional test
 * rather than with the jog: a jog proves a frame arrived, and moving proves the
 * path the user will actually use works end to end.
 *
 * Answers with the whole shade, because its `pairingState` has changed and the
 * UI must stop presenting it as an unfinished setup.
 */
export async function confirmPairing(id: number): Promise<ShadeDto> {
  // No body, so no `content-type` either: there is nothing to vary. The one
  // thing a payload could have carried is "unconfirmed", and that direction
  // would retire the entities of a working shade.
  const response = await request(`/shades/${id}/confirm-pairing`, { method: 'POST' });
  return response.json() as Promise<ShadeDto>;
}

/** Everything the dashboard needs, fetched in parallel. */
export interface Snapshot {
  shades: ShadeDto[];
  groups: GroupDto[];
  rooms: RoomDto[];
}

export async function loadSnapshot(): Promise<Snapshot> {
  const [shades, groups, rooms] = await Promise.all([listShades(), listGroups(), listRooms()]);
  return { shades, groups, rooms };
}
