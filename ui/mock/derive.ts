/**
 * The mock's port of the fields `ShadeDto` computes rather than stores.
 *
 * Two of them exist, and both are derived on the Rust side for the same reason:
 * a stored copy of something that can be recomputed is a copy that can be
 * wrong, and both of these gate a decision the user cannot easily check —
 * whether pairing is offered at all, and whether a position estimate is worth
 * anything.
 *
 * Keeping them in one file lets {@link StoredShade} be the *seed* shape and
 * `ShadeDto` be built from it, so the fixtures cannot hand-write an
 * `addressOrigin` that disagrees with their own address. The Rust side has that
 * property by construction; this is how the mock gets it too.
 */
import type { AddressOrigin } from '../src/api/generated/AddressOrigin.ts';
import type { CalibrationSource } from '../src/api/generated/CalibrationSource.ts';
import type { ShadeDto } from '../src/api/generated/ShadeDto.ts';
import {
  FACTORY_DOWN_TIME_MS,
  FACTORY_TILT_TIME_MS,
  FACTORY_UP_TIME_MS,
  OUR_SPACE,
} from './fixtures.ts';

/**
 * Everything about a shade the device actually keeps. The complement of this
 * type is exactly the derived set, so `Omit` naming them is the check: a field
 * that stops being derived in Rust makes this stop compiling.
 */
export type StoredShade = Omit<
  ShadeDto,
  'addressOrigin' | 'upTimeSource' | 'downTimeSource' | 'tiltTimeSource'
>;

/**
 * `AddressOrigin::of` — bit 23 is `RemoteIdentity::SPACE_START`, set on every
 * address this controller's allocator produces and on nothing it imports.
 */
export const originOf = (address: number): AddressOrigin =>
  (address & OUR_SPACE) !== 0 ? 'allocated' : 'imported';

/**
 * `CalibrationSource::of` — a travel time equal to the reference firmware's
 * compiled-in default is evidence that nobody chose it, so it reports as
 * uncalibrated rather than as a setting (R7).
 *
 * `measured` is never returned, exactly as in Rust: the guided sweep of R2 does
 * not exist, and the state is in the union so that building it later adds
 * behaviour instead of changing the contract.
 */
export const calibrationOf = (valueMs: number, factoryDefaultMs: number): CalibrationSource =>
  valueMs === factoryDefaultMs ? 'factoryDefault' : 'operatorSupplied';

/** Complete a stored shade into the payload the API serves. */
export const toDto = (stored: StoredShade): ShadeDto => ({
  ...stored,
  addressOrigin: originOf(stored.address),
  upTimeSource: calibrationOf(stored.upTimeMs, FACTORY_UP_TIME_MS),
  downTimeSource: calibrationOf(stored.downTimeMs, FACTORY_DOWN_TIME_MS),
  tiltTimeSource: calibrationOf(stored.tiltTimeMs, FACTORY_TILT_TIME_MS),
});
