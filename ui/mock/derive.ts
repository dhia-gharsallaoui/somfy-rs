/**
 * The mock's port of the fields `ShadeDto` computes rather than stores.
 *
 * Keeping them in one file lets {@link StoredShade} be the *seed* shape and
 * `ShadeDto` be built from it, so the fixtures cannot hand-write an
 * `addressOrigin` that disagrees with their own address. The Rust side has that
 * property by construction; this is how the mock gets it too.
 *
 * ## The set shrank, and that is the point
 *
 * `upTimeSource`, `downTimeSource` and `tiltTimeSource` used to be derived here
 * too, by comparing each travel time against the factory default. That was never
 * a derivation so much as a guess standing in for a missing field: it made
 * `measured` unreachable, and it could not tell an operator who genuinely chose
 * 10 s from one who had never touched the setting.
 *
 * The persisted shade record carries provenance now, so the three are stored —
 * here as on the device. What is left derived is `addressOrigin`, which really
 * is a fact about the address and really cannot drift.
 *
 * `positionUncertainty` and `calibrating` are live domain state on the device
 * rather than settings; the mock keeps them on the stored shape because its
 * world updates them as it simulates movement, which is the same thing by
 * another route.
 */
import type { AddressOrigin } from '../src/api/generated/AddressOrigin.ts';
import type { ShadeDto } from '../src/api/generated/ShadeDto.ts';
import { OUR_SPACE } from './fixtures.ts';

/**
 * Everything about a shade the mock keeps. The complement of this type is
 * exactly the derived set, so `Omit` naming it is the check: a field that stops
 * being derived in Rust makes this stop compiling.
 */
export type StoredShade = Omit<ShadeDto, 'addressOrigin'>;

/**
 * `AddressOrigin::of` — bit 23 is `RemoteIdentity::SPACE_START`, set on every
 * address this controller's allocator produces and on nothing it imports.
 */
export const originOf = (address: number): AddressOrigin =>
  (address & OUR_SPACE) !== 0 ? 'allocated' : 'imported';

/** Complete a stored shade into the payload the API serves. */
export const toDto = (stored: StoredShade): ShadeDto => ({
  ...stored,
  addressOrigin: originOf(stored.address),
});
