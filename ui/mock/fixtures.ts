/**
 * Seed data for the mock device.
 *
 * **Everything here is invented.** The remote addresses are a deliberately
 * absurd `0xFACE__` run so that no value in this repository can be mistaken for
 * a real shade's radio address — real ones are private and cost a physical
 * re-pairing to leak.
 *
 * The types are the generated ones. A field renamed, added or removed in
 * `somfy-api` and regenerated makes this file stop compiling, which is the
 * point: the mock cannot describe a device the firmware would not.
 */
import type { GroupDto } from '../src/api/generated/GroupDto.ts';
import type { RoomDto } from '../src/api/generated/RoomDto.ts';
import type { ShadeDto } from '../src/api/generated/ShadeDto.ts';

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
 */
export const SHADES: ShadeDto[] = [
  {
    id: 1,
    name: 'Living room left',
    address: 0xface01,
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
    id: 2,
    name: 'Living room right',
    address: 0xface02,
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
    id: 3,
    name: 'Living room terrace',
    address: 0xface03,
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
    id: 4,
    name: 'Kitchen',
    address: 0xface04,
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
    id: 5,
    name: 'Bedroom window',
    address: 0xface05,
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
    id: 6,
    name: 'Bedroom door',
    address: 0xface06,
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
  { id: 1, name: 'Living room', shadeIds: [1, 2, 3] },
  { id: 2, name: 'Kitchen', shadeIds: [4] },
  { id: 3, name: 'Bedroom', shadeIds: [5, 6] },
];

export const GROUPS: GroupDto[] = [
  // Fully inside one room — the dashboard nests these under it.
  { id: 1, name: 'Living room windows', shadeIds: [1, 2] },
  { id: 2, name: 'Bedroom', shadeIds: [5, 6] },
  // Deliberately spans two rooms, so the dashboard's "groups that do not fit a
  // single room" path is exercised by the fixtures rather than only in theory.
  { id: 3, name: 'Street side', shadeIds: [1, 4] },
];
