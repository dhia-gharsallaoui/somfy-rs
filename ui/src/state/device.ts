/**
 * The device's live state: one REST snapshot on mount, then the WebSocket
 * stream applied on top of it.
 *
 * A single hook owns this because the dashboard and the shade-detail screen
 * must never disagree about where a shade is. `shadeState` events carry only
 * `position`, `tiltPosition` and `direction` — the fields that move — so they
 * are merged into the snapshot rather than replacing it.
 */
import { useCallback, useEffect, useMemo, useState } from 'preact/hooks';

import { loadSnapshot, type Snapshot } from '../api/client';
import { ApiError } from '../api/errors';
import { connectEvents, type ConnectionState } from '../api/events';
import type { ShadeDto } from '../api/generated/ShadeDto';

export interface DeviceState {
  snapshot: Snapshot | undefined;
  connection: ConnectionState;
  error: string | undefined;
  reload: () => void;
  shade: (id: number) => ShadeDto | undefined;
}

export function useDevice(): DeviceState {
  const [snapshot, setSnapshot] = useState<Snapshot | undefined>(undefined);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [error, setError] = useState<string | undefined>(undefined);
  const [reloadToken, setReloadToken] = useState(0);

  const reload = useCallback(() => setReloadToken((token) => token + 1), []);

  useEffect(() => {
    let cancelled = false;
    setError(undefined);
    loadSnapshot()
      .then((next) => {
        if (!cancelled) setSnapshot(next);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setError(cause instanceof ApiError ? cause.message : String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [reloadToken]);

  useEffect(
    () =>
      connectEvents({
        onConnectionChange: setConnection,
        onEvent: (event) => {
          // One variant today; `src/api/events.ts` holds the gate that makes a
          // second one a compile error rather than a dropped message.
          setSnapshot((current) => (current ? applyShadeState(current, event) : current));
        },
      }),
    [],
  );

  const byId = useMemo(() => {
    const map = new Map<number, ShadeDto>();
    for (const shade of snapshot?.shades ?? []) map.set(shade.id, shade);
    return map;
  }, [snapshot]);

  const shade = useCallback((id: number) => byId.get(id), [byId]);

  return { snapshot, connection, error, reload, shade };
}

interface ShadeStateFields {
  id: number;
  position: number;
  tiltPosition: number;
  direction: number;
}

function applyShadeState(snapshot: Snapshot, event: ShadeStateFields): Snapshot {
  let changed = false;
  const shades = snapshot.shades.map((shade) => {
    if (shade.id !== event.id) return shade;
    if (
      shade.position === event.position &&
      shade.tiltPosition === event.tiltPosition &&
      shade.direction === event.direction
    ) {
      return shade;
    }
    changed = true;
    return {
      ...shade,
      position: event.position,
      tiltPosition: event.tiltPosition,
      direction: event.direction,
    };
  });
  return changed ? { ...snapshot, shades } : snapshot;
}
