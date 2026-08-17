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

/** The endpoints get words; everything between them gets a number. */
function openLabel(open: OpenPercent, t: Translate): string {
  if (open === PERCENT_MAX) return t('shade.open');
  if (open === PERCENT_MIN) return t('shade.closed');
  return t('shade.openPercent', { percent: open });
}

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
        <strong>{openLabel(open, t)}</strong>
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
      </div>

      <label class="tile__slider">
        <span class="visually-hidden">{t('command.sliderAria', { name: shade.name })}</span>
        <input
          type="range"
          min={0}
          max={100}
          step={1}
          value={open}
          aria-valuetext={openLabel(open, t)}
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
