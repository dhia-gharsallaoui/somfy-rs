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
import type { StoredShade } from './derive.ts';

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
 * The reference firmware's compiled-in travel-time defaults, mirroring
 * `somfy_api::FACTORY_*_TIME_MS`. A value equal to one of these is reported as
 * `factoryDefault` — nobody chose it — rather than as a setting (R7).
 */
export const FACTORY_UP_TIME_MS = 10_000;
export const FACTORY_DOWN_TIME_MS = 10_000;
export const FACTORY_TILT_TIME_MS = 7_000;

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
 * Seven fake shades. Positions are **wire values**: 0 fully open, 100 fully
 * closed (see `src/api/position.ts`). Travel times are deliberately asymmetric
 * on a couple of them — a roller descends faster than it rises, and the domain
 * has carried per-direction times all along.
 *
 * Ids start at **0**, because a shade's id is its registry slot and
 * `Registry::add_shade` fills the lowest free one. Numbering these from 1 would
 * make the first shade added through the UI take id 0 and sort itself in front
 * of the seed data, which looks like a bug and is not one.
 *
 * Four carry allocated addresses and three imported ones; see the module note.
 *
 * Typed {@link StoredShade}, not `ShadeDto`: `addressOrigin` and the three
 * calibration sources are **derived** (`./derive.ts`), so writing them here
 * would let a fixture claim an origin its own address contradicts.
 *
 * ## One shade is deliberately half-finished
 *
 * `Terrace awning` (id 6) carries `pairingState: 'awaitingConfirmation'` — a
 * shade somebody added and never finished pairing, which is the state the whole
 * add-a-shade flow exists to make visible rather than to hide. Without it the
 * dashboard's "finish setting up" section and the resume path through the
 * assistant would only ever be exercised by hand.
 *
 * Every other shade is `confirmedByOperator`, which is what a working
 * installation looks like and what a migrated table decodes as.
 *
 * ## Travel times, and why most of these are the factory defaults
 *
 * Deliberate, and the point of R7. Three of the six carry 10000/10000/7000 —
 * the reference firmware's compiled-in values — which is what a real migrated
 * setup looks like and is exactly the state that produced a 25%-open command
 * moving a shade about 1% on 2026-08-17. Two carry the numbers hand-measured
 * that day (**30 s up, 27 s down** — closing is gravity-assisted, so the ~10%
 * asymmetry is real) with its tilt time left untouched, so one shade shows the
 * mixed state and proves the flag is per field rather than per shade.
 */
export const SHADES: StoredShade[] = [
  {
    id: 0,
    name: 'Living room left',
    address: MOCK_BASE + 0,
    kind: KIND.roller,
    tiltMode: TILT.none,
    pairingState: 'confirmedByOperator',
    position: 0,
    target: 0,
    tiltPosition: 0,
    myPosition: 35,
    direction: 0,
    upTimeMs: FACTORY_UP_TIME_MS,
    downTimeMs: FACTORY_DOWN_TIME_MS,
    tiltTimeMs: FACTORY_TILT_TIME_MS,
  },
  {
    id: 1,
    name: 'Living room right',
    address: MOCK_BASE + 1,
    kind: KIND.roller,
    tiltMode: TILT.none,
    pairingState: 'confirmedByOperator',
    position: 100,
    target: 100,
    tiltPosition: 0,
    myPosition: 35,
    direction: 0,
    upTimeMs: FACTORY_UP_TIME_MS,
    downTimeMs: FACTORY_DOWN_TIME_MS,
    tiltTimeMs: FACTORY_TILT_TIME_MS,
  },
  {
    id: 2,
    name: 'Living room terrace',
    address: 0x7a_ce02,
    kind: KIND.awning,
    tiltMode: TILT.none,
    pairingState: 'confirmedByOperator',
    position: 60,
    target: 60,
    tiltPosition: 0,
    myPosition: null,
    direction: 0,
    upTimeMs: FACTORY_UP_TIME_MS,
    downTimeMs: FACTORY_DOWN_TIME_MS,
    tiltTimeMs: FACTORY_TILT_TIME_MS,
  },
  {
    id: 3,
    name: 'Kitchen',
    address: MOCK_BASE + 3,
    kind: KIND.blind,
    tiltMode: TILT.integrated,
    pairingState: 'confirmedByOperator',
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
    kind: KIND.shutter,
    tiltMode: TILT.none,
    pairingState: 'confirmedByOperator',
    position: 100,
    target: 100,
    tiltPosition: 0,
    myPosition: null,
    direction: 0,
    upTimeMs: 30_000,
    downTimeMs: 27_000,
    tiltTimeMs: FACTORY_TILT_TIME_MS,
  },
  {
    id: 5,
    name: 'Bedroom door',
    address: 0x7a_ce05,
    kind: KIND.draperyCenter,
    tiltMode: TILT.none,
    pairingState: 'confirmedByOperator',
    position: 0,
    target: 0,
    tiltPosition: 0,
    myPosition: null,
    direction: 0,
    upTimeMs: 6_000,
    downTimeMs: 6_000,
    tiltTimeMs: 0,
  },
  {
    // The half-finished one. Its address is allocated — the device invented it,
    // so no motor has heard it — and nobody has reported it working, so it has
    // no Home Assistant entities and the dashboard offers to finish it rather
    // than to drive it.
    id: 6,
    name: 'Terrace awning',
    address: MOCK_BASE + 6,
    kind: KIND.awning,
    tiltMode: TILT.none,
    pairingState: 'awaitingConfirmation',
    position: 0,
    target: 0,
    tiltPosition: 0,
    myPosition: null,
    direction: 0,
    upTimeMs: 12_000,
    downTimeMs: 11_000,
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
