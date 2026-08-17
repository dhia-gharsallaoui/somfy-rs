/**
 * Seed data for the mock device.
 *
 * **Everything here is invented.** The remote addresses are deliberately absurd
 * `…ACE__` runs so that no value in this repository can be mistaken for a real
 * shade's radio address — real ones are private and cost a physical re-pairing
 * to leak.
 *
 * The types are the generated ones. A field renamed, added or removed in
 * `somfy-api` and regenerated makes this file stop compiling, which is the
 * point: the mock cannot describe a device the firmware would not.
 *
 * ## Two address runs, because the difference is the whole feature
 *
 * `addressOrigin` is read off bit 23 of the address ({@link OUR_SPACE}), so a
 * fixture set living entirely on one side of that bit could not exercise the
 * screens that branch on it. Half of these shades therefore sit in each:
 *
 * - **`0x8ACE__` — allocated.** The shape `RemoteIdentity::address_for`
 *   produces: the reserved bit, then a device-derived base, then the shade's
 *   id. Pairing is offered on these.
 * - **`0x7ACE__` — imported.** Bit 23 clear, so it could not have come from
 *   this controller's allocator. These stand in for a table carried over from
 *   the controller being replaced, which is the case the owner objected to
 *   seeing a pairing button on: pairing them teaches a motor an address a
 *   *different* controller is still counting on.
 */
import type { GroupDto } from '../src/api/generated/GroupDto.ts';
import type { RoomDto } from '../src/api/generated/RoomDto.ts';
import type { ShadeDto } from '../src/api/generated/ShadeDto.ts';

/**
 * `RemoteIdentity::SPACE_START` — bit 23, set on every address this
 * controller's allocator produces and on no address it imports.
 */
export const OUR_SPACE = 0x80_0000;

/**
 * The mock controller's allocation base, standing in for
 * `RemoteIdentity::from_mac(...).base()`. A shade allocated at id `n` takes
 * `MOCK_BASE + n`, probing upward past anything already taken — the same walk
 * `RemoteIdentity::address_for` does.
 */
export const MOCK_BASE = OUR_SPACE | 0x0a_ce00;

/** `MAX_SHADES` from `somfy-domain`'s registry. */
export const MAX_SHADES = 32;

/**
 * `ShadeKind` discriminants from `somfy-domain` (`types.rs`): Roller 0x00,
 * Blind 0x01, DraperyLeft 0x02, Awning 0x03, Shutter 0x04, DraperyRight 0x07,
 * DraperyCenter 0x08.
 */
export const KIND = {
  roller: 0x00,
  blind: 0x01,
  draperyLeft: 0x02,
  awning: 0x03,
  shutter: 0x04,
  draperyRight: 0x07,
  draperyCenter: 0x08,
} as const;

/**
 * `TiltMode` discriminants from the same file: None 0x00, TiltMotor 0x01,
 * Integrated 0x02, TiltOnly 0x03, EuroMode 0x04.
 */
export const TILT = {
  none: 0x00,
  motor: 0x01,
  integrated: 0x02,
  tiltOnly: 0x03,
  euro: 0x04,
} as const;

/**
 * Six fake shades. Positions are **wire values**: 0 fully open, 100 fully
 * closed (see `src/api/position.ts`). Travel times are deliberately asymmetric
 * on a couple of them — a roller descends faster than it rises, and the domain
 * has carried per-direction times all along.
 *
 * Ids start at **0**, because a shade's id is its registry slot and
 * `Registry::add_shade` fills the lowest free one. Numbering these from 1 would
 * make the first shade added through the UI take id 0 and sort itself in front
 * of the seed data, which looks like a bug and is not one.
 *
 * Three carry allocated addresses and three imported ones; see the module note.
 */
export const SHADES: ShadeDto[] = [
  {
    id: 0,
    name: 'Living room left',
    address: MOCK_BASE + 0,
    addressOrigin: 'allocated',
    kind: KIND.roller,
    tiltMode: TILT.none,
    position: 0,
    target: 0,
    tiltPosition: 0,
    myPosition: 35,
    direction: 0,
    upTimeMs: 12_000,
    downTimeMs: 9_500,
    tiltTimeMs: 0,
  },
  {
    id: 1,
    name: 'Living room right',
    address: MOCK_BASE + 1,
    addressOrigin: 'allocated',
    kind: KIND.roller,
    tiltMode: TILT.none,
    position: 100,
    target: 100,
    tiltPosition: 0,
    myPosition: 35,
    direction: 0,
    upTimeMs: 12_000,
    downTimeMs: 9_500,
    tiltTimeMs: 0,
  },
  {
    id: 2,
    name: 'Living room terrace',
    address: 0x7a_ce02,
    addressOrigin: 'imported',
    kind: KIND.awning,
    tiltMode: TILT.none,
    position: 60,
    target: 60,
    tiltPosition: 0,
    myPosition: null,
    direction: 0,
    upTimeMs: 18_000,
    downTimeMs: 18_000,
    tiltTimeMs: 0,
  },
  {
    id: 3,
    name: 'Kitchen',
    address: MOCK_BASE + 3,
    addressOrigin: 'allocated',
    kind: KIND.blind,
    tiltMode: TILT.integrated,
    position: 40,
    target: 40,
    tiltPosition: 20,
    myPosition: 50,
    direction: 0,
    upTimeMs: 8_000,
    downTimeMs: 7_000,
    tiltTimeMs: 1_500,
  },
  {
    id: 4,
    name: 'Bedroom window',
    address: 0x7a_ce04,
    addressOrigin: 'imported',
    kind: KIND.shutter,
    tiltMode: TILT.none,
    position: 100,
    target: 100,
    tiltPosition: 0,
    myPosition: null,
    direction: 0,
    upTimeMs: 10_000,
    downTimeMs: 10_000,
    tiltTimeMs: 0,
  },
  {
    id: 5,
    name: 'Bedroom door',
    address: 0x7a_ce05,
    addressOrigin: 'imported',
    kind: KIND.draperyCenter,
    tiltMode: TILT.none,
    position: 0,
    target: 0,
    tiltPosition: 0,
    myPosition: null,
    direction: 0,
    upTimeMs: 6_000,
    downTimeMs: 6_000,
    tiltTimeMs: 0,
  },
];

export const ROOMS: RoomDto[] = [
  { id: 1, name: 'Living room', shadeIds: [0, 1, 2] },
  { id: 2, name: 'Kitchen', shadeIds: [3] },
  { id: 3, name: 'Bedroom', shadeIds: [4, 5] },
];

export const GROUPS: GroupDto[] = [
  // Fully inside one room — the dashboard nests these under it.
  { id: 1, name: 'Living room windows', shadeIds: [0, 1] },
  { id: 2, name: 'Bedroom', shadeIds: [4, 5] },
  // Deliberately spans two rooms, so the dashboard's "groups that do not fit a
  // single room" path is exercised by the fixtures rather than only in theory.
  { id: 3, name: 'Street side', shadeIds: [0, 3] },
];
