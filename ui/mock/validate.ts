/**
 * A port of `CreateShadeDto::to_config` (`crates/somfy-api/src/shades.rs`).
 *
 * The mock exists so UI work needs no hardware, and a mock that accepted a
 * shade the device would refuse would hide exactly the bug that costs a user a
 * walk to a window. So the *rejections* are modelled as carefully as the happy
 * path, in the same order, returning the same {@link ApiErrorCode}s — the same
 * discipline `parseCommand` already applies to `goTo` and `setMy`.
 *
 * Order matters and is not incidental: a body with both an empty name and an
 * unmodelled kind must produce `nameEmpty` on both sides, or the UI would
 * highlight a different field against the mock than against the device.
 */
import type { ApiErrorCode } from '../src/api/generated/ApiErrorCode.ts';
import type { CreateShadeDto } from '../src/api/generated/CreateShadeDto.ts';
import type { PatchShadeDto } from '../src/api/generated/PatchShadeDto.ts';
import type { ShadeDto } from '../src/api/generated/ShadeDto.ts';
import { KIND, MAX_SHADES, TILT } from './fixtures.ts';

/** `somfy_api::NAME_MAX_BYTES` — the capacity of `heapless::String<32>`. */
export const NAME_MAX_BYTES = 32;

/**
 * UTF-8 length, because the Rust limit is a byte capacity rather than a
 * character count. `'é'.length` is 1 in JavaScript and 2 in a
 * `heapless::String<32>`, so a French name of 20 characters can be over the
 * limit and a UI counting `.length` would have promised the user it fits.
 */
export const nameBytes = (name: string): number => new TextEncoder().encode(name).length;

const KINDS = new Set<number>(Object.values(KIND));
const TILT_MODES = new Set<number>(Object.values(TILT));

/**
 * The kinds/tilt modes the *firmware* models, not the ones this file happens to
 * list: both sets are `somfy_domain::ShadeKind::from_raw` /
 * `TiltMode::from_raw`, which is why {@link KIND} and {@link TILT} carry the
 * discriminants rather than an invented enumeration.
 */
export function validateCreateShade(
  body: CreateShadeDto,
  shadeCount: number,
): ApiErrorCode | undefined {
  if (body.name.length === 0) return 'nameEmpty';
  if (nameBytes(body.name) > NAME_MAX_BYTES) return 'nameTooLong';
  if (!KINDS.has(body.kind)) return 'invalidKind';
  if (!TILT_MODES.has(body.tiltMode)) return 'invalidTiltMode';
  // Lift times only. A shade with no tilt has no tilt travel to time, which is
  // what every tilt-less row in a real table looks like.
  if (body.upTimeMs === 0 || body.downTimeMs === 0) return 'travelTimeZero';
  // Last, because it is the one rejection that is about the device rather than
  // about the body: a full registry is fixed by removing a shade, not by
  // correcting a field, and reporting it before a genuine typo would send the
  // user to the wrong remedy.
  if (shadeCount >= MAX_SHADES) return 'registryFull';
  return undefined;
}

/**
 * A port of `PatchShadeDto::apply`.
 *
 * Same rules, same order, checked against the **result** rather than the body —
 * so a patch setting only `upTimeMs` to zero is refused even though it says
 * nothing about the other direction. The invariant it protects is the one Rust
 * protects: nothing reachable by creating a shade and then patching it may be
 * unreachable by creating it directly.
 */
export function validatePatchShade(
  body: PatchShadeDto,
  current: ShadeDto,
): ApiErrorCode | undefined {
  if (body.name !== undefined) {
    if (body.name.length === 0) return 'nameEmpty';
    if (nameBytes(body.name) > NAME_MAX_BYTES) return 'nameTooLong';
  }
  if (body.kind !== undefined && !KINDS.has(body.kind)) return 'invalidKind';
  if (body.tiltMode !== undefined && !TILT_MODES.has(body.tiltMode)) return 'invalidTiltMode';

  const upTimeMs = body.upTimeMs ?? current.upTimeMs;
  const downTimeMs = body.downTimeMs ?? current.downTimeMs;
  if (upTimeMs === 0 || downTimeMs === 0) return 'travelTimeZero';

  return undefined;
}
