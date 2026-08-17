/**
 * Travel times: the three numbers the position estimate is computed from, what
 * each one's provenance is, and a way to correct them by hand.
 *
 * ## Why this screen matters more than it looks
 *
 * On 2026-08-17 a command for 25% open moved a shade about 1%. All three shades
 * carried 10000/10000/7000 — the reference firmware's compiled-in defaults,
 * imported faithfully because nobody had ever calibrated them, and shown by the
 * UI as though somebody had chosen them. A stopwatch gave 30 s up and 27 s
 * down. Two requirements came out of that, and both live here:
 *
 * - **R7 (raised to MUST):** a factory-default value MUST be surfaced as
 *   *uncalibrated*, not presented as configured. So an uncalibrated value is
 *   marked at the field, and the panel carries a warning while any remains —
 *   "three identical values across three different shades are evidence nobody
 *   chose them".
 * - **R9:** hand-entered values MUST be accepted on an existing shade, not only
 *   at creation and not only through an automatic sweep. A sweep runs the shade
 *   end to end twice per direction, which is not acceptable over a desk, in a
 *   sleeping room, or on an awning in wind — and a sweep with nothing to check
 *   itself against cannot be caught being wrong.
 *
 * ## What is deliberately not here
 *
 * **A "measure automatically" button.** The guided calibration of R2 does not
 * exist. A control promising a measurement nothing performs is worse than no
 * control, so the panel says the measurement is not built rather than offering
 * it. `CalibrationSource` already carries `measured` for when it is.
 *
 * **A dead-band field.** R8 records that the first ~4 s of Up travel off the
 * closed limit separates the slats without lifting — but the spec says the
 * mechanism is unresolved, and its two candidates need the same number handled
 * in opposite ways. `PatchShadeDto` in `crates/somfy-api` carries the argument.
 */
import { useState } from 'preact/hooks';

import { patchShade } from '../api/client';
import { errorMessageKey } from '../api/errors';
import type { CalibrationSource } from '../api/generated/CalibrationSource';
import type { PatchShadeDto } from '../api/generated/PatchShadeDto';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { useT, type Translate } from '../i18n';
import type { MessageKey } from '../i18n/en';
import { TILT_NONE } from './kind';

/**
 * Total over the generated {@link CalibrationSource}: a state added in Rust
 * fails `tsc` here rather than rendering an unlabelled value. `tone` decides
 * whether it reads as a warning, which is the visible half of R7 — an
 * uncalibrated value has to *look* uncalibrated.
 */
const SOURCE_TEXT: Record<CalibrationSource, { label: MessageKey; tone: 'warn' | 'ok' }> = {
  factoryDefault: { label: 'calib.factoryDefault', tone: 'warn' },
  operatorSupplied: { label: 'calib.operatorSupplied', tone: 'ok' },
  measured: { label: 'calib.measured', tone: 'ok' },
};

/** One editable travel time, paired with where its current value came from. */
interface Row {
  label: MessageKey;
  field: 'upTimeMs' | 'downTimeMs' | 'tiltTimeMs';
  source: CalibrationSource;
}

export function TravelTimes({ shade, onSaved }: { shade: ShadeDto; onSaved: () => void }) {
  const t = useT();
  const [draft, setDraft] = useState<Partial<Record<Row['field'], number>>>({});
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);
  const [saved, setSaved] = useState(false);

  const rows: Row[] = [
    { label: 'detail.upTime', field: 'upTimeMs', source: shade.upTimeSource },
    { label: 'detail.downTime', field: 'downTimeMs', source: shade.downTimeSource },
    ...(shade.tiltMode !== TILT_NONE
      ? ([{ label: 'detail.tiltTime', field: 'tiltTimeMs', source: shade.tiltTimeSource }] as Row[])
      : []),
  ];

  const uncalibrated = rows.filter((row) => row.source === 'factoryDefault');
  const seconds = (row: Row): number => draft[row.field] ?? shade[row.field] / 1000;
  const changed = rows.filter((row) => Math.round(seconds(row) * 1000) !== shade[row.field]);
  // The lift times are the estimate's divisor; zero is not a slow shade, it is
  // an estimator with no scale. Refused here as well as by the device.
  const valid = rows.every(
    (row) => row.field === 'tiltTimeMs' || (seconds(row) > 0 && Number.isFinite(seconds(row))),
  );

  const save = (event: Event) => {
    event.preventDefault();
    if (busy || changed.length === 0 || !valid) return;
    setBusy(true);
    setFailure(undefined);
    setSaved(false);

    const body: PatchShadeDto = {};
    for (const row of changed) body[row.field] = Math.round(seconds(row) * 1000);

    patchShade(shade.id, body)
      .then(() => {
        setDraft({});
        setSaved(true);
        onSaved();
      })
      .catch((cause: unknown) => setFailure(t(errorMessageKey(cause))))
      .finally(() => setBusy(false));
  };

  return (
    <form class="panel" onSubmit={save}>
      <h3>{t('detail.travelTimes')}</h3>

      {uncalibrated.length > 0 && (
        <p class="note note--warn">{t('calib.uncalibratedWarning')}</p>
      )}

      <p class="field__hint">{t('calib.hint')}</p>

      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {t('calib.failed', { reason: failure })}
        </p>
      )}
      {saved && (
        <p class="note" role="status">
          {t('calib.saved')}
        </p>
      )}

      {rows.map((row) => (
        <TravelRow
          key={row.field}
          row={row}
          seconds={seconds(row)}
          t={t}
          onChange={(value) => setDraft((current) => ({ ...current, [row.field]: value }))}
        />
      ))}

      <div class="actions">
        <button
          type="submit"
          class="btn btn--primary"
          disabled={busy || changed.length === 0 || !valid}
        >
          {busy ? t('calib.saving') : t('calib.save')}
        </button>
        {changed.length > 0 && (
          <button type="button" class="btn btn--ghost" disabled={busy} onClick={() => setDraft({})}>
            {t('calib.revert')}
          </button>
        )}
      </div>

      {/* R2 lands here. Named as missing rather than offered as a button. */}
      <p class="field__hint">{t('calib.autoPending')}</p>
    </form>
  );
}

function TravelRow({
  row,
  seconds,
  t,
  onChange,
}: {
  row: Row;
  seconds: number;
  t: Translate;
  onChange: (value: number) => void;
}) {
  const { label, tone } = SOURCE_TEXT[row.source];
  return (
    <label class="field field--inline">
      <span class="field__label">
        {t(row.label)}
        <span class={`tag tag--${tone}`}>{t(label)}</span>
      </span>
      <input
        type="number"
        class="field__input field__input--short"
        min={row.field === 'tiltTimeMs' ? 0 : 0.1}
        max={600}
        step={0.1}
        value={seconds}
        onInput={(event) => onChange(Number(event.currentTarget.value))}
      />
      <span class="field__suffix">s</span>
    </label>
  );
}
