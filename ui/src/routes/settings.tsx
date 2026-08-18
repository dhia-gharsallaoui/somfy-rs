/**
 * The settings screen: the network the device joins, and the broker it
 * publishes to.
 *
 * ## Three things here are not ordinary form handling
 *
 * **A secret is never shown, because it is never sent.** `SettingsDto` has no
 * field a passphrase or a broker password could arrive in — see
 * `crates/somfy-api/src/settings.rs` — so this screen could not prefill one if
 * it wanted to. What it gets instead is `pskSet` / `passwordSet`, and what it
 * shows is *whether* one is set. Every secret input is therefore a three-way
 * choice rather than a text box: keep the stored one, type a new one, or have
 * none. An empty text box would have meant one of those three and nobody would
 * know which.
 *
 * **Changing Wi-Fi is a test, not a save.** The device puts the candidate on
 * the radio and leaves the network this page arrived over. It puts the stored
 * credential back — by restarting, which is how it avoids ever holding two
 * passphrases — unless somebody reaches it on the *new* network and confirms.
 * So the flow is deliberately shaped around losing this connection: the warning
 * before, the "the device has left" state during, and a confirm button that is
 * only reachable from the other side. `somfy_config::WifiTrial` carries the
 * argument for why association is not the test.
 *
 * **Changing the broker restarts the device, and the screen says so.** Not an
 * implementation detail: the restart is what makes the retained Home Assistant
 * entities published under the *previous* topic namespaces get deleted before
 * the new ones go out (requirements spec R5). The device recovers those old
 * namespaces by re-scanning its configuration ring at boot, which is the only
 * place they still exist.
 *
 * ## Why the polling is shaped the way it is
 *
 * While a trial runs this screen polls, and the poll is *expected to fail* —
 * the device is on another network. A failed poll during a trial is therefore
 * not an error state; it is the "join the other network" state, which is the
 * one piece of information the operator actually needs at that moment.
 */
import { useEffect, useState } from 'preact/hooks';

import {
  cancelWifiTrial,
  clearMqtt,
  confirmWifi,
  getSettings,
  saveMqtt,
  startWifiTrial,
} from '../api/client';
import { ApiError, FIELD_LABEL, errorMessageKey } from '../api/errors';
import type { SecretDto } from '../api/generated/SecretDto';
import type { SettingsDto } from '../api/generated/SettingsDto';
import type { SettingsFieldDto } from '../api/generated/SettingsFieldDto';
import type { TrialPhaseDto } from '../api/generated/TrialPhaseDto';
import type { WifiTrialDto } from '../api/generated/WifiTrialDto';
import { useT, type Translate } from '../i18n';
import type { MessageKey } from '../i18n/en';

/**
 * How often the settings are re-read while a credential trial is live.
 *
 * Two seconds, and the figure is about the operator rather than the device: it
 * is how long "the device has left this network" takes to appear after the
 * radio moves, and how quickly the page notices that it is now *on* the new
 * network and can offer the confirm button. The device's own deadlines are
 * forty-five seconds and three minutes, so nothing here is racing them.
 */
const TRIAL_POLL_MS = 2_000;

/** How often the settings are re-read otherwise: never, until something changes. */
const IDLE_POLL_MS = 0;

/**
 * How the three states of a write-only secret are labelled.
 *
 * Total over the generated {@link SecretDto}'s tag, so a fourth meaning added
 * in Rust fails `tsc` here rather than quietly disappearing from a radio group
 * — which would leave an operator unable to express it at all.
 */
const SECRET_CHOICE: Record<SecretDto['secret'], MessageKey> = {
  keep: 'settings.secretKeep',
  set: 'settings.secretSet',
  clear: 'settings.secretClear',
};

/**
 * What each trial phase says. Total over the generated {@link TrialPhaseDto}.
 */
const PHASE_TEXT: Record<TrialPhaseDto, MessageKey> = {
  associating: 'settings.trialAssociating',
  awaitingConfirmation: 'settings.trialAwaiting',
};

/** Everything the screen knows about the device, or why it does not. */
type Loaded =
  | { at: 'loading' }
  | { at: 'ready'; settings: SettingsDto }
  // Unreachable **and no trial is known to be running**, which is an ordinary
  // network error. The other case — unreachable during a trial — is not an
  // error and is held in `lastTrial` below.
  | { at: 'unreachable'; detail: string };

export function Settings() {
  const t = useT();
  const [loaded, setLoaded] = useState<Loaded>({ at: 'loading' });
  /**
   * The last trial this page saw, kept across a failed poll.
   *
   * This is the whole reason the screen behaves during the outage: the device
   * disappearing *while a trial is live* means it has moved to the candidate
   * network, which is the expected next event rather than a fault. Without
   * remembering the trial, the page would show a generic "could not reach the
   * device" at exactly the moment the operator needs to be told which network
   * to join.
   */
  const [lastTrial, setLastTrial] = useState<WifiTrialDto | undefined>(undefined);
  const [reloadToken, setReloadToken] = useState(0);
  /**
   * How a trial ended, held **here** rather than inside the panel that ended it.
   *
   * The panel unmounts the moment the trial goes away, so a message set inside
   * it would be gone before it was read. That is not cosmetic on a cancel: the
   * device restarts onto the stored network, so the very next poll fails — and
   * without this the screen would fall into "the device has left, go and join
   * {candidate}", which is the opposite of what just happened.
   */
  const [settled, setSettled] = useState<MessageKey | undefined>(undefined);

  const reload = () => setReloadToken((token) => token + 1);

  const trial = (loaded.at === 'ready' ? loaded.settings.wifiTrial : undefined) ?? lastTrial;

  useEffect(() => {
    let cancelled = false;
    const fetchOnce = () => {
      getSettings()
        .then((settings) => {
          if (cancelled) return;
          setLoaded({ at: 'ready', settings });
          setLastTrial(settings.wifiTrial ?? undefined);
        })
        .catch((cause: unknown) => {
          if (cancelled) return;
          setLoaded({
            at: 'unreachable',
            detail: cause instanceof ApiError ? cause.message : String(cause),
          });
        });
    };
    fetchOnce();
    // Polling only while a trial is live. The rest of the time the settings
    // change when this screen changes them and at no other moment, so a timer
    // would be a request every two seconds forever for nothing.
    const period = trial ? TRIAL_POLL_MS : IDLE_POLL_MS;
    if (period === 0) return () => void (cancelled = true);
    const timer = setInterval(fetchOnce, period);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [reloadToken, trial !== undefined]);

  if (loaded.at === 'loading') {
    return (
      <section class="panel">
        <p>{t('settings.loading')}</p>
      </section>
    );
  }

  // A trial this page has just ended. Outranks everything below, including the
  // unreachable branch: after a cancel the device *is* unreachable, because it
  // is restarting onto the credential it never stopped having, and reporting
  // that as a fault would be wrong twice over.
  if (settled) {
    return (
      <div class="detail">
        <Head t={t} />
        <section class="panel panel--good">
          <p role="status">{t(settled)}</p>
          {settled === 'settings.trialCancelled' && <p class="note">{t('settings.restarting')}</p>}
          <button
            type="button"
            class="btn"
            onClick={() => {
              setSettled(undefined);
              setLastTrial(undefined);
              reload();
            }}
          >
            {t('settings.retry')}
          </button>
        </section>
      </div>
    );
  }

  // Unreachable *and* a trial is running: the device has moved, which is what
  // was asked of it. Told as an instruction, not as a failure.
  if (loaded.at === 'unreachable' && trial) {
    return (
      <div class="detail">
        <Head t={t} />
        <TrialPanel trial={trial} live={false} onSettled={setSettled} t={t} />
      </div>
    );
  }

  if (loaded.at === 'unreachable') {
    return (
      <div class="detail">
        <Head t={t} />
        <section class="panel panel--error" role="alert">
          <p>{t('settings.unreachable', { detail: loaded.detail })}</p>
          <button type="button" class="btn" onClick={reload}>
            {t('settings.retry')}
          </button>
        </section>
      </div>
    );
  }

  const { settings } = loaded;
  return (
    <div class="detail">
      <Head t={t} />
      {trial && <TrialPanel trial={trial} live onSettled={setSettled} t={t} />}
      {/*
        The Wi-Fi form is hidden while a trial runs rather than disabled: a
        second candidate is refused by the device anyway (`trialInProgress`),
        and offering a form whose submit cannot succeed is worse than not
        offering it.
      */}
      {!trial && <WifiPanel wifi={settings.wifi} onStarted={reload} t={t} />}
      <MqttPanel mqtt={settings.mqtt} t={t} />
    </div>
  );
}

function Head({ t }: { t: Translate }) {
  return (
    <>
      <nav class="detail__nav">
        <a class="link" href="/">
          ← {t('detail.back')}
        </a>
      </nav>
      <header class="detail__head">
        <h2>{t('settings.title')}</h2>
      </header>
    </>
  );
}

// ---------------------------------------------------------------------------
// Wi-Fi
// ---------------------------------------------------------------------------

function WifiPanel({
  wifi,
  onStarted,
  t,
}: {
  wifi: SettingsDto['wifi'];
  onStarted: () => void;
  t: Translate;
}) {
  const [ssid, setSsid] = useState(wifi?.ssid ?? '');
  const [psk, setPsk] = useState<SecretDto['secret']>(wifi ? 'keep' : 'set');
  const [pskValue, setPskValue] = useState('');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);
  const [badField, setBadField] = useState<SettingsFieldDto | undefined>(undefined);

  const changed = ssid !== (wifi?.ssid ?? '') || psk !== 'keep';
  const ready = ssid.length > 0 && changed && !(psk === 'set' && pskValue.length === 0);

  const submit = (event: Event) => {
    event.preventDefault();
    if (!ready || busy) return;
    setBusy(true);
    setFailure(undefined);
    setBadField(undefined);
    startWifiTrial({ ssid, psk: secretOf(psk, pskValue) })
      .then(() => {
        // Not "saved" — a trial has started, and the next thing that happens is
        // this page losing the device. `onStarted` re-reads so the trial panel
        // takes over.
        onStarted();
      })
      .catch((cause: unknown) => {
        setFailure(t(errorMessageKey(cause), fieldParam(cause, t)));
        setBadField(cause instanceof ApiError ? cause.field : undefined);
        setBusy(false);
      });
  };

  return (
    <section class="panel">
      <h3>{t('settings.wifiTitle')}</h3>
      <p class="prose">{t('settings.wifiIntro')}</p>
      {!wifi && <p class="note note--warn">{t('settings.wifiNone')}</p>}

      <form onSubmit={submit}>
        <label class="field">
          <span class="field__label">{t('settings.wifiSsid')}</span>
          <input
            type="text"
            class={`field__input${badField === 'ssid' ? ' field__input--bad' : ''}`}
            value={ssid}
            autoComplete="off"
            required
            onInput={(event) => setSsid(event.currentTarget.value)}
          />
        </label>

        <SecretField
          label={t('settings.wifiPsk')}
          name="psk"
          stored={wifi ? wifi.pskSet : false}
          storedText={
            wifi ? (wifi.pskSet ? 'settings.wifiPskStored' : 'settings.wifiPskOpen') : undefined
          }
          canKeep={wifi !== null}
          choice={psk}
          onChoice={setPsk}
          value={pskValue}
          onValue={setPskValue}
          bad={badField === 'psk'}
          t={t}
        />

        {changed && (
          <p class="note note--warn" role="status">
            {wifi
              ? t('settings.wifiWarn', {
                  ssid,
                  current: wifi.ssid,
                  minutes: 3,
                })
              : t('settings.wifiWarnNoCurrent', { ssid, minutes: 3 })}
          </p>
        )}

        {failure !== undefined && (
          <p class="note note--warn" role="alert">
            {t('settings.failed', { reason: failure })}
          </p>
        )}

        <div class="actions">
          <button type="submit" class="btn btn--primary" disabled={!ready || busy}>
            {busy ? t('settings.wifiSubmitting') : t('settings.wifiSubmit')}
          </button>
        </div>
      </form>
    </section>
  );
}

/**
 * The live trial.
 *
 * `live` is false when the device is unreachable, which during a trial is the
 * *expected* state rather than a fault — the device is on the other network, so
 * the instruction is to go there.
 */
function TrialPanel({
  trial,
  live,
  onSettled,
  t,
}: {
  trial: WifiTrialDto;
  live: boolean;
  onSettled: (message: MessageKey) => void;
  t: Translate;
}) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  // How it ended is reported **upwards**, not kept here: this panel unmounts as
  // soon as the trial goes away, and after a cancel the device is restarting, so
  // a message held here would be gone before it was read and the screen would
  // fall into "the device has left, go and join {candidate}" — the opposite of
  // what just happened.
  const act = (run: () => Promise<void>, message: MessageKey) => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    run()
      .then(() => onSettled(message))
      .catch((cause: unknown) => {
        setFailure(t(errorMessageKey(cause), fieldParam(cause, t)));
        setBusy(false);
      });
  };

  const seconds = Math.ceil(trial.remainingMs / 1000);

  return (
    <section class="panel panel--pending">
      <h3>{t('settings.trialTitle', { ssid: trial.ssid })}</h3>

      {
        <>
          <p class="prose">
            {live
              ? t(PHASE_TEXT[trial.phase], { ssid: trial.ssid, seconds })
              : t('settings.trialLeft', { ssid: trial.ssid })}
          </p>
          {live && <p class="note">{t('settings.trialRemaining', { seconds })}</p>}

          {failure !== undefined && (
            <p class="note note--warn" role="alert">
              {t('settings.failed', { reason: failure })}
            </p>
          )}

          <div class="actions">
            {/*
              Offered whenever the page can reach the device, which is the
              honest test: reaching it is exactly what confirming claims. On the
              old network the device is gone and there is nothing to press; on
              the new one it answers, and that answer *is* the evidence.
            */}
            <button
              type="button"
              class="btn btn--primary"
              disabled={busy || !live}
              onClick={() => act(confirmWifi, 'settings.trialSaved')}
            >
              {busy ? t('settings.trialConfirming') : t('settings.trialConfirm')}
            </button>
            <button
              type="button"
              class="btn btn--ghost"
              disabled={busy || !live}
              onClick={() => act(cancelWifiTrial, 'settings.trialCancelled')}
            >
              {t('settings.trialCancel')}
            </button>
          </div>
        </>
      }
    </section>
  );
}

// ---------------------------------------------------------------------------
// Broker
// ---------------------------------------------------------------------------

function MqttPanel({ mqtt, t }: { mqtt: SettingsDto['mqtt']; t: Translate }) {
  const [address, setAddress] = useState(mqtt?.address ?? '');
  const [port, setPort] = useState(mqtt?.port ?? 1883);
  const [username, setUsername] = useState(mqtt?.username ?? '');
  const [password, setPassword] = useState<SecretDto['secret']>(mqtt ? 'keep' : 'clear');
  const [passwordValue, setPasswordValue] = useState('');
  const [discoveryPrefix, setDiscoveryPrefix] = useState(mqtt?.discoveryPrefix ?? 'homeassistant');
  const [stateRoot, setStateRoot] = useState(mqtt?.stateRoot ?? 'somfyrs');
  const [busy, setBusy] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);
  const [badField, setBadField] = useState<SettingsFieldDto | undefined>(undefined);
  const [settled, setSettled] = useState<MessageKey | undefined>(undefined);

  const ready =
    address.length > 0 && discoveryPrefix.length > 0 && stateRoot.length > 0 && !(password === 'set' && passwordValue.length === 0);

  const run = (work: Promise<void>, message: MessageKey) => {
    setFailure(undefined);
    setBadField(undefined);
    work
      .then(() => setSettled(message))
      .catch((cause: unknown) => {
        setFailure(t(errorMessageKey(cause), fieldParam(cause, t)));
        setBadField(cause instanceof ApiError ? cause.field : undefined);
      })
      .finally(() => {
        setBusy(false);
        setClearing(false);
      });
  };

  const submit = (event: Event) => {
    event.preventDefault();
    if (!ready || busy || clearing) return;
    setBusy(true);
    run(
      saveMqtt({
        address,
        port,
        username,
        password: secretOf(password, passwordValue),
        discoveryPrefix,
        stateRoot,
      }),
      'settings.mqttSaved',
    );
  };

  if (settled) {
    return (
      <section class="panel panel--good">
        <h3>{t('settings.mqttTitle')}</h3>
        <p role="status">{t(settled)}</p>
        <p class="note">{t('settings.restarting')}</p>
      </section>
    );
  }

  return (
    <section class="panel">
      <h3>{t('settings.mqttTitle')}</h3>
      <p class="prose">{t('settings.mqttIntro')}</p>
      {!mqtt && <p class="note">{t('settings.mqttNone')}</p>}

      <form onSubmit={submit}>
        <label class="field">
          <span class="field__label">{t('settings.mqttAddress')}</span>
          <input
            type="text"
            inputMode="numeric"
            class={`field__input${badField === 'brokerAddress' ? ' field__input--bad' : ''}`}
            value={address}
            placeholder="192.168.1.10"
            autoComplete="off"
            required
            onInput={(event) => setAddress(event.currentTarget.value)}
          />
        </label>

        <label class="field field--inline">
          <span class="field__label">{t('settings.mqttPort')}</span>
          <input
            type="number"
            class={`field__input field__input--short${badField === 'brokerPort' ? ' field__input--bad' : ''}`}
            min={1}
            max={65535}
            step={1}
            value={port}
            required
            onInput={(event) => setPort(Number(event.currentTarget.value))}
          />
        </label>

        <label class="field">
          <span class="field__label">{t('settings.mqttUsername')}</span>
          <input
            type="text"
            class={`field__input${badField === 'brokerUsername' ? ' field__input--bad' : ''}`}
            value={username}
            autoComplete="off"
            aria-describedby="mqtt-username-hint"
            onInput={(event) => setUsername(event.currentTarget.value)}
          />
        </label>
        <p id="mqtt-username-hint" class="field__hint">
          {t('settings.mqttUsernameHint')}
        </p>

        <SecretField
          label={t('settings.mqttPassword')}
          name="mqtt-password"
          stored={mqtt ? mqtt.passwordSet : false}
          storedText={
            mqtt
              ? mqtt.passwordSet
                ? 'settings.mqttPasswordStored'
                : 'settings.mqttPasswordNone'
              : undefined
          }
          canKeep={mqtt !== null && mqtt.passwordSet}
          choice={password}
          onChoice={setPassword}
          value={passwordValue}
          onValue={setPasswordValue}
          bad={badField === 'brokerPassword'}
          t={t}
        />

        <label class="field">
          <span class="field__label">{t('settings.mqttDiscoveryPrefix')}</span>
          <input
            type="text"
            class={`field__input${badField === 'discoveryPrefix' ? ' field__input--bad' : ''}`}
            value={discoveryPrefix}
            autoComplete="off"
            required
            aria-describedby="mqtt-prefix-hint"
            onInput={(event) => setDiscoveryPrefix(event.currentTarget.value)}
          />
        </label>
        <p id="mqtt-prefix-hint" class="field__hint">
          {t('settings.mqttDiscoveryPrefixHint')}
        </p>

        <label class="field">
          <span class="field__label">{t('settings.mqttStateRoot')}</span>
          <input
            type="text"
            class={`field__input${badField === 'stateRoot' ? ' field__input--bad' : ''}`}
            value={stateRoot}
            autoComplete="off"
            required
            aria-describedby="mqtt-root-hint"
            onInput={(event) => setStateRoot(event.currentTarget.value)}
          />
        </label>
        <p id="mqtt-root-hint" class="field__hint">
          {t('settings.mqttStateRootHint')}
        </p>

        <p class="note note--warn">{t('settings.mqttWarn')}</p>

        {failure !== undefined && (
          <p class="note note--warn" role="alert">
            {t('settings.failed', { reason: failure })}
          </p>
        )}

        <div class="actions">
          <button type="submit" class="btn btn--primary" disabled={!ready || busy || clearing}>
            {busy ? t('settings.mqttSubmitting') : t('settings.mqttSubmit')}
          </button>
          {/*
            Two clicks, expanded inline, never `confirm()` — the same pattern
            `delete-shade.tsx` uses. Removing the broker retires every Home
            Assistant entity this device owns, and doing that by a stray click
            on a mobile keyboard is not recoverable from the screen.
          */}
          {mqtt && (
            <ClearBroker
              busy={busy || clearing}
              onConfirm={() => {
                setClearing(true);
                run(clearMqtt(), 'settings.mqttCleared');
              }}
              t={t}
            />
          )}
        </div>
      </form>
    </section>
  );
}

function ClearBroker({
  busy,
  onConfirm,
  t,
}: {
  busy: boolean;
  onConfirm: () => void;
  t: Translate;
}) {
  const [asking, setAsking] = useState(false);
  if (!asking) {
    return (
      <button type="button" class="btn btn--ghost" disabled={busy} onClick={() => setAsking(true)}>
        {t('settings.mqttClear')}
      </button>
    );
  }
  return (
    <>
      <span class="note note--warn">{t('settings.mqttConfirmClear')}</span>
      <button type="button" class="btn btn--danger" disabled={busy} onClick={onConfirm}>
        {busy ? t('settings.mqttClearing') : t('settings.mqttClear')}
      </button>
      <button type="button" class="btn btn--ghost" disabled={busy} onClick={() => setAsking(false)}>
        {t('detail.back')}
      </button>
    </>
  );
}

// ---------------------------------------------------------------------------
// A write-only secret
// ---------------------------------------------------------------------------

/**
 * The three things an operator can mean about a secret, as three radio buttons.
 *
 * A text box would have been one box meaning three things, and an empty one
 * would have been ambiguous between "leave it" and "remove it" — which is the
 * ambiguity `SecretDto` exists to remove. `canKeep` is false when there is
 * nothing stored to keep, so the impossible choice is not offered rather than
 * offered and refused.
 */
function SecretField({
  label,
  name,
  stored,
  storedText,
  canKeep,
  choice,
  onChoice,
  value,
  onValue,
  bad,
  t,
}: {
  label: string;
  name: string;
  stored: boolean;
  storedText: MessageKey | undefined;
  canKeep: boolean;
  choice: SecretDto['secret'];
  onChoice: (choice: SecretDto['secret']) => void;
  value: string;
  onValue: (value: string) => void;
  bad: boolean;
  t: Translate;
}) {
  const choices: SecretDto['secret'][] = canKeep ? ['keep', 'set', 'clear'] : ['set', 'clear'];
  return (
    <fieldset class="field">
      <legend class="field__label">{label}</legend>
      {storedText && <p class="field__hint">{t(storedText)}</p>}
      {choices.map((option) => (
        <label key={option} class="field field--inline">
          <input
            type="radio"
            name={name}
            checked={choice === option}
            value={option}
            onChange={() => onChoice(option)}
          />
          <span class="field__label">{t(SECRET_CHOICE[option])}</span>
        </label>
      ))}
      {choice === 'set' && (
        <input
          type="password"
          class={`field__input${bad ? ' field__input--bad' : ''}`}
          value={value}
          autoComplete="new-password"
          aria-label={label}
          onInput={(event) => onValue(event.currentTarget.value)}
        />
      )}
      {/* `stored` is deliberately unused for rendering the value: there is none
          to render. It only decides whether "keep" was ever an option, which
          the caller has already applied through `canKeep`. */}
      <span class="visually-hidden">{stored ? '' : ''}</span>
    </fieldset>
  );
}

/** Build the wire form of a secret from the radio choice and the text box. */
function secretOf(choice: SecretDto['secret'], value: string): SecretDto {
  switch (choice) {
    case 'keep':
      return { secret: 'keep' };
    case 'clear':
      return { secret: 'clear' };
    case 'set':
      return { secret: 'set', value };
  }
}

/**
 * The `{field}` a settings message interpolates, translated.
 *
 * Every settings message is written to read with a field name in it, because
 * the device answers with a rule and a field as two separate things — see
 * `crates/somfy-api/src/errors.rs`. A rejection that names no field passes an
 * empty string, which only happens for messages that do not use `{field}`.
 */
function fieldParam(cause: unknown, t: Translate): Record<string, string> {
  const field = cause instanceof ApiError ? cause.field : undefined;
  return { field: field ? t(FIELD_LABEL[field]) : '' };
}
