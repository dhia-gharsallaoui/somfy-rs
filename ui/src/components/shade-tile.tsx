/**
 * One shade: what it is, where it is, and the four ways to move it —
 * open / favourite / close, plus a position slider.
 *
 * The slider is in **openness** (100 = open), the convention every consumer
 * blind app uses; the wire underneath is raw Somfy (0 = open). The two
 * conversions live in `src/api/position.ts` and nowhere else.
 */
import { useEffect, useState } from 'preact/hooks';

import { commandShade } from '../api/client';
import type { ShadeDto } from '../api/generated/ShadeDto';
import {
  motionOf,
  openPercent,
  PERCENT_MAX,
  PERCENT_MIN,
  wireFromOpenPercent,
  type OpenPercent,
} from '../api/position';
import { useT, type Translate } from '../i18n';
import { kindKey } from './kind';

/**
 * The endpoints get words; everything between them gets a number.
 *
 * A number the device is not sure of gets "about", and that is not decoration.
 * RTS is one-way, so an intermediate position is dead reckoning from the last
 * time the shade reached a limit — and on a shade whose travel times nobody has
 * measured it is dead reckoning from a number nobody chose. A flat "60%" there
 * is a claim the device cannot support; "about 60%" is what it actually knows.
 *
 * The endpoints stay exact whatever the uncertainty says, because they are the
 * one thing this protocol is sure of: the motor stops itself at its own end
 * stops, and reaching one resets the doubt to zero.
 */
function openLabel(open: OpenPercent, t: Translate, uncertain = false): string {
  if (open === PERCENT_MAX) return t('shade.open');
  if (open === PERCENT_MIN) return t('shade.closed');
  return uncertain
    ? t('shade.openPercentApprox', { percent: open })
    : t('shade.openPercent', { percent: open });
}

/**
 * How much doubt, in percentage points, before the reading is hedged.
 *
 * A presentation figure rather than a threshold the device acts on — it has its
 * own, and it is about whether to spend a whole extra traverse re-anchoring.
 * Two points is roughly where a rounded whole-percent reading stops being the
 * number it claims to be, and below it hedging would be noise on every tile.
 */
const HEDGE_AT_PERCENT = 2;

const MOTION_KEY = {
  opening: 'shade.opening',
  closing: 'shade.closing',
  idle: 'shade.idle',
} as const;

export interface ShadeTileProps {
  shade: ShadeDto;
  /** Drop the tile's own heading when the screen already carries one. */
  detail?: boolean;
}

export function ShadeTile({ shade, detail = false }: ShadeTileProps) {
  const t = useT();
  const [dragged, setDragged] = useState<number | undefined>(undefined);

  // A drag is only ever an optimistic overlay. As soon as the device reports a
  // new position the device wins — the estimate on the board is the truth, not
  // whatever the finger last touched.
  useEffect(() => setDragged(undefined), [shade.position]);

  const open = dragged ?? openPercent(shade.position);
  const motion = motionOf(shade.direction);
  // A dragged value is what the finger is asking for, not what the device
  // believes, so it is never hedged.
  const uncertain = dragged === undefined && shade.positionUncertainty >= HEDGE_AT_PERCENT;

  // The fill is the *closed* fraction, which is the wire value by definition —
  // no conversion, and deliberately not routed through `position.ts`.
  const fill = shade.position;

  return (
    <article class={`tile tile--${motion}`}>
      <div class="tile__visual" aria-hidden="true">
        <div class="tile__fill" style={{ blockSize: `${fill}%` }} />
      </div>

      {/* On the detail screen the name and kind are already the page heading. */}
      {!detail && (
        <header class="tile__head">
          <h3 class="tile__name">
            <a class="tile__link" href={`/shades/${shade.id}`}>
              {shade.name}
            </a>
          </h3>
          <p class="tile__kind">{t(kindKey(shade.kind))}</p>
        </header>
      )}

      <p class="tile__state">
        <strong
          title={
            uncertain
              ? t('shade.uncertainAria', { margin: shade.positionUncertainty })
              : undefined
          }
        >
          {openLabel(open, t, uncertain)}
        </strong>
        {motion !== 'idle' && <span class="tile__motion">{t(MOTION_KEY[motion])}</span>}
      </p>

      <div class="tile__buttons" role="group">
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
          aria-label={t('command.myAria', { name: shade.name })}
          onClick={() => void commandShade(shade.id, { action: 'my' })}
        >
          <span class="btn__glyph" aria-hidden="true">
            ■
          </span>
          {t('command.my')}
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
        {/*
          Offered only where it can do something. The vent position *is* the
          shade's measured slat-separation time — the command drives to the
          closed limit and runs up for exactly that long, which is why it needs
          no position estimate and why it has nothing to aim at until somebody
          measures it. A button that always refused would be worse than none.
        */}
        {shade.ventBandMs > 0 && (
          <button
            type="button"
            class="btn"
            aria-label={t('command.ventAria', { name: shade.name })}
            onClick={() => void commandShade(shade.id, { action: 'vent' })}
          >
            <span class="btn__glyph" aria-hidden="true">
              ≡
            </span>
            {t('command.vent')}
          </button>
        )}
      </div>

      <label class="tile__slider">
        <span class="visually-hidden">{t('command.sliderAria', { name: shade.name })}</span>
        <input
          type="range"
          min={0}
          max={100}
          step={1}
          value={open}
          aria-valuetext={openLabel(open, t, uncertain)}
          onInput={(event) => setDragged(Number(event.currentTarget.value))}
          onChange={(event) => {
            const next = Number(event.currentTarget.value);
            void commandShade(shade.id, {
              action: 'goTo',
              position: wireFromOpenPercent(next),
            });
          }}
        />
      </label>
    </article>
  );
}
