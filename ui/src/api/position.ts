/**
 * The one place the shade-position convention is written down.
 *
 * # Two conventions, opposite directions
 *
 * **The wire is Somfy's.** `somfy-domain`'s `Pos` (`crates/somfy-domain/src/types.rs`)
 * is documented as "0 = fully up/open, 10000 = fully closed", and `somfy-api`
 * narrows that to whole percent, so every `position`, `target`,
 * `tiltPosition` and `myPosition` on `ShadeDto`/`ShadeStateEvent` means
 * **0 = fully open, 100 = fully closed**.
 *
 * **The user's is the other one.** Home Assistant's cover scale, Apple Home and
 * every consumer blind app read the opposite way: 100 = open. The firmware does
 * not pre-invert to suit them; it publishes the raw Somfy value and *declares*
 * the inversion (`somfy-mqtt` writes `"position_open":0,"position_closed":100`
 * into the HA discovery payload — `crates/somfy-mqtt/src/entity.rs`).
 *
 * # What this UI does
 *
 * The same thing, one layer up. Wire values stay raw everywhere in the app —
 * `ShadeDto.position` is never rewritten on arrival, and a `goTo` posts a raw
 * wire value. The inversion happens **only** at the render/input boundary,
 * through {@link openPercent} and {@link wireFromOpenPercent}, so that what the
 * user sees and drags is openness (100 = open), matching every other blind UI
 * they own.
 *
 * The rule this file exists to enforce: **`100 -` appears nowhere else in the
 * codebase.** If you find yourself typing it, the conversion belongs here.
 */

/** Both scales are whole percent, so both share these bounds. */
export const PERCENT_MIN = 0;
export const PERCENT_MAX = 100;

/** Wire value of a fully open shade. */
export const WIRE_OPEN = PERCENT_MIN;

/** Wire value of a fully closed shade. */
export const WIRE_CLOSED = PERCENT_MAX;

/**
 * A raw wire position: whole percent, `0` open … `100` closed. Exactly what
 * `ShadeDto.position` / `ShadeStateEvent.position` carry.
 */
export type WirePercent = number;

/**
 * A user-facing openness: whole percent, `0` closed … `100` open. What the
 * slider and the tile label show.
 */
export type OpenPercent = number;

const clamp = (value: number): number =>
  Math.min(WIRE_CLOSED, Math.max(WIRE_OPEN, Math.round(value)));

/** Wire position → user-facing openness. The only inversion in the app. */
export const openPercent = (wire: WirePercent): OpenPercent => WIRE_CLOSED - clamp(wire);

/** User-facing openness → wire position. The inverse of {@link openPercent}. */
export const wireFromOpenPercent = (open: OpenPercent): WirePercent => WIRE_CLOSED - clamp(open);

/**
 * `direction` on the wire uses the sign convention deployed devices emit:
 * `-1` up (opening), `0` idle, `+1` down (closing). Named here so no screen
 * has to remember which way the sign points.
 */
export const DIRECTION_UP = -1;
export const DIRECTION_IDLE = 0;
export const DIRECTION_DOWN = 1;

export type Motion = 'opening' | 'idle' | 'closing';

/** Wire `direction` → the motion the user is watching. */
export function motionOf(direction: number): Motion {
  if (direction === DIRECTION_UP) return 'opening';
  if (direction === DIRECTION_DOWN) return 'closing';
  return 'idle';
}
