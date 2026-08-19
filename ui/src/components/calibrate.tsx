/**
 * The guided travel-time measurement (R2).
 *
 * ## Two presses, and why that is the whole screen
 *
 * Press to start a leg, watch the shade, press when it stops. Nothing on the
 * device can see the window — RTS is one-way, there is no encoder and no limit
 * switch — so the second press *is* the measurement, and every additional press
 * a flow asks for is one more of the operator's reaction times mixed into a
 * number.
 *
 * The traverse survives that mixing and the other figures did not. A run is tens
 * of seconds long and only one of its ends is human: the device knows exactly
 * when it put the frame on the air. So a guided run is *more* accurate than a
 * stopwatch, which carries a reaction delay at both ends — and that is the
 * reason to use this panel rather than a wristwatch and the fields above it.
 *
 * Until 2026-08-19 a run also asked for the moment the shade first stirred and
 * the moment the curtain separated from the slats, which fixed the start delay
 * and the slat band. Both were single presses at moments a fraction of a second
 * wide, so each carried a whole reaction delay against the interval it defined.
 * They are entered by hand now, in the panel above, which R9 requires as a MUST
 * anyway. Most of this file's prose went with them.
 *
 * ## What this screen may not claim
 *
 * **That the shade did anything.** `begin` queues a traverse and starts a clock;
 * whether the motor heard the frame is settled by the operator watching, which
 * is why the running state offers "it has stopped" rather than a progress bar.
 *
 * **That Cancel stops the shade.** It does not, deliberately —
 * `Controller::cancel_calibration` plans no frame, because an operator
 * abandoning a measurement has not asked for the motor to halt in the middle of
 * a window. The button says so.
 *
 * **That the run is still running.** Any command to this shade from anywhere —
 * the controls above, Home Assistant, a wall remote somebody in the house just
 * pressed — takes the shade's single activity slot and ends the run, and the
 * device does not volunteer that. With one press left, the operator finds out at
 * that press, as a refusal. So the screen warns while the run is up and turns
 * that refusal into the sentence that explains it.
 *
 * ## A refused run is not a finished one
 *
 * `Shade::finish_calibration` validates against a copy and, when the numbers do
 * not survive, **leaves the run open** so the operator can press again rather
 * than start over. `notCalibrating` means the device's copy is genuinely gone;
 * anything else leaves the controls up.
 */
import { useEffect, useState } from 'preact/hooks';

import { calibrateShade } from '../api/client';
import { ApiError, errorMessageKey } from '../api/errors';
import type { CalibrationLegDto } from '../api/generated/CalibrationLegDto';
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
 * says 30 s must be visibly wrong". That check is the only defence against the
 * one failure nothing here can catch — a plausible but wrong figure from
 * pressing early — so the elapsed time survives the run rather than vanishing
 * into the panel above.
 */
interface Done {
  leg: CalibrationLegDto;
  /**
   * Whole tenths, deliberately.
   *
   * This is the *browser's* reading of the run, and the device's own — the value
   * in the field above — differs from it by one network round trip. More
   * precision would show them disagreeing in a digit neither of them means: the
   * measurement's real resolution is a human press, a couple of hundred
   * milliseconds, and a stopwatch check is coarser still.
   */
  seconds: number;
}

/**
 * Per-leg wording, so the copy cannot be paired with the wrong direction.
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
    done: 'calib.autoDoneUp',
    next: 'calib.autoNextDown',
    otherSource: 'downTimeSource',
  },
  down: {
    start: 'calib.autoDown',
    prep: 'calib.autoDownPrep',
    done: 'calib.autoDoneDown',
    next: 'calib.autoNextUp',
    otherSource: 'upTimeSource',
  },
};

export function Calibrate({ shade, onFinished }: { shade: ShadeDto; onFinished: () => void }) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' });
  const [busy, setBusy] = useState(false);
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
          return;
        }
        // Everything else leaves the run where it was. An implausible finish is
        // the important one: the device deliberately keeps the run open so the
        // operator can press again, and dropping back to idle here would
        // discard a measurement that is still recoverable.
        setFailure(
          code === 'calibrationImplausible' ? 'calib.autoImplausible' : errorMessageKey(cause),
        );
      })
      .finally(() => setBusy(false));
  };

  const begin = (leg: CalibrationLegDto) =>
    send({ step: 'begin', leg }, () => setPhase({ kind: 'running', leg, startedMs: Date.now() }));

  const finish = (leg: CalibrationLegDto, startedMs: number) =>
    send({ step: 'finish' }, () => {
      setPhase({
        kind: 'idle',
        done: { leg, seconds: Math.round((Date.now() - startedMs) / 100) / 10 },
      });
      onFinished();
    });

  const cancel = () => send({ step: 'cancel' }, () => setPhase({ kind: 'idle' }));

  // A guided run on a shade no motor answers measures nothing: the device
  // transmits, nothing moves, and whatever the operator presses is stored as
  // `measured`. The panel above still accepts hand-entered values, which is what
  // R9 asks for and is exactly the case it asks for it in.
  const unpaired = shade.pairingState === 'awaitingConfirmation';

  return (
    <section class="panel">
      <h3>{t('calib.autoTitle')}</h3>
      <p class="prose">{t('calib.autoHint')}</p>

      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {t(failure)}
        </p>
      )}

      {unpaired ? (
        <p class="note note--warn">{t('calib.autoUnpaired', { name: shade.name })}</p>
      ) : phase.kind === 'idle' ? (
        <>
          {phase.done && <Finished done={phase.done} shade={shade} t={t} />}
          {/* Before the button, not after: the cost of pressing it is a full
              traverse of somebody's window, and a warning underneath a control
              is a warning read second. */}
          <p class="note note--warn">{t('calib.autoCost')}</p>
          {(['up', 'down'] as const).map((leg) => (
            <div key={leg}>
              <p class="field__hint">{t(LEG[leg].prep)}</p>
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
