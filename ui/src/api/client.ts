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

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const response = await request(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  return response.json() as Promise<T>;
}

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
