/**
 * Adding a shade — the first half of the flow that ends at the pairing
 * assistant.
 *
 * ## The shape of the screen follows what the device actually decides
 *
 * The form asks for six things and no more, because six is what
 * `CreateShadeDto` carries. The id and the remote address are **not** fields:
 * the device allocates the address out of its own space so that no other
 * controller transmits at it, and a form field would hand that decision to
 * whoever is typing — which is precisely the two-controllers-one-identity
 * failure the allocator exists to end.
 *
 * ## Why there is a confirmation step rather than a redirect
 *
 * A newly allocated address is one **no motor has ever heard**, so the shade
 * that just appeared on the dashboard will not move. That is surprising enough
 * to be worth a screen of its own rather than a toast: it is the single fact
 * that decides whether the user walks to the window now or wonders later why
 * nothing works.
 *
 * ## Validation is a courtesy, not the authority
 *
 * Every rule checked here is checked again by the device
 * (`CreateShadeDto::to_config`), which is the one that counts. The client-side
 * copy exists to disable the button rather than to make a round trip to say
 * "the name is empty" — and when the device refuses anyway, its typed code is
 * what gets rendered, translated through `src/api/errors.ts`.
 */
import { useState } from 'preact/hooks';
import { useLocation } from 'preact-iso/router';

import { createShade } from '../api/client';
import { errorMessageKey } from '../api/errors';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { formatAddress } from '../components/format';
import { KIND_OPTIONS, TILT_NONE, TILT_OPTIONS } from '../components/kind';
import { useT, type Translate } from '../i18n';
import type { DeviceState } from '../state/device';

/** `somfy_api::NAME_MAX_BYTES` — the capacity of `heapless::String<32>`. */
const NAME_MAX_BYTES = 32;

/**
 * The limit is a **byte** capacity, and `String.length` counts UTF-16 code
 * units. "Chambre à coucher côté rue" is 26 of those and 28 bytes, so a UI
 * counting characters would cheerfully offer a name the device refuses.
 */
const nameBytes = (name: string): number => new TextEncoder().encode(name).length;

const DEFAULTS = {
  // `ShadeConfig::new`'s factory defaults, which are what a shade provisioned
  // without a measurement gets. Offered as a starting point rather than left
  // blank, because a plausible number the user corrects beats an empty field.
  upTimeMs: 10_000,
  downTimeMs: 10_000,
  tiltTimeMs: 7_000,
} as const;

export function ShadeNew({ device }: { device: DeviceState }) {
  const t = useT();
  const [created, setCreated] = useState<ShadeDto | undefined>(undefined);

  return created ? (
    <Created shade={created} t={t} />
  ) : (
    <Form device={device} onCreated={setCreated} t={t} />
  );
}

// ------------------------------------------------------------------- the form

function Form({
  device,
  onCreated,
  t,
}: {
  device: DeviceState;
  onCreated: (shade: ShadeDto) => void;
  t: Translate;
}) {
  const [name, setName] = useState('');
  const [kind, setKind] = useState(KIND_OPTIONS[0]?.value ?? 0);
  const [tiltMode, setTiltMode] = useState(TILT_NONE);
  const [upSeconds, setUpSeconds] = useState(DEFAULTS.upTimeMs / 1000);
  const [downSeconds, setDownSeconds] = useState(DEFAULTS.downTimeMs / 1000);
  const [tiltSeconds, setTiltSeconds] = useState(DEFAULTS.tiltTimeMs / 1000);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const used = nameBytes(name);
  const hasTilt = tiltMode !== TILT_NONE;
  const ready = used > 0 && used <= NAME_MAX_BYTES && upSeconds > 0 && downSeconds > 0;

  const submit = (event: Event) => {
    event.preventDefault();
    if (!ready || busy) return;
    setBusy(true);
    setFailure(undefined);
    createShade({
      name,
      kind,
      tiltMode,
      upTimeMs: Math.round(upSeconds * 1000),
      downTimeMs: Math.round(downSeconds * 1000),
      // A shade with no tilt has no tilt travel to time.
      tiltTimeMs: hasTilt ? Math.round(tiltSeconds * 1000) : 0,
    })
      .then((shade) => {
        // Reload before showing the confirmation: the dashboard behind this
        // screen must already know about the shade when the user returns.
        device.reload();
        onCreated(shade);
      })
      .catch((cause: unknown) => setFailure(t(errorMessageKey(cause))))
      .finally(() => setBusy(false));
  };

  return (
    <form class="detail" onSubmit={submit}>
      <nav class="detail__nav">
        <a class="link" href="/">
          ← {t('detail.back')}
        </a>
      </nav>

      <header class="detail__head">
        <h2>{t('add.title')}</h2>
      </header>

      <p class="prose">{t('add.intro')}</p>

      {failure !== undefined && (
        <p class="panel panel--error" role="alert">
          {t('add.failed', { reason: failure })}
        </p>
      )}

      <section class="panel">
        <label class="field">
          <span class="field__label">{t('add.name')}</span>
          <input
            type="text"
            class="field__input"
            value={name}
            autoComplete="off"
            required
            aria-describedby="name-hint"
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </label>
        <p id="name-hint" class={`field__hint${used > NAME_MAX_BYTES ? ' field__hint--bad' : ''}`}>
          {t('add.nameHint', { used, max: NAME_MAX_BYTES })}
        </p>

        <label class="field">
          <span class="field__label">{t('add.kind')}</span>
          <select
            class="field__input"
            value={kind}
            onChange={(event) => setKind(Number(event.currentTarget.value))}
          >
            {KIND_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {t(option.label)}
              </option>
            ))}
          </select>
        </label>

        <label class="field">
          <span class="field__label">{t('add.tiltMode')}</span>
          <select
            class="field__input"
            value={tiltMode}
            aria-describedby="tilt-hint"
            onChange={(event) => setTiltMode(Number(event.currentTarget.value))}
          >
            {TILT_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {t(option.label)}
              </option>
            ))}
          </select>
        </label>
        <p id="tilt-hint" class="field__hint">
          {t('add.tiltHint')}
        </p>
      </section>

      <section class="panel">
        <h3>{t('add.times')}</h3>
        <p class="field__hint">{t('add.timesHint')}</p>
        <Seconds label={t('add.upTime')} value={upSeconds} onChange={setUpSeconds} />
        <Seconds label={t('add.downTime')} value={downSeconds} onChange={setDownSeconds} />
        {hasTilt && (
          <Seconds label={t('add.tiltTime')} value={tiltSeconds} onChange={setTiltSeconds} />
        )}
      </section>

      <div class="actions">
        <button type="submit" class="btn btn--primary" disabled={!ready || busy}>
          {busy ? t('add.submitting') : t('add.submit')}
        </button>
        <a class="link" href="/">
          {t('add.cancel')}
        </a>
      </div>
    </form>
  );
}

function Seconds({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <label class="field field--inline">
      <span class="field__label">{label}</span>
      <input
        type="number"
        class="field__input field__input--short"
        min={0.1}
        max={600}
        step={0.1}
        value={value}
        required
        onInput={(event) => onChange(Number(event.currentTarget.value))}
      />
      <span class="field__suffix">s</span>
    </label>
  );
}

// ------------------------------------------------------------- what happened

function Created({ shade, t }: { shade: ShadeDto; t: Translate }) {
  const { route } = useLocation();
  return (
    <div class="detail">
      <header class="detail__head">
        <h2>{t('add.createdTitle', { name: shade.name })}</h2>
        <p class="mono">{t('add.createdAddress', { address: formatAddress(shade.address) })}</p>
      </header>

      <section class="panel">
        <p class="prose">{t('add.createdBody', { name: shade.name })}</p>
      </section>

      <div class="actions">
        <button
          type="button"
          class="btn btn--primary"
          onClick={() => route(`/shades/${shade.id}/pair`)}
        >
          {t('add.createdPair')}
        </button>
        <a class="link" href={`/shades/${shade.id}`}>
          {t('add.createdLater')}
        </a>
      </div>
    </div>
  );
}
