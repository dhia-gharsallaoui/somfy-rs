/**
 * The pairing assistant — the second half of adding a shade, and the only way
 * a shade ever acquires Home Assistant entities.
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
 * 3. **The only acknowledgement is what a person sees.** RTS is one-way; the
 *    controller transmits and never hears back. So there is no success state to
 *    render and the assistant does not render one: it asks the user what
 *    happened, and *the user's answer* is what advances it.
 *
 * ## Why the last step is a functional test and not the jog
 *
 * The jog is a real signal and it is kept — step 3 says to watch for it — but
 * it is **not** what commits. Two reasons, and the second is the stronger:
 *
 * - A jog is a subtle up-and-down that is easy to miss, and missing it is
 *   indistinguishable from it not happening.
 * - A jog proves that *a frame arrived*. It does not prove the shade can be
 *   driven from here: the rolling code could be behind what the motor has
 *   already accepted, the travel times could be nonsense, the address could
 *   belong to a different channel. Pressing Open and watching the shade open
 *   proves the whole path, and it is the path the user will actually use.
 *
 * `docs/hardware-checklist.md`'s own sequence ends the same way — "**Test it**:
 * open and close the shade from Home Assistant's cover entity" — and this moves
 * that step inside the flow instead of leaving it as advice after the end.
 *
 * ## Nothing on this screen reports the device's own opinion of the position
 *
 * Deliberately. The position estimate advances when the controller *transmits*,
 * whether or not any motor heard, so a live position readout here would be the
 * transmitter reporting its own success — which this project treats as proving
 * nothing. The instrument is the person, and the screen must not compete with
 * them.
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
 * reporting its own success.
 */
import { useEffect, useState } from 'preact/hooks';
import { useLocation } from 'preact-iso/router';

import { commandShade, confirmPairing, deleteShade, getShade, pairShade } from '../api/client';
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

/** The four steps the user is walked through, before the outcome. */
const TOTAL_STEPS = 4;

type Stage =
  /** What you need before starting — including a remote this controller is not. */
  | { at: 'prepare' }
  /** Put the motor into programming mode. Owns the moment the window opens. */
  | { at: 'programming' }
  /** Transmit. `openedAt` is when step 2 was confirmed. */
  | { at: 'send'; openedAt: number; sent: boolean; failure: string | undefined }
  /** Drive the shade and see whether it obeys. The evidence that commits. */
  | { at: 'test'; failure: string | undefined }
  /** The operator reported it working, and the device has announced it. */
  | { at: 'done' }
  /** It did not work. Causes in the order the checklist ranks them. */
  | { at: 'retry' };

export function ShadePair({ device, id }: { device: DeviceState; id: number }) {
  const t = useT();
  const loaded = useShade(device, id);

  if (loaded.at !== 'ready') {
    return (
      <section class="panel">
        <p>{loaded.at === 'missing' ? t('detail.notFound', { id }) : t('dashboard.loading')}</p>
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
  if (loaded.shade.addressOrigin !== 'allocated') {
    return <Blocked shade={loaded.shade} device={device} t={t} />;
  }

  return <Assistant shade={loaded.shade} device={device} t={t} />;
}

// --------------------------------------------------------------------- loading

type Loaded = { at: 'ready'; shade: ShadeDto } | { at: 'missing' } | { at: 'loading' };

/**
 * The shade, from the dashboard's snapshot if it is there and from the device
 * directly if it is not.
 *
 * The fallback is not defensive padding. Adding a shade routes here the instant
 * the `POST` answers, which is *before* the reloaded snapshot arrives, so
 * without it the first thing a new shade's owner would see is "no shade with id
 * 6" — a screen saying the thing they just made does not exist. It also makes
 * this URL a real deep link: reloading the page mid-setup lands back here.
 */
function useShade(device: DeviceState, id: number): Loaded {
  const live = device.shade(id);
  const [fetched, setFetched] = useState<ShadeDto | undefined>(undefined);
  const [missing, setMissing] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setFetched(undefined);
    setMissing(false);
    getShade(id)
      .then((shade) => {
        if (!cancelled) setFetched(shade);
      })
      .catch(() => {
        if (!cancelled) setMissing(true);
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  const shade = live ?? fetched;
  if (shade) return { at: 'ready', shade };
  return missing ? { at: 'missing' } : { at: 'loading' };
}

// ------------------------------------------------------------------- blocked

/**
 * An address this controller did not allocate. Pairing it is a no-op, so the
 * assistant does not run.
 *
 * It is still offered a way *out* rather than being a dead end, and there are
 * two of them because there are two ways to arrive here. A shade whose setup was
 * finished elsewhere — a migrated table — is simply working, and says so. One
 * that is somehow both imported and unconfirmed cannot be finished by pairing,
 * so the honest offer is the one thing that is true: if it already responds,
 * say so and it is done.
 */
function Blocked({
  shade,
  device,
  t,
}: {
  shade: ShadeDto;
  device: DeviceState;
  t: Translate;
}) {
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
      {shade.pairingState === 'awaitingConfirmation' && (
        <ItAlreadyWorks shade={shade} device={device} t={t} />
      )}
    </div>
  );
}

/**
 * The escape hatch for a shade that cannot be paired from here and may already
 * respond: an imported address whose setup was never marked finished.
 *
 * Same report as the assistant's last step and the same wording of the
 * question, because it is the same claim — an operator saying they drove the
 * shade and it moved.
 */
function ItAlreadyWorks({
  shade,
  device,
  t,
}: {
  shade: ShadeDto;
  device: DeviceState;
  t: Translate;
}) {
  const { route } = useLocation();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const confirm = () => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    confirmPairing(shade.id)
      .then(() => {
        device.reload();
        route(`/shades/${shade.id}`);
      })
      .catch((cause: unknown) => {
        setFailure(t(errorMessageKey(cause)));
        setBusy(false);
      });
  };

  return (
    <section class="panel">
      <h3>{t('pair.alreadyTitle')}</h3>
      <p class="prose">{t('pair.alreadyBody', { name: shade.name })}</p>
      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {t('pair.confirmFailed', { reason: failure })}
        </p>
      )}
      <div class="actions">
        <Drive shade={shade} t={t} />
      </div>
      <div class="actions">
        <button type="button" class="btn btn--primary" disabled={busy} onClick={confirm}>
          {busy ? t('pair.confirming') : t('pair.alreadyConfirm')}
        </button>
      </div>
      <Abandon shade={shade} device={device} t={t} />
    </section>
  );
}

// ----------------------------------------------------------------- assistant

function Assistant({
  shade,
  device,
  t,
}: {
  shade: ShadeDto;
  device: DeviceState;
  t: Translate;
}) {
  const { route } = useLocation();
  const [stage, setStage] = useState<Stage>({ at: 'prepare' });

  const step =
    stage.at === 'prepare' ? 1 : stage.at === 'programming' ? 2 : stage.at === 'send' ? 3 : 4;
  const walking = stage.at !== 'done' && stage.at !== 'retry';

  return (
    <div class="detail">
      <nav class="detail__nav">
        <a class="link" href={`/shades/${shade.id}`}>
          ← {t('pair.blockedBack', { name: shade.name })}
        </a>
      </nav>

      <header class="detail__head">
        <h2>{t('pair.title', { name: shade.name })}</h2>
        {walking && (
          <p class="detail__kind">{t('pair.progress', { step, total: TOTAL_STEPS })}</p>
        )}
      </header>

      {/*
        The one thing this screen must never let a user forget while they are
        halfway through: right now, this shade does nothing and is in no
        Home Assistant.
      */}
      {shade.pairingState === 'awaitingConfirmation' && stage.at !== 'done' && (
        <p class="note note--warn">{t('pair.unfinished', { name: shade.name })}</p>
      )}

      {stage.at === 'prepare' && <Prepare t={t} onNext={() => setStage({ at: 'programming' })} />}

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
          onTest={() => setStage({ at: 'test', failure: undefined })}
          onNothing={() => setStage({ at: 'retry' })}
        />
      )}

      {stage.at === 'test' && (
        <Test
          shade={shade}
          device={device}
          t={t}
          stage={stage}
          setStage={setStage}
          onConfirmed={() => setStage({ at: 'done' })}
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

      {stage.at !== 'done' && <Abandon shade={shade} device={device} t={t} />}
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
  onTest,
  onNothing,
}: {
  shade: ShadeDto;
  t: Translate;
  stage: Extract<Stage, { at: 'send' }>;
  setStage: (stage: Stage) => void;
  onTest: () => void;
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
      // reported as a failure rather than folded into "did anything happen?".
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
        says nothing about whether the motor did. The jog is offered as a signal
        to watch for and is deliberately not a gate: it is easy to miss, and it
        proves only that a frame arrived. The next step is what proves the shade
        can be driven.
      */}
      {stage.sent && (
        <div class="outcome">
          <p class="outcome__sent" role="status">
            {t('pair.step3Sent')}
          </p>
          <p class="prose">{t('pair.step3NoFeedback')}</p>
          <div class="actions">
            <button type="button" class="btn btn--primary" onClick={onTest}>
              {t('pair.step3Next')}
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

/**
 * The step that decides. The operator drives the shade from here and says
 * whether it moved; only a `yes` announces it to Home Assistant.
 *
 * These are the same `POST /command` calls the dashboard tile makes, against a
 * shade that has no MQTT entities yet — which is exactly why the local API can
 * command an unannounced shade at all. Without that, the only way to test one
 * would be to announce it first, which is the thing being avoided.
 */
function Test({
  shade,
  device,
  t,
  stage,
  setStage,
  onConfirmed,
  onNothing,
}: {
  shade: ShadeDto;
  device: DeviceState;
  t: Translate;
  stage: Extract<Stage, { at: 'test' }>;
  setStage: (stage: Stage) => void;
  onConfirmed: () => void;
  onNothing: () => void;
}) {
  const [busy, setBusy] = useState(false);

  const confirm = () => {
    if (busy) return;
    setBusy(true);
    confirmPairing(shade.id)
      .then(() => {
        // Reload so the dashboard stops showing this as an unfinished setup.
        device.reload();
        onConfirmed();
      })
      .catch((cause: unknown) => {
        setStage({ ...stage, failure: t(errorMessageKey(cause)) });
        setBusy(false);
      });
  };

  return (
    <section class="panel">
      <h3>{t('pair.step4Title')}</h3>
      <p class="prose">{t('pair.step4Body', { name: shade.name })}</p>
      {/*
        The trap the hardware checklist already records: a command toward a
        limit the shade is already at produces no visible motion, which is
        indistinguishable from a frame that never arrived. Both directions are
        offered and the copy says why one of them may look like a failure.
      */}
      <p class="note">{t('pair.step4Limit')}</p>
      <p class="note">{t('pair.step4Why')}</p>

      <div class="actions">
        <Drive shade={shade} t={t} />
      </div>

      {stage.failure !== undefined && (
        <p class="panel panel--error" role="alert">
          {t('pair.confirmFailed', { reason: stage.failure })}
        </p>
      )}

      <div class="outcome">
        <h3>{t('pair.step4Question', { name: shade.name })}</h3>
        <p class="prose">{t('pair.step4OnlyYou', { name: shade.name })}</p>
        <div class="actions">
          <button type="button" class="btn btn--primary" disabled={busy} onClick={confirm}>
            {busy ? t('pair.confirming') : t('pair.step4Yes')}
          </button>
          <button type="button" class="btn" disabled={busy} onClick={onNothing}>
            {t('pair.step4No')}
          </button>
        </div>
      </div>
    </section>
  );
}

/**
 * Open and Close, and nothing else — no slider, no position readout.
 *
 * A slider would invite a part-open command, whose result depends on travel
 * times nobody has measured yet; a position readout would show the device's own
 * estimate, which advances on transmission whether or not a motor heard. Either
 * one turns "did it move?" into a question about the screen.
 */
function Drive({ shade, t }: { shade: ShadeDto; t: Translate }) {
  return (
    <>
      <button
        type="button"
        class="btn"
        aria-label={t('command.upAria', { name: shade.name })}
        onClick={() => void commandShade(shade.id, { action: 'up' })}
      >
        <span class="btn__glyph" aria-hidden="true">
          ▲
        </span>
        {t('command.up')}
      </button>
      <button
        type="button"
        class="btn"
        aria-label={t('command.downAria', { name: shade.name })}
        onClick={() => void commandShade(shade.id, { action: 'down' })}
      >
        <span class="btn__glyph" aria-hidden="true">
          ▼
        </span>
        {t('command.down')}
      </button>
    </>
  );
}

function Done({ shade, t, onLeave }: { shade: ShadeDto; t: Translate; onLeave: () => void }) {
  return (
    <section class="panel panel--good">
      <h3>{t('pair.doneTitle', { name: shade.name })}</h3>
      <p class="prose">{t('pair.doneWitness')}</p>
      <p class="prose">{t('pair.doneAnnounced', { name: shade.name })}</p>
      <p class="prose">{t('pair.doneEnd')}</p>
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
        <li>{t('pair.retryCode')}</li>
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
 * Abandoning a setup that was never finished.
 *
 * Offered only while the shade is unconfirmed, and it really does leave nothing
 * behind: no entity was ever published for it, so the device has nothing on the
 * broker to clear — unlike deleting a working shade, which is a different and
 * much louder action and lives on the detail screen with its own warning.
 *
 * The one thing that survives is the address's rolling code, and that is
 * correct rather than a leak: a counter that goes backwards is what makes a
 * motor stop obeying, so nothing here deletes one. If the same slot is used
 * again it gets the same address and carries on from where it was.
 */
function Abandon({
  shade,
  device,
  t,
}: {
  shade: ShadeDto;
  device: DeviceState;
  t: Translate;
}) {
  const { route } = useLocation();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  if (shade.pairingState !== 'awaitingConfirmation') return null;

  const discard = () => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    deleteShade(shade.id)
      .then(() => {
        device.reload();
        route('/');
      })
      .catch((cause: unknown) => {
        setFailure(t(errorMessageKey(cause)));
        setBusy(false);
      });
  };

  return (
    <section class="panel panel--danger">
      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {failure}
        </p>
      )}
      {confirming ? (
        <>
          <p class="prose">{t('pair.abandonWarning', { name: shade.name })}</p>
          <div class="actions">
            <button type="button" class="btn btn--danger" disabled={busy} onClick={discard}>
              {busy ? t('pair.abandoning') : t('pair.abandonConfirm', { name: shade.name })}
            </button>
            <button
              type="button"
              class="btn btn--ghost"
              disabled={busy}
              onClick={() => setConfirming(false)}
            >
              {t('pair.abandonCancel')}
            </button>
          </div>
        </>
      ) : (
        <div class="actions">
          <button type="button" class="btn" onClick={() => setConfirming(true)}>
            {t('pair.abandon')}
          </button>
        </div>
      )}
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
