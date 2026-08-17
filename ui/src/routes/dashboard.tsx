/**
 * The dashboard: rooms → groups → shade tiles (design spec §8).
 *
 * The DTOs do not nest — a `GroupDto` is a set of shade ids and so is a
 * `RoomDto` — so the hierarchy is derived here: a group whose every member
 * lives in one room is shown inside that room, and a group that spans rooms
 * gets its own section above them. Shades in no room are collected at the end
 * rather than dropped, because a shade you cannot see is a shade you cannot
 * fix.
 */
import type { Snapshot } from '../api/client';
import type { GroupDto } from '../api/generated/GroupDto';
import type { RoomDto } from '../api/generated/RoomDto';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { GroupRow } from '../components/group-row';
import { ShadeTile } from '../components/shade-tile';
import { useT } from '../i18n';
import type { DeviceState } from '../state/device';

export function Dashboard({ device }: { device: DeviceState }) {
  const t = useT();

  if (device.error !== undefined) {
    return (
      <section class="panel panel--error">
        <p>{t('dashboard.error', { detail: device.error })}</p>
        <button type="button" class="btn" onClick={device.reload}>
          {t('dashboard.retry')}
        </button>
      </section>
    );
  }

  if (!device.snapshot) {
    return <p class="panel">{t('dashboard.loading')}</p>;
  }

  if (device.snapshot.shades.length === 0) {
    return <p class="panel">{t('dashboard.empty')}</p>;
  }

  const layout = buildLayout(device.snapshot, t('dashboard.unassigned'));

  return (
    <div class="dashboard">
      <h2 class="visually-hidden">{t('dashboard.title')}</h2>

      {layout.crossRoomGroups.length > 0 && (
        <section class="section">
          <div class="section__groups">
            {layout.crossRoomGroups.map((group) => (
              <GroupRow key={group.id} group={group} />
            ))}
          </div>
        </section>
      )}

      {layout.rooms.map((room) => (
        <section class="section" key={room.name}>
          <h2 class="section__title">{room.name}</h2>
          {room.groups.length > 0 && (
            <div class="section__groups">
              {room.groups.map((group) => (
                <GroupRow key={group.id} group={group} />
              ))}
            </div>
          )}
          <div class="grid">
            {room.shades.map((shade) => (
              <ShadeTile key={shade.id} shade={shade} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

interface RoomSection {
  name: string;
  groups: GroupDto[];
  shades: ShadeDto[];
}

interface Layout {
  crossRoomGroups: GroupDto[];
  rooms: RoomSection[];
}

function buildLayout(snapshot: Snapshot, unassignedLabel: string): Layout {
  const shadesById = new Map(snapshot.shades.map((shade) => [shade.id, shade]));
  const roomOfShade = new Map<number, RoomDto>();
  for (const room of snapshot.rooms) {
    for (const shadeId of room.shadeIds) roomOfShade.set(shadeId, room);
  }

  const groupsByRoom = new Map<number, GroupDto[]>();
  const crossRoomGroups: GroupDto[] = [];
  for (const group of snapshot.groups) {
    const roomIds = new Set(group.shadeIds.map((id) => roomOfShade.get(id)?.id));
    const [only] = [...roomIds];
    if (roomIds.size === 1 && only !== undefined) {
      groupsByRoom.set(only, [...(groupsByRoom.get(only) ?? []), group]);
    } else {
      crossRoomGroups.push(group);
    }
  }

  const rooms: RoomSection[] = snapshot.rooms.map((room) => ({
    name: room.name,
    groups: groupsByRoom.get(room.id) ?? [],
    shades: room.shadeIds
      .map((id) => shadesById.get(id))
      .filter((shade): shade is ShadeDto => shade !== undefined),
  }));

  const orphans = snapshot.shades.filter((shade) => !roomOfShade.has(shade.id));
  if (orphans.length > 0) {
    rooms.push({ name: unassignedLabel, groups: [], shades: orphans });
  }

  return { crossRoomGroups, rooms };
}
