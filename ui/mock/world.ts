/**
 * The mock device's state and its motion model.
 *
 * The model is a port of `somfy-domain`'s dead reckoning (`shade.rs`,
 * `motion.rs`), not an invention: positions are integrated in hundredths of a
 * percent like `Pos`, `Up`/`Down` seek the hard limits, `My` freezes a moving
 * shade and recalls the favourite on an idle one, and `StepUp`/`StepDown` nudge
 * the target by `STEP_TRAVEL_MS` (100 ms) worth of travel. It is a *model* of
 * the firmware, so the dashboard is exercised against behaviour the real device
 * can actually produce — a mock that teleports positions would hide every
 * mistake the transition states contain.
 *
 * Two things it deliberately does **not** model.
 *
 * **The mid-range stop frame.** A real seek to an intermediate position is a
 * `Down`/`Up` plus a single `My` when the estimate arrives, because the motor
 * self-stops only at a hard limit. Here the integrator simply stops at the
 * target, because the frame has no observable effect on the wire — carrying a
 * `stopOnArrival` flag nothing reads would be faithfulness theatre.
 *
 * **Radio loss.** That single stop frame does go missing in the field (see the
 * position-accuracy requirements, cause A), but a mock that dropped frames at
 * random would make UI work non-reproducible. If that behaviour is ever wanted
 * — and it would be a good way to test how the UI reports an overshoot — it
 * belongs behind an explicit switch.
 */
import type { CommandDto } from '../src/api/generated/CommandDto.ts';
import type { GroupDto } from '../src/api/generated/GroupDto.ts';
import type { RoomDto } from '../src/api/generated/RoomDto.ts';
import type { ShadeDto } from '../src/api/generated/ShadeDto.ts';
import type { WsEvent } from '../src/api/generated/WsEvent.ts';
import { GROUPS, ROOMS, SHADES } from './fixtures.ts';

/** `Pos::FULL` — full travel in hundredths of a percent (`somfy-domain`). */
const FULL_RAW = 10_000;

/** `STEP_TRAVEL_MS` — motor run time per Step tap (`shade.rs`). */
const STEP_TRAVEL_MS = 100;

/** How often the world integrates motion and pushes state. */
export const TICK_MS = 200;

/** Direction sign convention, as the wire uses it. */
const UP = -1;
const IDLE = 0;
const DOWN = 1;

interface Motion {
  /** Live position, hundredths of a percent. 0 open … 10000 closed. */
  raw: number;
  /** Where it is heading, same units. */
  targetRaw: number;
  direction: number;
}

const percentToRaw = (percent: number): number =>
  Math.min(100, Math.max(0, Math.round(percent))) * 100;

const rawToPercent = (raw: number): number => Math.round(raw / 100);

export type Listener = (event: WsEvent) => void;

export class World {
  private readonly shades = new Map<number, ShadeDto>();
  private readonly motion = new Map<number, Motion>();
  private readonly listeners = new Set<Listener>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private lastTickMs = Date.now();

  constructor() {
    for (const shade of SHADES) {
      // Copy: the fixtures stay pristine so a dev-server restart is a real reset.
      this.shades.set(shade.id, { ...shade });
      this.motion.set(shade.id, {
        raw: percentToRaw(shade.position),
        targetRaw: percentToRaw(shade.target),
        direction: IDLE,
      });
    }
  }

  /**
   * Reads tick first. `ShadeDto::from_shade` says the same thing on the Rust
   * side — "call after `Shade::tick` to reflect the latest position" — and a
   * mock that served a stale position to a REST client just because no
   * WebSocket happened to be open would be lying in a way the device does not.
   */
  listShades(): ShadeDto[] {
    this.tick();
    return [...this.shades.values()];
  }

  getShade(id: number): ShadeDto | undefined {
    this.tick();
    return this.shades.get(id);
  }

  listRooms(): RoomDto[] {
    return ROOMS;
  }

  listGroups(): GroupDto[] {
    return GROUPS;
  }

  /** Current state of every shade, for a client that has just connected. */
  snapshotEvents(): WsEvent[] {
    return this.listShades().map((shade) => this.eventFor(shade));
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    this.start();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.stop();
    };
  }

  /**
   * Apply a command to one shade.
   *
   * The `switch` is exhaustive over the generated {@link CommandDto} union and
   * ends in {@link assertNever}: when `somfy-api` gains an action, this stops
   * compiling instead of silently ignoring the new command.
   */
  command(id: number, command: CommandDto): boolean {
    const shade = this.shades.get(id);
    const motion = this.motion.get(id);
    if (!shade || !motion) return false;

    // Advance to now before re-targeting, exactly as `Shade::handle` syncs
    // first: a real motor keeps moving while the request is in flight.
    this.tick();

    switch (command.action) {
      case 'up':
        this.setTarget(motion, 0);
        break;
      case 'down':
        this.setTarget(motion, FULL_RAW);
        break;
      case 'my':
        if (motion.direction !== IDLE) {
          motion.targetRaw = motion.raw;
          motion.direction = IDLE;
        } else if (shade.myPosition !== null) {
          this.seek(motion, percentToRaw(shade.myPosition));
        }
        break;
      case 'stepUp':
        this.step(shade, motion, UP);
        break;
      case 'stepDown':
        this.step(shade, motion, DOWN);
        break;
      case 'goTo':
        this.seek(motion, percentToRaw(command.position));
        break;
      case 'setMy':
        shade.myPosition = command.position;
        break;
      default:
        return assertNever(command);
    }

    this.publish(shade, motion);
    return true;
  }

  /** Fan a command out to a group's members, as the device does in v1.0. */
  commandGroup(id: number, command: CommandDto): boolean {
    const group = GROUPS.find((candidate) => candidate.id === id);
    if (!group) return false;
    for (const shadeId of group.shadeIds) this.command(shadeId, command);
    return true;
  }

  private setTarget(motion: Motion, targetRaw: number): void {
    motion.targetRaw = targetRaw;
    motion.direction =
      targetRaw === motion.raw ? IDLE : targetRaw > motion.raw ? DOWN : UP;
  }

  /** `Shade::seek`: seeking the current position is a no-op with no frame. */
  private seek(motion: Motion, targetRaw: number): void {
    if (targetRaw === motion.raw) return;
    this.setTarget(motion, targetRaw);
  }

  /** `Shade::step_target`: `FULL_RAW * STEP_TRAVEL_MS / travel_ms`, clamped. */
  private step(shade: ShadeDto, motion: Motion, direction: number): void {
    const travelMs = direction === UP ? shade.upTimeMs : shade.downTimeMs;
    if (travelMs === 0) return;
    const stepRaw = Math.min(FULL_RAW, Math.floor((FULL_RAW * STEP_TRAVEL_MS) / travelMs));
    const next = clampRaw(motion.raw + (direction === UP ? -stepRaw : stepRaw));
    this.setTarget(motion, next);
  }

  private start(): void {
    if (this.timer !== undefined) return;
    this.lastTickMs = Date.now();
    this.timer = setInterval(() => this.tick(), TICK_MS);
    // Never hold the dev server open on our account.
    this.timer.unref?.();
  }

  private stop(): void {
    if (this.timer === undefined) return;
    clearInterval(this.timer);
    this.timer = undefined;
  }

  /** Integrate every moving shade forward to now, publishing what changed. */
  private tick(): void {
    const now = Date.now();
    const elapsedMs = now - this.lastTickMs;
    this.lastTickMs = now;
    if (elapsedMs <= 0) return;

    for (const [id, motion] of this.motion) {
      if (motion.direction === IDLE) continue;
      const shade = this.shades.get(id);
      if (!shade) continue;

      const travelMs = motion.direction === UP ? shade.upTimeMs : shade.downTimeMs;
      if (travelMs === 0) {
        motion.direction = IDLE;
        continue;
      }

      const travelled = (FULL_RAW * elapsedMs) / travelMs;
      const moved = motion.direction === UP ? motion.raw - travelled : motion.raw + travelled;
      const arrived =
        motion.direction === UP ? moved <= motion.targetRaw : moved >= motion.targetRaw;

      motion.raw = clampRaw(arrived ? motion.targetRaw : moved);
      if (arrived) {
        motion.direction = IDLE;
      }
      this.publish(shade, motion);
    }
  }

  /** Mirror the motion state onto the DTO and push it to subscribers. */
  private publish(shade: ShadeDto, motion: Motion): void {
    shade.position = rawToPercent(motion.raw);
    shade.target = rawToPercent(motion.targetRaw);
    shade.direction = motion.direction;
    const event = this.eventFor(shade);
    for (const listener of this.listeners) listener(event);
  }

  private eventFor(shade: ShadeDto): WsEvent {
    return {
      ev: 'shadeState',
      id: shade.id,
      position: shade.position,
      tiltPosition: shade.tiltPosition,
      direction: shade.direction,
    };
  }
}

const clampRaw = (raw: number): number => Math.min(FULL_RAW, Math.max(0, Math.round(raw)));

/**
 * Reached only if {@link CommandDto} grows a variant the switch above does not
 * handle — in which case the parameter is no longer `never` and this call is a
 * compile error, which is exactly the drift we want to catch.
 */
function assertNever(value: never): never {
  throw new Error(`unhandled command: ${JSON.stringify(value)}`);
}
