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

import type { CommandDto } from './generated/CommandDto';
import type { GroupDto } from './generated/GroupDto';
import type { RoomDto } from './generated/RoomDto';
import type { ShadeDto } from './generated/ShadeDto';

export const API_BASE = '/api/v1';

/** A non-2xx response, carrying whatever the device said about it. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly path: string,
    detail: string,
  ) {
    super(`${path} failed (${status}): ${detail}`);
    this.name = 'ApiError';
  }
}

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
