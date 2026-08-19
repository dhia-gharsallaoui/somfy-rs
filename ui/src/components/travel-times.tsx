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
 * ## The two things that used to be missing here
 *
 * **Automatic measurement (R2)** is the second panel now. It is guided rather
 * than automatic in any deeper sense: nothing on the device can see the shade,
 * so a measurement is the device's clock and the operator's eye. Three numbers
 * come out of one Up traverse, which is what keeps the dead time and the dead
 * band from costing extra shade travel.
 *
 * **The dead bands (R5, R8)** are three more rows. The spec settled the
 * mechanism by elimination on 2026-08-17 — these motors complete full traverses
 * from three-frame bursts, which a motor reading burst length as a slat command
 * could not do — so it is mechanical, and the estimator subtracts it. They are
 * presented as *parts of* the travel times, because that is what they are:
 * measuring one makes part-open positions more accurate without changing how
 * long a full travel takes.
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

/**
 * One editable duration.
 *
 * `source` is present only on the three travel times: a dead band has no
 * provenance of its own, because it is measured by the same run — and stored
 * beside — the travel time it is a part of. Marking it separately would invite
 * the reading that it is an independent setting, which is the misunderstanding
 * this panel's copy exists to prevent.
 */
interface Row {
  label: MessageKey;
  field: 'upTimeMs' | 'downTimeMs' | 'tiltTimeMs' | 'startLagMs' | 'ventBandMs' | 'closeBandMs';
  source?: CalibrationSource;
  /** Smallest step the device stores this at, in seconds. */
  step: number;
  hint?: MessageKey;
}

/**
 * `somfy_domain::START_LAG_RESOLUTION_MS` and `DEAD_BAND_RESOLUTION_MS`, in
 * seconds.
 *
 * The device rounds onto them as a value arrives, so the input steps by the
 * same amount — a field that accepts 4.25 s and reads back 4.3 s looks like a
 * bug and is not one.
 */
const LAG_STEP_S = 0.01;
const BAND_STEP_S = 0.1;

export function TravelTimes({ shade, onSaved }: { shade: ShadeDto; onSaved: () => void }) {
  const t = useT();
  const [draft, setDraft] = useState<Partial<Record<Row['field'], number>>>({});
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);
  const [saved, setSaved] = useState(false);

  const rows: Row[] = [
    { label: 'detail.upTime', field: 'upTimeMs', source: shade.upTimeSource, step: 0.1 },
    { label: 'detail.downTime', field: 'downTimeMs', source: shade.downTimeSource, step: 0.1 },
    ...(shade.tiltMode !== TILT_NONE
      ? ([
          {
            label: 'detail.tiltTime',
            field: 'tiltTimeMs',
            source: shade.tiltTimeSource,
            step: 0.1,
          },
        ] as Row[])
      : []),
    // The hint is the one asymmetry a guided run cannot fix and must not hide:
    // the two curtain taps are a *difference*, so the operator's reaction time
    // cancels out of the bands, while the start delay is a single tap and
    // carries it whole. A measured lag is therefore worth less than a measured
    // band, and this is the field where somebody is most likely to correct one
    // by hand.
    { label: 'calib.startLag', field: 'startLagMs', step: LAG_STEP_S, hint: 'calib.startLagHint' },
    { label: 'calib.ventBand', field: 'ventBandMs', step: BAND_STEP_S, hint: 'calib.ventBandHint' },
    { label: 'calib.closeBand', field: 'closeBandMs', step: BAND_STEP_S },
  ];

  const uncalibrated = rows.filter((row) => row.source === 'factoryDefault');
  const seconds = (row: Row): number => draft[row.field] ?? shade[row.field] / 1000;
  const changed = rows.filter((row) => Math.round(seconds(row) * 1000) !== shade[row.field]);
  const at = (field: Row['field']): number =>
    Math.round((draft[field] ?? shade[field] / 1000) * 1000);
  // Three rules, all of them the device's, restated so the button is disabled
  // rather than the request refused:
  //
  // - a lift time of zero is not a slow shade, it is an estimator with no scale;
  // - nothing here may be negative;
  // - the lag and a band are *parts of* their direction's travel time, so
  //   together they have to leave some travel behind them — a 30 s Up that is
  //   30 s of slat separation has no phase in which the curtain rises.
  const valid =
    rows.every((row) => Number.isFinite(seconds(row)) && seconds(row) >= 0) &&
    at('upTimeMs') > 0 &&
    at('downTimeMs') > 0 &&
    at('startLagMs') + at('ventBandMs') < at('upTimeMs') &&
    at('startLagMs') + at('closeBandMs') < at('downTimeMs');

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

      <p class="field__hint">{t('calib.bandsHint')}</p>

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
  const source = row.source === undefined ? undefined : SOURCE_TEXT[row.source];
  return (
    <label class="field field--inline">
      <span class="field__label">
        {t(row.label)}
        {source && <span class={`tag tag--${source.tone}`}>{t(source.label)}</span>}
      </span>
      <input
        type="number"
        class="field__input field__input--short"
        min={0}
        max={600}
        step={row.step}
        value={seconds}
        onInput={(event) => onChange(Number(event.currentTarget.value))}
      />
      <span class="field__suffix">s</span>
      {row.hint && <span class="field__hint">{t(row.hint)}</span>}
    </label>
  );
}
