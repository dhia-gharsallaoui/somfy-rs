/**
 * The pairing assistant.
 *
 * This screen teaches a procedure that happens mostly **away from the screen**,
 * and that is the constraint everything here is shaped by.
 * `docs/hardware-checklist.md` → "Pairing a shade" is the source; the three
 * facts that decide the design are:
 *
 * 1. **Something else has to start it.** The motor must be put into programming
 *    mode by a remote it already obeys, held for about two seconds. This
 *    controller cannot do it — a motor that has never heard of it ignores
 *    everything it sends, including the pairing frame. So the assistant's
 *    second step is an instruction to press a button that is not on this
 *    device, and it must say plainly that there is no way around it.
 * 2. **The window is about two minutes**, opened by that press and closed by a
 *    timer nobody here controls. Step 3 shows what is left of it, and says so
 *    approximately, because the clock started on a press this app never saw.
 * 3. **The only acknowledgement is the motor jogging.** RTS is one-way; the
 *    controller transmits and never hears back. So there is no success state to
 *    render and the assistant does not render one: after transmitting it asks
 *    the user what happened, and *the user's answer* is what advances it.
 *
 * ## What this deliberately does not have
 *
 * **No unpair control.** The firmware has none, on purpose: on a physical
 * remote pairing is a tapped PROG and unpairing is a *held* one, so the length
 * of the burst is the whole difference, and `somfy_domain::PAIR_REPEATS` pins
 * ours to a tap. An unpair button would be one mis-resolved repeat count away
 * from removing this controller from a shade that was working, and the cost is
 * paid at the window rather than at the keyboard.
 *
 * **No spinner that resolves into "paired".** A `202 Accepted` means the device
 * took the request. Rendering that as success would be the transmitter
 * reporting its own success, which this project treats as proving nothing.
 */
import { useEffect, useState } from 'preact/hooks';
import { useLocation } from 'preact-iso/router';

import { pairShade } from '../api/client';
import { errorMessageKey } from '../api/errors';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { clock } from '../components/format';
import { useT, type Translate } from '../i18n';
import type { DeviceState } from '../state/device';

/**
 * How long a motor stays in programming mode after the PROG hold, per the
 * hardware checklist ("roughly two minutes"). Used only to show the user what
 * is left; nothing branches on it expiring, because the real timer is in the
 * motor and this one started when a human said it did.
 */
const WINDOW_SECONDS = 120;

/** The three steps the user is walked through, before the outcome question. */
const TOTAL_STEPS = 3;

type Stage =
  /** What you need before starting — including a remote this controller is not. */
  | { at: 'prepare' }
  /** Put the motor into programming mode. Owns the moment the window opens. */
  | { at: 'programming' }
  /** Transmit, then ask what happened. `openedAt` is when step 2 was confirmed. */
  | { at: 'send'; openedAt: number; sent: boolean; failure: string | undefined }
  /** The user saw the jog. They are the sensor; the controller never was. */
  | { at: 'done' }
  /** The user saw nothing. Causes in the order the checklist ranks them. */
  | { at: 'retry' };

export function ShadePair({ device, id }: { device: DeviceState; id: number }) {
  const t = useT();
  const shade = device.shade(id);

  if (!shade) {
    return (
      <section class="panel">
        <p>{device.snapshot ? t('detail.notFound', { id }) : t('dashboard.loading')}</p>
        <a class="btn" href="/">
          {t('detail.back')}
        </a>
      </section>
    );
  }

  // The gate. Pairing a motor to an address another controller allocated
  // teaches it nothing it does not know and leaves the shared-identity clash
  // exactly where it was, so the assistant refuses to run rather than walking
  // somebody to a window for nothing.
  if (shade.addressOrigin !== 'allocated') {
    return <Blocked shade={shade} t={t} />;
  }

  return <Assistant shade={shade} t={t} />;
}

// ------------------------------------------------------------------- blocked

function Blocked({ shade, t }: { shade: ShadeDto; t: Translate }) {
  return (
    <div class="detail">
      <nav class="detail__nav">
        <a class="link" href={`/shades/${shade.id}`}>
          ← {t('pair.blockedBack', { name: shade.name })}
        </a>
      </nav>
      <section class="panel panel--error">
        <h2>{t('pair.blockedTitle')}</h2>
        <p class="prose">{t('pair.blockedBody', { name: shade.name })}</p>
        <p class="prose">{t('pair.blockedAdvice', { name: shade.name })}</p>
      </section>
    </div>
  );
}

// ----------------------------------------------------------------- assistant

function Assistant({ shade, t }: { shade: ShadeDto; t: Translate }) {
  const { route } = useLocation();
  const [stage, setStage] = useState<Stage>({ at: 'prepare' });

  const step = stage.at === 'prepare' ? 1 : stage.at === 'programming' ? 2 : 3;

  return (
    <div class="detail">
      <nav class="detail__nav">
        <a class="link" href={`/shades/${shade.id}`}>
          ← {t('pair.blockedBack', { name: shade.name })}
        </a>
      </nav>

      <header class="detail__head">
        <h2>{t('pair.title', { name: shade.name })}</h2>
        {stage.at !== 'done' && stage.at !== 'retry' && (
          <p class="detail__kind">{t('pair.progress', { step, total: TOTAL_STEPS })}</p>
        )}
      </header>

      {stage.at === 'prepare' && (
        <Prepare t={t} onNext={() => setStage({ at: 'programming' })} />
      )}

      {stage.at === 'programming' && (
        <Programming
          t={t}
          onBack={() => setStage({ at: 'prepare' })}
          onNext={() =>
            setStage({ at: 'send', openedAt: Date.now(), sent: false, failure: undefined })
          }
        />
      )}

      {stage.at === 'send' && (
        <Send
          shade={shade}
          t={t}
          stage={stage}
          setStage={setStage}
          onJogged={() => setStage({ at: 'done' })}
          onNothing={() => setStage({ at: 'retry' })}
        />
      )}

      {stage.at === 'done' && (
        <Done shade={shade} t={t} onLeave={() => route(`/shades/${shade.id}`)} />
      )}

      {stage.at === 'retry' && (
        <Retry
          shade={shade}
          t={t}
          onAgain={() => setStage({ at: 'programming' })}
          onStop={() => route(`/shades/${shade.id}`)}
        />
      )}
    </div>
  );
}

function Prepare({ t, onNext }: { t: Translate; onNext: () => void }) {
  return (
    <section class="panel">
      <h3>{t('pair.step1Title')}</h3>
      <ul class="steps">
        <li>{t('pair.step1Remote')}</li>
        <li>{t('pair.step1See')}</li>
        <li>{t('pair.step1Still')}</li>
      </ul>
      <p class="note">{t('pair.additive')}</p>
      <div class="actions">
        <button type="button" class="btn btn--primary" onClick={onNext}>
          {t('pair.step1Next')}
        </button>
      </div>
    </section>
  );
}

function Programming({
  t,
  onBack,
  onNext,
}: {
  t: Translate;
  onBack: () => void;
  onNext: () => void;
}) {
  return (
    <section class="panel">
      <h3>{t('pair.step2Title')}</h3>
      <ol class="steps steps--numbered">
        <li>{t('pair.step2Hold')}</li>
        <li>{t('pair.step2Recessed')}</li>
        <li>{t('pair.step2Channel')}</li>
      </ol>
      <p class="note">{t('pair.step2Window')}</p>
      <div class="actions">
        <button type="button" class="btn btn--primary" onClick={onNext}>
          {t('pair.step2Next')}
        </button>
        <button type="button" class="btn btn--ghost" onClick={onBack}>
          {t('pair.step2Back')}
        </button>
      </div>
    </section>
  );
}

function Send({
  shade,
  t,
  stage,
  setStage,
  onJogged,
  onNothing,
}: {
  shade: ShadeDto;
  t: Translate;
  stage: Extract<Stage, { at: 'send' }>;
  setStage: (stage: Stage) => void;
  onJogged: () => void;
  onNothing: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const remaining = useCountdown(stage.openedAt);

  const send = () => {
    if (busy) return;
    setBusy(true);
    pairShade(shade.id)
      .then(() => setStage({ ...stage, sent: true, failure: undefined }))
      // A refusal here is the device declining to transmit at all, which is a
      // different thing from a frame that went out and was not heard — so it is
      // reported as a failure rather than folded into "did it jog?".
      .catch((cause: unknown) => setStage({ ...stage, failure: t(errorMessageKey(cause)) }))
      .finally(() => setBusy(false));
  };

  return (
    <section class="panel">
      <h3>{t('pair.step3Title')}</h3>
      <p class="prose">{t('pair.step3Body', { name: shade.name })}</p>

      <p class={`note${remaining === 0 ? ' note--warn' : ''}`} role="status">
        {remaining > 0
          ? t('pair.step3Remaining', { time: clock(remaining) })
          : t('pair.step3Expired')}
      </p>

      {stage.failure !== undefined && (
        <p class="panel panel--error" role="alert">
          {t('pair.step3Failed', { reason: stage.failure })}
        </p>
      )}

      <div class="actions">
        <button type="button" class="btn btn--primary" disabled={busy} onClick={send}>
          {busy ? t('pair.step3Sending') : t('pair.step3Send')}
        </button>
      </div>

      {/*
        Everything below appears only after the device accepted the request, and
        says nothing about whether the motor did. The question is the whole
        point: the user is the only instrument this procedure has.
      */}
      {stage.sent && (
        <div class="outcome">
          <p class="outcome__sent" role="status">
            {t('pair.step3Sent')}
          </p>
          <p class="prose">{t('pair.step3NoFeedback')}</p>
          <h3>{t('pair.step3Question')}</h3>
          <div class="actions">
            <button type="button" class="btn btn--primary" onClick={onJogged}>
              {t('pair.step3Yes')}
            </button>
            <button type="button" class="btn" onClick={onNothing}>
              {t('pair.step3No')}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

function Done({
  shade,
  t,
  onLeave,
}: {
  shade: ShadeDto;
  t: Translate;
  onLeave: () => void;
}) {
  return (
    <section class="panel panel--good">
      <h3>{t('pair.doneTitle', { name: shade.name })}</h3>
      <p class="prose">{t('pair.doneWitness')}</p>
      <p class="prose">{t('pair.doneEnd')}</p>
      <p class="prose">{t('pair.doneTest', { name: shade.name })}</p>
      <div class="actions">
        <button type="button" class="btn btn--primary" onClick={onLeave}>
          {t('pair.doneBack', { name: shade.name })}
        </button>
      </div>
    </section>
  );
}

function Retry({
  shade,
  t,
  onAgain,
  onStop,
}: {
  shade: ShadeDto;
  t: Translate;
  onAgain: () => void;
  onStop: () => void;
}) {
  return (
    <section class="panel">
      <h3>{t('pair.retryTitle')}</h3>
      <p class="prose">{t('pair.retryIntro')}</p>
      {/* Ordered, and the order is the checklist's: cheapest check first. */}
      <ol class="steps steps--numbered">
        <li>{t('pair.retryWindow')}</li>
        <li>{t('pair.retryChannel')}</li>
        <li>{t('pair.retryRange')}</li>
      </ol>
      <p class="note">{t('pair.additive')}</p>
      <div class="actions">
        <button type="button" class="btn btn--primary" onClick={onAgain}>
          {t('pair.retryAgain')}
        </button>
        <button type="button" class="btn btn--ghost" onClick={onStop}>
          {t('pair.retryStop', { name: shade.name })}
        </button>
      </div>
    </section>
  );
}

/**
 * Seconds left of the programming window, counting down from when the user
 * confirmed the motor jogged.
 *
 * An estimate, and labelled as one in the copy: the motor's timer started at
 * the PROG press, which happened at a window this app cannot see. It is shown
 * because "you have about ninety seconds" changes what somebody does next, and
 * nothing branches on it reaching zero — the assistant keeps the send button
 * live, because a window that turns out to have been longer is common and a
 * disabled button would be the app overruling what the user can see.
 */
function useCountdown(openedAt: number): number {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  return Math.max(0, WINDOW_SECONDS - Math.floor((now - openedAt) / 1000));
}
