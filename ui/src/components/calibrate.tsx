/**
 * The guided travel-time measurement (R2), and the two numbers it gets for free
 * (R5, R8).
 *
 * ## What this screen actually is
 *
 * Nothing on the device can see the shade. RTS is one-way, there is no encoder
 * and no limit-switch feedback, so the only instrument available is a person
 * watching the window and the device's own clock. "Automatic" here means the
 * device holds the stopwatch, not that it holds the eyes.
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
 * design that needed a run per number would be one people decline to use.
 *
 * ## Why every tap is optional, and why they are worth different amounts
 *
 * A human tap lands a couple of hundred milliseconds after what it aims at. The
 * band is the *difference* of two taps, so the operator's reaction delay
 * cancels out of it; the start delay is a single tap and carries it whole. So
 * the band is the reliable one, the delay is indicative, and the traverse —
 * seconds long — is the reliable one of all. Skipping a tap leaves that value
 * as it was rather than storing a worse one.
 *
 * ## What this screen may not claim
 *
 * That the shade did anything. `begin` queues a traverse and starts a clock;
 * whether the motor heard the frame is settled by the operator watching, which
 * is why the next control is "it has started moving" rather than a spinner.
 */
import { useEffect, useState } from 'preact/hooks';

import { calibrateShade } from '../api/client';
import { errorMessageKey } from '../api/errors';
import type { CalibrationLegDto } from '../api/generated/CalibrationLegDto';
import type { CalibrationMarkDto } from '../api/generated/CalibrationMarkDto';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { useT } from '../i18n';
import type { MessageKey } from '../i18n/en';

/** Which run is in progress here, if any. */
type Phase = { kind: 'idle' } | { kind: 'running'; leg: CalibrationLegDto; startedMs: number };

export function Calibrate({ shade, onFinished }: { shade: ShadeDto; onFinished: () => void }) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' });
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<MessageKey | undefined>(undefined);
  const [failure, setFailure] = useState<string | undefined>(undefined);
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
        setFailure(t(errorMessageKey(cause)));
        // A refused step ends the run on this screen: the device's copy is
        // gone or was never there, and leaving the controls up would offer
        // taps against a run that does not exist.
        setPhase({ kind: 'idle' });
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

  const finish = () =>
    send({ step: 'finish' }, () => {
      setPhase({ kind: 'idle' });
      setNote('calib.autoDone');
      onFinished();
    });

  const cancel = () =>
    send({ step: 'cancel' }, () => {
      setPhase({ kind: 'idle' });
      setNote(undefined);
    });

  return (
    <section class="panel">
      <h3>{t('calib.autoTitle')}</h3>
      <p class="field__hint">{t('calib.autoHint')}</p>

      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {t('calib.failed', { reason: failure })}
        </p>
      )}
      {note !== undefined && (
        <p class="note" role="status">
          {t(note)}
        </p>
      )}

      {phase.kind === 'idle' ? (
        <>
          <p class="field__hint">{t('calib.autoUpPrep')}</p>
          <div class="actions">
            <button type="button" class="btn" disabled={busy} onClick={() => begin('up')}>
              {t('calib.autoUp')}
            </button>
          </div>
          <p class="field__hint">{t('calib.autoDownPrep')}</p>
          <div class="actions">
            <button type="button" class="btn" disabled={busy} onClick={() => begin('down')}>
              {t('calib.autoDown')}
            </button>
          </div>
        </>
      ) : (
        <>
          <p class="note" role="status">
            {t('calib.autoRunning', { elapsed: String(elapsed) })}
          </p>
          <p class="field__hint">{t('calib.autoOptional')}</p>
          <div class="actions">
            <button type="button" class="btn" disabled={busy} onClick={() => mark('motionBegan')}>
              {t('calib.autoMarkMotion')}
            </button>
            <button type="button" class="btn" disabled={busy} onClick={() => mark('curtainMoved')}>
              {t(
                phase.leg === 'up' ? 'calib.autoMarkCurtainUp' : 'calib.autoMarkCurtainDown',
              )}
            </button>
          </div>
          <div class="actions">
            <button type="button" class="btn btn--primary" disabled={busy} onClick={finish}>
              {t('calib.autoFinish')}
            </button>
            <button type="button" class="btn btn--ghost" disabled={busy} onClick={cancel}>
              {t('calib.autoCancel')}
            </button>
          </div>
        </>
      )}
    </section>
  );
}
