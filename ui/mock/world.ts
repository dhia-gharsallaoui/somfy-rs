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
import type { ApiErrorCode } from '../src/api/generated/ApiErrorCode.ts';
import type { CommandDto } from '../src/api/generated/CommandDto.ts';
import type { CreateShadeDto } from '../src/api/generated/CreateShadeDto.ts';
import type { GroupDto } from '../src/api/generated/GroupDto.ts';
import type { PatchShadeDto } from '../src/api/generated/PatchShadeDto.ts';
import type { RoomDto } from '../src/api/generated/RoomDto.ts';
import type { ShadeDto } from '../src/api/generated/ShadeDto.ts';
import type { WsEvent } from '../src/api/generated/WsEvent.ts';
import type { CalibrationSource } from '../src/api/generated/CalibrationSource.ts';
import type { CalibrationStepDto } from '../src/api/generated/CalibrationStepDto.ts';
import { originOf, toDto, type StoredShade } from './derive.ts';
import {
  DEAD_BAND_RESOLUTION_MS,
  FACTORY_DOWN_TIME_MS,
  FACTORY_TILT_TIME_MS,
  FACTORY_UP_TIME_MS,
  GROUPS,
  MAX_SHADES,
  MOCK_BASE,
  ROOMS,
  SHADES,
  START_LAG_RESOLUTION_MS,
} from './fixtures.ts';
import { validateCreateShade, validatePatchShade } from './validate.ts';

/**
 * `somfy_api::shades::supplied_source` — a submitted value equal to the factory
 * default is recorded as one nobody chose, whichever endpoint it arrived on.
 */
const sourceOf = (valueMs: number, factoryMs: number): CalibrationSource =>
  valueMs === factoryMs ? 'factoryDefault' : 'operatorSupplied';

/** `round_onto` in `somfy_domain::types`: nearest multiple of `step`. */
const roundTo = (ms: number, step: number): number => Math.round(ms / step) * step;

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

/**
 * What `POST /api/v1/shades` produced: the created shade, or the code the
 * device would have refused it with. A discriminated result rather than a
 * thrown error, so the caller has to look.
 */
export type CreateResult = { ok: ShadeDto } | { error: ApiErrorCode };

/**
 * What `POST /api/v1/shades/{id}/pair` produced.
 *
 * `accepted` is deliberately not called `sent`, and there is no `succeeded`:
 * RTS is one-way, so the furthest any layer here can honestly go is "the
 * request was taken". Whether the motor heard it is settled by a person
 * watching the shade.
 */
export type PairResult = 'accepted' | { error: ApiErrorCode };

/**
 * A calibration run in progress: when it started, and whichever moments the
 * operator has reported so far.
 *
 * `somfy_domain`'s `MAX_TRAVEL_TIME_MS`, restated: three minutes is the ceiling
 * deployed controllers already enforce on a hand-entered travel time, and a run
 * still going after that is one where somebody walked away.
 */
const MAX_TRAVEL_TIME_MS = 180_000;

interface CalibrationRun {
  leg: 'up' | 'down';
  startedMs: number;
  motionBeganMs?: number;
  curtainMovedMs?: number;
}

export class World {
  /**
   * Stored fields only. `addressOrigin` is computed by {@link toDto} on the way
   * out, exactly as `ShadeDto::from_shade` computes it — so nothing in here can
   * hold a stale copy of a fact that is really a function of another field.
   */
  private readonly shades = new Map<number, StoredShade>();
  private readonly motion = new Map<number, Motion>();
  /**
   * Calibration runs in progress, keyed by shade.
   *
   * Not part of {@link StoredShade}, because a run is not a setting: it holds
   * wall-clock stamps and disappears when the conversation ends, whichever way
   * it ends. `shade.calibrating` is the visible half and is kept in step here.
   */
  private readonly runs = new Map<number, CalibrationRun>();
  private readonly listeners = new Set<Listener>();
  /**
   * Rooms and groups are copied, not aliased, because deleting a shade now
   * mutates them — `Registry::remove_shade` drops the id from every group and
   * room it belonged to, and a mock that left the id behind would report a
   * group of three that only ever moves two.
   */
  private readonly rooms: RoomDto[];
  private readonly groups: GroupDto[];
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
    this.rooms = ROOMS.map((room) => ({ ...room, shadeIds: [...room.shadeIds] }));
    this.groups = GROUPS.map((group) => ({ ...group, shadeIds: [...group.shadeIds] }));
  }

  /**
   * Reads tick first. `ShadeDto::from_shade` says the same thing on the Rust
   * side — "call after `Shade::tick` to reflect the latest position" — and a
   * mock that served a stale position to a REST client just because no
   * WebSocket happened to be open would be lying in a way the device does not.
   */
  listShades(): ShadeDto[] {
    this.tick();
    return [...this.shades.values()].map(toDto);
  }

  getShade(id: number): ShadeDto | undefined {
    this.tick();
    const shade = this.shades.get(id);
    return shade && toDto(shade);
  }

  listRooms(): RoomDto[] {
    return this.rooms;
  }

  listGroups(): GroupDto[] {
    return this.groups;
  }

  // -------------------------------------------------------------- lifecycle

  /**
   * `POST /api/v1/shades`. Validates as the firmware does, then assigns the id
   * and allocates the address — the two things a client may never choose.
   *
   * A created shade is **not** placed in a room or a group: the device has no
   * way to guess which, and the dashboard already collects room-less shades
   * under "Not in a room" rather than dropping them.
   */
  createShade(body: CreateShadeDto): CreateResult {
    const error = validateCreateShade(body, this.shades.size);
    if (error) return { error };

    const id = this.nextId();
    if (id === undefined) return { error: 'registryFull' };

    const address = this.allocateAddress(id);
    const shade: StoredShade = {
      id,
      name: body.name,
      address,
      kind: body.kind,
      tiltMode: body.tiltMode,
      // The address was invented a line above, so no motor has heard it and
      // this shade moves nothing. It exists, it is commandable here — which is
      // how the setup flow tests it — and it has no Home Assistant entities
      // until somebody reports it working. `somfy_domain::PairingState`.
      pairingState: 'awaitingConfirmation',
      // A shade nobody has moved and nobody has overheard is at the position
      // `Shade::new` starts it at. The first Open or Close corrects it against
      // a physical limit.
      position: 0,
      target: 0,
      tiltPosition: 0,
      myPosition: null,
      direction: IDLE,
      upTimeMs: body.upTimeMs,
      downTimeMs: body.downTimeMs,
      tiltTimeMs: body.tiltTimeMs,
      // `supplied_source` in `somfy_api::shades`: both forms in this UI are
      // pre-filled, so submitting a field untouched is not evidence anybody
      // chose the number in it. R7's ruling, applied where a value enters.
      upTimeSource: sourceOf(body.upTimeMs, FACTORY_UP_TIME_MS),
      downTimeSource: sourceOf(body.downTimeMs, FACTORY_DOWN_TIME_MS),
      tiltTimeSource: sourceOf(body.tiltTimeMs, FACTORY_TILT_TIME_MS),
      // Nothing has been measured on a shade that has never moved, and zero is
      // the un-compensated model rather than a guess.
      startLagMs: 0,
      ventBandMs: 0,
      closeBandMs: 0,
      // A shade this controller has never moved has never been anywhere, so its
      // position is a convention. `Shade::new` makes the same claim.
      positionUncertainty: 0,
    };

    this.shades.set(id, shade);
    this.motion.set(id, { raw: 0, targetRaw: 0, direction: IDLE });
    return { ok: toDto(shade) };
  }

  /**
   * `PATCH /api/v1/shades/{id}` — the port of `PatchShadeDto::apply`.
   *
   * Validates against the **result** rather than the body, and writes nothing
   * unless the whole patch is acceptable: a shade left renamed but still
   * holding a rejected travel time would be worse than one that changed
   * nothing at all.
   *
   * The travel times take effect immediately, including mid-travel — `tick`
   * reads them every interval, which is the same thing a real shade does when
   * its configuration changes under a moving estimate.
   */
  patchShade(id: number, body: PatchShadeDto): CreateResult {
    const current = this.shades.get(id);
    if (!current) return { error: 'notFound' };

    const dto = toDto(current);
    const error = validatePatchShade(body, dto);
    if (error) return { error };

    const next: StoredShade = {
      ...current,
      ...(body.name !== undefined && { name: body.name }),
      ...(body.kind !== undefined && { kind: body.kind }),
      ...(body.tiltMode !== undefined && { tiltMode: body.tiltMode }),
      // Each travel time carries its provenance with it, and only the fields
      // the body actually names move — R7 is per field, not per shade.
      ...(body.upTimeMs !== undefined && {
        upTimeMs: body.upTimeMs,
        upTimeSource: sourceOf(body.upTimeMs, FACTORY_UP_TIME_MS),
      }),
      ...(body.downTimeMs !== undefined && {
        downTimeMs: body.downTimeMs,
        downTimeSource: sourceOf(body.downTimeMs, FACTORY_DOWN_TIME_MS),
      }),
      ...(body.tiltTimeMs !== undefined && {
        tiltTimeMs: body.tiltTimeMs,
        tiltTimeSource: sourceOf(body.tiltTimeMs, FACTORY_TILT_TIME_MS),
      }),
      // Rounded onto the resolution its measurement actually has, here as in
      // `somfy_domain::round_start_lag_ms` — so what a later `GET` returns is
      // the number the device is running rather than the one that was typed.
      ...(body.startLagMs !== undefined && {
        startLagMs: roundTo(body.startLagMs, START_LAG_RESOLUTION_MS),
      }),
      ...(body.ventBandMs !== undefined && {
        ventBandMs: roundTo(body.ventBandMs, DEAD_BAND_RESOLUTION_MS),
      }),
      ...(body.closeBandMs !== undefined && {
        closeBandMs: roundTo(body.closeBandMs, DEAD_BAND_RESOLUTION_MS),
      }),
    };

    this.shades.set(id, next);
    return { ok: toDto(next) };
  }

  /**
   * `POST /api/v1/shades/{id}/calibrate` — the port of `Shade`'s calibration
   * run.
   *
   * The run is real rather than faked: `begin` sends the traverse and stamps a
   * start time, the marks are stamped as they arrive, and `finish` turns the
   * intervals into the same three numbers the device stores. That matters
   * because the thing being exercised is a **conversation with timing in it** —
   * a screen that can only be checked against a stub is a screen nobody has
   * checked.
   *
   * What it does not reproduce is the arithmetic's edges: the device refuses a
   * traverse of zero or over three minutes, and refuses marks that leave no
   * travel behind them. Those are refusals the UI has to render, so they are
   * modelled; the rounding is modelled too, because a value that reads back
   * different from what was typed is exactly the surprise a mock exists to
   * surface early.
   */
  calibrate(id: number, step: CalibrationStepDto): { ok: true } | { error: ApiErrorCode } {
    const shade = this.shades.get(id);
    const motion = this.motion.get(id);
    if (!shade || !motion) return { error: 'notFound' };
    const now = Date.now();

    switch (step.step) {
      case 'begin':
        this.tick();
        this.setTarget(motion, step.leg === 'up' ? 0 : FULL_RAW);
        this.runs.set(id, { leg: step.leg, startedMs: now });
        this.publish(shade, motion);
        return { ok: true };

      case 'mark': {
        const run = this.runs.get(id);
        if (!run) return { error: 'notCalibrating' };
        if (step.mark === 'motionBegan') run.motionBeganMs = now;
        else run.curtainMovedMs = now;
        return { ok: true };
      }

      case 'finish': {
        const run = this.runs.get(id);
        if (!run) return { error: 'notCalibrating' };
        const elapsed = now - run.startedMs;
        if (elapsed <= 0 || elapsed > MAX_TRAVEL_TIME_MS) {
          return { error: 'calibrationImplausible' };
        }

        const lag =
          run.motionBeganMs === undefined
            ? shade.startLagMs
            : roundTo(run.motionBeganMs - run.startedMs, START_LAG_RESOLUTION_MS);
        const band =
          run.curtainMovedMs === undefined
            ? undefined
            : roundTo(
                run.leg === 'up'
                  ? run.curtainMovedMs - run.startedMs - lag
                  : now - run.curtainMovedMs,
                DEAD_BAND_RESOLUTION_MS,
              );

        // Applied to a copy first, so a run whose numbers do not survive
        // validation leaves the shade exactly as it was — `Shade::finish_calibration`.
        const next: StoredShade = { ...shade, startLagMs: lag };
        if (run.leg === 'up') {
          next.upTimeMs = elapsed;
          next.upTimeSource = 'measured';
          if (band !== undefined) next.ventBandMs = band;
        } else {
          next.downTimeMs = elapsed;
          next.downTimeSource = 'measured';
          if (band !== undefined) next.closeBandMs = band;
        }
        if (
          lag < 0 ||
          (band !== undefined && band < 0) ||
          lag + next.ventBandMs >= next.upTimeMs ||
          lag + next.closeBandMs >= next.downTimeMs
        ) {
          return { error: 'calibrationImplausible' };
        }

        // The run ended at a physical limit, which the motor enforces whatever
        // this controller believes — so the estimate is exact again.
        next.positionUncertainty = 0;
        const limit = run.leg === 'up' ? 0 : FULL_RAW;
        motion.raw = limit;
        motion.targetRaw = limit;
        motion.direction = IDLE;
        this.runs.delete(id);
        this.shades.set(id, next);
        this.publish(next, motion);
        return { ok: true };
      }

      default:
        // Deliberately does not stop the shade: an operator abandoning a
        // measurement has not asked for the motor to halt where it happens to
        // be. `Controller::cancel_calibration`.
        this.runs.delete(id);
        return { ok: true };
    }
  }

  /**
   * `DELETE /api/v1/shades/{id}`, including the part that is easy to forget:
   * `Registry::remove_shade` also drops the id from every group and room.
   */
  deleteShade(id: number): boolean {
    if (!this.shades.delete(id)) return false;
    this.motion.delete(id);
    for (const room of this.rooms) room.shadeIds = room.shadeIds.filter((m) => m !== id);
    for (const group of this.groups) group.shadeIds = group.shadeIds.filter((m) => m !== id);
    return true;
  }

  /**
   * `POST /api/v1/shades/{id}/pair`.
   *
   * Changes **nothing**, and that is the accurate model rather than a stub. A
   * real device queues a `Prog` burst at the shade's address; it learns no more
   * about the motor afterwards than it knew before, so there is no state for
   * either side to update. What it can refuse is pairing an address that came
   * from another controller, where the burst would go out perfectly and
   * accomplish nothing.
   */
  pairShade(id: number): PairResult {
    const shade = this.shades.get(id);
    if (!shade) return { error: 'notFound' };
    if (originOf(shade.address) !== 'allocated') return { error: 'addressNotAllocated' };
    return 'accepted';
  }

  /**
   * `POST /api/v1/shades/{id}/confirm-pairing`.
   *
   * The operator's report that they commanded the shade and watched it move.
   * On a real device this is what publishes the Home Assistant entities; here
   * there is no broker, so the whole observable effect is the state change —
   * which is precisely what the UI branches on, so the mock exercises the same
   * paths the device does.
   *
   * **Idempotent, and only in one direction.** Confirming an already-confirmed
   * shade answers `200` and changes nothing, which is how a client retrying
   * over a flaky link recovers. There is no way to say "unconfirmed": that
   * direction retires a working shade's entities, and removing the shade is the
   * loud way to do it.
   */
  confirmPairing(id: number): CreateResult {
    const current = this.shades.get(id);
    if (!current) return { error: 'notFound' };
    const next: StoredShade = { ...current, pairingState: 'confirmedByOperator' };
    this.shades.set(id, next);
    return { ok: toDto(next) };
  }

  /** Lowest free registry slot, as `Registry::add_shade` picks it. */
  private nextId(): number | undefined {
    for (let id = 0; id < MAX_SHADES; id++) {
      if (!this.shades.has(id)) return id;
    }
    return undefined;
  }

  /**
   * `RemoteIdentity::address_for`: the base plus the shade's id, walking upward
   * past anything the table already holds. The probe is not decorative — an
   * imported table carries a foreign controller's addresses, and allocating
   * over one of them is the exact clash the reserved bit exists to prevent.
   */
  private allocateAddress(id: number): number {
    const taken = new Set([...this.shades.values()].map((shade) => shade.address));
    for (let probe = 0; probe <= MAX_SHADES; probe++) {
      const address = MOCK_BASE + id + probe;
      if (!taken.has(address)) return address;
    }
    // Unreachable for the same reason it is in Rust: at most MAX_SHADES
    // addresses are held and MAX_SHADES + 1 distinct candidates are probed.
    throw new Error('no free address');
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
      // Close fully, then open just far enough to separate the slats. The
      // device does it in three timed steps and uses no position estimate at
      // all, which is the whole point of the command; the mock has no wall
      // clock worth simulating that against, so it models the *outcome* — the
      // shade ends fully closed, with the light gaps open and the curtain
      // exactly where a close leaves it.
      //
      // The refusal is modelled properly, though, because that is the part a
      // user meets: a shade whose slat-separation band has never been measured
      // has nothing for the vent to aim at.
      case 'vent':
        if (shade.ventBandMs === 0) return false;
        this.setTarget(motion, FULL_RAW);
        motion.raw = FULL_RAW;
        motion.targetRaw = FULL_RAW;
        motion.direction = IDLE;
        // Reaching a limit is the one thing this protocol can be sure of, so
        // the estimate is exact again. `Shade::reach_limit`.
        shade.positionUncertainty = 0;
        break;
      default:
        return assertNever(command);
    }

    this.publish(shade, motion);
    return true;
  }

  /** Fan a command out to a group's members, as the device does in v1.0. */
  commandGroup(id: number, command: CommandDto): boolean {
    const group = this.groups.find((candidate) => candidate.id === id);
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
  private step(shade: StoredShade, motion: Motion, direction: number): void {
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
  private publish(shade: StoredShade, motion: Motion): void {
    shade.position = rawToPercent(motion.raw);
    shade.target = rawToPercent(motion.targetRaw);
    shade.direction = motion.direction;
    const event = this.eventFor(shade);
    for (const listener of this.listeners) listener(event);
  }

  private eventFor(shade: StoredShade): WsEvent {
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
