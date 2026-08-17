/**
 * The `/api/v1/events` WebSocket (design spec §7.2): one socket streaming JSON
 * state deltas, reconnecting with bounded backoff.
 *
 * Messages are typed as the generated {@link WsEvent}, and dispatch is
 * exhaustive on its `ev` tag — when Plan 5 adds a second variant in Rust, this
 * file stops compiling until it is handled, which is the point.
 */

import type { WsEvent } from './generated/WsEvent';

const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 10_000;

export type ConnectionState = 'connecting' | 'open' | 'closed';

export interface EventStreamHandlers {
  onEvent: (event: WsEvent) => void;
  onConnectionChange?: (state: ConnectionState) => void;
}

function eventsUrl(): string {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${location.host}/api/v1/events`;
}

/**
 * Open the event stream. Returns a disposer; call it on unmount.
 *
 * Reconnect is exponential with a ceiling, because the device reboots (OTA,
 * watchdog recovery) are routine and a UI that gives up after one drop is a UI
 * that shows stale positions.
 */
export function connectEvents({ onEvent, onConnectionChange }: EventStreamHandlers): () => void {
  let socket: WebSocket | undefined;
  let retryMs = RECONNECT_MIN_MS;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;

  const setState = (state: ConnectionState) => onConnectionChange?.(state);

  const open = () => {
    if (disposed) return;
    setState('connecting');
    socket = new WebSocket(eventsUrl());

    socket.addEventListener('open', () => {
      retryMs = RECONNECT_MIN_MS;
      setState('open');
    });

    socket.addEventListener('message', (message: MessageEvent<string>) => {
      const parsed = parseEvent(message.data);
      if (parsed) onEvent(parsed);
    });

    socket.addEventListener('close', () => {
      if (disposed) return;
      setState('closed');
      retryTimer = setTimeout(open, retryMs);
      retryMs = Math.min(retryMs * 2, RECONNECT_MAX_MS);
    });

    // `error` is always followed by `close`, which owns the retry.
    socket.addEventListener('error', () => socket?.close());
  };

  open();

  return () => {
    disposed = true;
    if (retryTimer !== undefined) clearTimeout(retryTimer);
    socket?.close();
  };
}

/** Every `ev` tag the generated {@link WsEvent} can carry. */
type EventTag = WsEvent['ev'];

/**
 * The drift gate for the event stream.
 *
 * `Record<EventTag, true>` must name **every** tag in the generated union. When
 * Plan 5 adds a second `WsEvent` variant in Rust and the bindings are
 * regenerated, `EventTag` grows a member this object does not have and
 * `tsc` fails here — so a new firmware event cannot be silently dropped on the
 * floor by a UI that was never told about it.
 */
const KNOWN_EVENT_TAGS: Record<EventTag, true> = {
  shadeState: true,
};

/**
 * Parse one frame. A malformed or unrecognised message is dropped rather than
 * thrown: an event stream that kills the dashboard because the device sent one
 * frame from a newer firmware is worse than one that ignores it.
 */
function parseEvent(raw: string): WsEvent | undefined {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return undefined;
  }
  if (typeof value !== 'object' || value === null) return undefined;
  const tag = (value as { ev?: unknown }).ev;
  if (typeof tag !== 'string' || !(tag in KNOWN_EVENT_TAGS)) return undefined;
  return value as WsEvent;
}
