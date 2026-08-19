/**
 * The guided travel-time measurement (R2), and the two numbers it gets for free
 * (R5, R8).
 *
 * ## What this screen actually is
 *
 * Nothing on the device can see the shade. RTS is one-way, there is no encoder
 * and no limit-switch feedback, so the only instrument available is a person
 * watching the window and the device's own clock. "Automatic" here means the
 * device holds the stopwatch, not that it holds the eyes — and the screen says
 * that in as many words, because an operator who thinks the device is watching
 * has no reason to watch.
 *
 * That is not a poor substitute for something better — it is the only
 * measurement the physics permits. What it replaces is worse: on 2026-08-17
 * three shades were found carrying 10000/10000/7000, values nobody had ever
 * chosen, and a command for 25% open moved one of them about 1%.
 *
 * ## Three numbers from one traverse
 *
 * The Up run yields the traverse time, the start delay and the slat-separation
 * band, because they are three moments of the same movement. That matters
 * because R9 records that a sweep through the full range is not always
 * acceptable — over a desk, in a sleeping room, on an awning in wind — so a
 * design that needed a run per number would be one people decline to use. For
 * the same reason the whole cost is stated *before* the first button rather
 * than discovered during it, and hand entry sits in the panel above rather than
 * behind a link.
 *
 * ## Why every tap is optional, and why they are worth different amounts
 *
 * A human tap lands a couple of hundred milliseconds after what it aims at. The
 * band is the *difference* of two taps, so the operator's reaction delay
 * cancels out of it; the start delay is a single tap and carries it whole. So
 * the band is the reliable one, the delay is indicative, and the traverse —
 * seconds long — is the most reliable of all. That asymmetry is on the endpoint
 * and it is on this screen too: papering over it would leave an operator
 * treating a figure that is mostly their own reaction time as a measurement.
 *
 * Skipping a tap leaves that value as it was rather than storing a worse one —
 * with one consequence worth naming, because it is surprising and it is pinned
 * by `somfy-domain`'s `skipping_the_motion_tap_folds_the_start_delay_into_the_band`:
 * with no start-delay tap the band is measured against the *stored* delay, so
 * on a shade whose delay is still zero the band swallows it.
 *
 * ## What this screen may not claim
 *
 * **That the shade did anything.** `begin` queues a traverse and starts a clock;
 * whether the motor heard the frame is settled by the operator watching, which
 * is why the next control is "it has started moving" rather than a spinner.
 *
 * **That Cancel stops the shade.** It does not, deliberately —
 * `Controller::cancel_calibration` plans no frame, because an operator
 * abandoning a measurement has not asked for the motor to halt in the middle of
 * a window. The button says so.
 *
 * **That the run is still running.** Any command to this shade from anywhere —
 * the controls above, Home Assistant, a wall remote somebody in the house just
 * pressed — takes the shade's single activity slot and ends the run, and the
 * device does not volunteer that: the operator finds out at the next tap, as a
 * refusal. So the screen warns while the run is up, and turns that refusal into
 * the sentence that explains it rather than a generic failure.
 *
 * ## A refused run is not a finished one
 *
 * `Shade::finish_calibration` validates against a copy and, when the numbers do
 * not survive, **leaves the run open** so the operator can tap again rather than
 * start over. This screen used to return to idle on any refusal, which threw
 * away a run the device was still holding. It now distinguishes: `notCalibrating`
 * means the device's copy is genuinely gone, and anything else leaves the
 * controls up.
 */
import { useEffect, useState } from 'preact/hooks';

import { calibrateShade } from '../api/client';
import { ApiError, errorMessageKey } from '../api/errors';
import type { CalibrationLegDto } from '../api/generated/CalibrationLegDto';
import type { CalibrationMarkDto } from '../api/generated/CalibrationMarkDto';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { useT, type Translate } from '../i18n';
import type { MessageKey } from '../i18n/en';

/** Which run is in progress here, if any. */
type Phase =
  /** Nothing running. `done` is the last run's result, kept so it can be checked. */
  | { kind: 'idle'; done?: Done }
  | { kind: 'running'; leg: CalibrationLegDto; startedMs: number };

/**
 * What a finished run measured, kept on screen after it ends.
 *
 * R9's third reason for hand entry is that "a calibration routine needs
 * something to be checked against — a sweep reporting 10 s where a stopwatch
 * says 30 s must be visibly wrong". That check is only possible if the figure is
 * still there to look at, so the elapsed time survives the run rather than
 * vanishing into the panel above.
 */
interface Done {
  leg: CalibrationLegDto;
  /**
   * Whole tenths, deliberately.
   *
   * This is the *browser's* reading of the run, and the device's own — the value
   * in the field above — differs from it by one network round trip. More
   * precision would show them disagreeing in a digit neither of them means: the
   * measurement's real resolution is a human tap, a couple of hundred
   * milliseconds, and a stopwatch check is coarser still.
   */
  seconds: number;
}

/**
 * Per-leg wording, so neither the copy nor the marks can be paired wrongly.
 *
 * A record keyed by the generated {@link CalibrationLegDto} rather than a
 * template-built key: a third leg added in Rust fails `tsc` here, where a
 * `` `calib.auto${leg}` `` would have compiled and rendered a blank.
 */
const LEG: Record<
  CalibrationLegDto,
  {
    start: MessageKey;
    prep: MessageKey;
    writes: MessageKey;
    curtain: MessageKey;
    done: MessageKey;
    /** Where this run leaves the shade, and therefore what to measure next. */
    next: MessageKey;
    /** Which of `ShadeDto`'s provenances the *other* run would write. */
    otherSource: 'upTimeSource' | 'downTimeSource';
  }
> = {
  up: {
    start: 'calib.autoUp',
    prep: 'calib.autoUpPrep',
    writes: 'calib.autoUpWrites',
    curtain: 'calib.autoMarkCurtainUp',
    done: 'calib.autoDoneUp',
    next: 'calib.autoNextDown',
    otherSource: 'downTimeSource',
  },
  down: {
    start: 'calib.autoDown',
    prep: 'calib.autoDownPrep',
    writes: 'calib.autoDownWrites',
    curtain: 'calib.autoMarkCurtainDown',
    done: 'calib.autoDoneDown',
    next: 'calib.autoNextUp',
    otherSource: 'upTimeSource',
  },
};

export function Calibrate({ shade, onFinished }: { shade: ShadeDto; onFinished: () => void }) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' });
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<MessageKey | undefined>(undefined);
  const [failure, setFailure] = useState<MessageKey | undefined>(undefined);
  const [elapsed, setElapsed] = useState(0);

  // A visible clock, because the operator is timing something and a run with no
  // elapsed figure gives them nothing to sanity-check the stored value against
  // afterwards. One second is the right granularity: the thing being measured
  // is tens of seconds, and a faster tick would only add noise to the page.
  useEffect(() => {
    if (phase.kind !== 'running') return undefined;
    const started = phase.startedMs;
    setElapsed(0);
    const timer = setInterval(() => setElapsed(Math.round((Date.now() - started) / 1000)), 1000);
    return () => clearInterval(timer);
  }, [phase]);

  const send = (step: Parameters<typeof calibrateShade>[1], after: () => void) => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    calibrateShade(shade.id, step)
      .then(after)
      .catch((cause: unknown) => {
        const code = cause instanceof ApiError ? cause.code : undefined;
        if (code === 'notCalibrating') {
          // The device has no run. Either something else commanded the shade —
          // which silently takes its activity slot — or this tab is stale. Both
          // end the run here, and the message names the likely cause rather
          // than restating the code.
          setFailure('calib.autoInterrupted');
          setPhase({ kind: 'idle' });
          setNote(undefined);
          return;
        }
        // Everything else leaves the run where it was. An implausible finish is
        // the important one: the device deliberately keeps the run open so the
        // operator can re-tap, and dropping back to idle here would discard a
        // measurement that is still recoverable.
        setFailure(code === 'calibrationImplausible' ? 'calib.autoImplausible' : errorMessageKey(cause));
      })
      .finally(() => setBusy(false));
  };

  const begin = (leg: CalibrationLegDto) =>
    send({ step: 'begin', leg }, () => {
      setNote(undefined);
      setPhase({ kind: 'running', leg, startedMs: Date.now() });
    });

  const mark = (value: CalibrationMarkDto) =>
    send({ step: 'mark', mark: value }, () => setNote('calib.autoMarked'));

  const finish = (leg: CalibrationLegDto, startedMs: number) =>
    send({ step: 'finish' }, () => {
      setPhase({
        kind: 'idle',
        done: { leg, seconds: Math.round((Date.now() - startedMs) / 100) / 10 },
      });
      setNote(undefined);
      onFinished();
    });

  const cancel = () =>
    send({ step: 'cancel' }, () => {
      setPhase({ kind: 'idle' });
      setNote(undefined);
    });

  // A guided run on a shade no motor answers measures nothing: the device
  // transmits, nothing moves, and whatever the operator taps is stored as
  // `measured`. The panel above still accepts hand-entered values, which is what
  // R9 asks for and is exactly the case it asks for it in.
  const unpaired = shade.pairingState === 'awaitingConfirmation';

  return (
    <section class="panel">
      <h3>{t('calib.autoTitle')}</h3>
      <p class="prose">{t('calib.autoHint')}</p>
      <p class="prose">{t('calib.autoOneWay')}</p>

      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {t(failure)}
        </p>
      )}
      {note !== undefined && (
        <p class="note" role="status">
          {t(note)}
        </p>
      )}

      {unpaired ? (
        <p class="note note--warn">{t('calib.autoUnpaired', { name: shade.name })}</p>
      ) : phase.kind === 'idle' ? (
        <>
          {phase.done && <Finished done={phase.done} shade={shade} t={t} />}
          <p class="note note--warn">{t('calib.autoCost')}</p>
          {(['up', 'down'] as const).map((leg) => (
            <div key={leg}>
              <p class="field__hint">{t(LEG[leg].prep)}</p>
              <p class="field__hint">{t(LEG[leg].writes)}</p>
              <div class="actions">
                <button type="button" class="btn" disabled={busy} onClick={() => begin(leg)}>
                  {t(LEG[leg].start)}
                </button>
              </div>
            </div>
          ))}
        </>
      ) : (
        <>
          <p class="note" role="status">
            {t('calib.autoRunning', { elapsed: String(elapsed) })}
          </p>
          <p class="prose">{t('calib.autoWatch')}</p>
          <div class="actions">
            <button type="button" class="btn" disabled={busy} onClick={() => mark('motionBegan')}>
              {t('calib.autoMarkMotion')}
            </button>
            <button type="button" class="btn" disabled={busy} onClick={() => mark('curtainMoved')}>
              {t(LEG[phase.leg].curtain)}
            </button>
          </div>
          <p class="field__hint">{t('calib.autoOptional')}</p>
          <p class="field__hint">{t('calib.autoSkipMotion')}</p>
          <div class="actions">
            <button
              type="button"
              class="btn btn--primary"
              disabled={busy}
              onClick={() => finish(phase.leg, phase.startedMs)}
            >
              {t('calib.autoFinish')}
            </button>
            <button type="button" class="btn btn--ghost" disabled={busy} onClick={cancel}>
              {t('calib.autoCancel')}
            </button>
          </div>
          <p class="field__hint">{t('calib.autoCancelNote')}</p>
          <p class="note note--warn">{t('calib.autoDoNotTouch')}</p>
        </>
      )}
    </section>
  );
}

/**
 * What the last run measured, and what to do with it.
 *
 * Two things, and the second is the one that saves a traverse: the run ended at
 * a physical limit, which is precisely where the *other* direction's run has to
 * start. Measuring both in one visit therefore costs three traverses rather than
 * four, and an operator who does not know that pays the extra one.
 */
function Finished({ done, shade, t }: { done: Done; shade: ShadeDto; t: Translate }) {
  const leg = LEG[done.leg];
  return (
    <>
      <p class="note" role="status">
        {t(leg.done, { seconds: done.seconds.toFixed(1) })}
      </p>
      <p class="field__hint">{t('calib.autoCheck')}</p>
      {shade[leg.otherSource] !== 'measured' && <p class="field__hint">{t(leg.next)}</p>}
    </>
  );
}
