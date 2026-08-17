/**
 * Shade detail. Design spec §8 puts three things on this screen: tilt,
 * travel-time calibration, and linked remotes.
 *
 * Only the first is renderable today — tilt is config-carriage in this
 * generation (`somfy-api` deliberately exposes no tilt *command*), and the
 * device exposes no linked-remote resource yet. **Calibration is real work, not
 * a checkbox**: the position-accuracy requirements' R2 asks for a guided
 * measurement of `upTimeMs` and `downTimeMs` *independently*, which means a
 * multi-step flow with its own timer, its own instructions, and a save. This
 * route is laid out as a stack of sections precisely so that flow has somewhere
 * to land without the page being redesigned around it.
 *
 * What *is* live here is the same control surface as the dashboard tile, so a
 * shade can be driven while its settings are read.
 */
import type { AddressOrigin } from '../api/generated/AddressOrigin';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { openPercent } from '../api/position';
import { DeleteShade } from '../components/delete-shade';
import { formatAddress, seconds } from '../components/format';
import { kindKey, TILT_NONE } from '../components/kind';
import { ShadeTile } from '../components/shade-tile';
import { useT, type Translate } from '../i18n';
import type { MessageKey } from '../i18n/en';
import type { DeviceState } from '../state/device';

/**
 * Total over the generated {@link AddressOrigin}, so a third origin added in
 * Rust fails `tsc` here rather than rendering a blank line. The `note` is the
 * consequence the user actually needs — for an imported address it is the
 * reason pairing is not offered, which would otherwise look like a missing
 * button.
 */
const ORIGIN_TEXT: Record<AddressOrigin, { label: MessageKey; note: MessageKey }> = {
  allocated: { label: 'detail.originAllocated', note: 'detail.originAllocatedNote' },
  imported: { label: 'detail.originImported', note: 'detail.originImportedNote' },
};

export function ShadeDetail({ device, id }: { device: DeviceState; id: number }) {
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

  return (
    <div class="detail">
      <nav class="detail__nav">
        <a class="link" href="/">
          ← {t('detail.back')}
        </a>
      </nav>

      <header class="detail__head">
        <h2>{shade.name}</h2>
        <p class="detail__kind">{t(kindKey(shade.kind))}</p>
      </header>

      <ShadeTile shade={shade} detail />

      <section class="panel">
        <h3>{t('detail.address')}</h3>
        <p class="mono">{formatAddress(shade.address)}</p>
        <p>{favouriteLabel(shade, t)}</p>
      </section>

      {/*
        Origin, and the pairing entry point it gates. Pairing is offered here
        rather than standing on the dashboard because it is a step inside adding
        a shade, not a control for one that already works — and it is offered
        *only* for an address this controller allocated, because pairing a motor
        to another controller's address accomplishes nothing.
      */}
      <section class="panel">
        <h3>{t('detail.origin')}</h3>
        <p>
          <strong>{t(ORIGIN_TEXT[shade.addressOrigin].label)}</strong>
        </p>
        <p class="prose">{t(ORIGIN_TEXT[shade.addressOrigin].note)}</p>
        {shade.addressOrigin === 'allocated' && (
          <a class="btn" href={`/shades/${shade.id}/pair`}>
            {t('detail.pair')}
          </a>
        )}
      </section>

      <section class="panel">
        <h3>{t('detail.travelTimes')}</h3>
        <dl class="facts">
          <dt>{t('detail.upTime')}</dt>
          <dd>{t('detail.seconds', { seconds: seconds(shade.upTimeMs) })}</dd>
          <dt>{t('detail.downTime')}</dt>
          <dd>{t('detail.seconds', { seconds: seconds(shade.downTimeMs) })}</dd>
          {shade.tiltMode !== TILT_NONE && (
            <>
              <dt>{t('detail.tiltTime')}</dt>
              <dd>{t('detail.seconds', { seconds: seconds(shade.tiltTimeMs) })}</dd>
            </>
          )}
        </dl>
      </section>

      {/* R2 lands here. */}
      <section class="panel panel--pending">
        <h3>{t('detail.calibration')}</h3>
        <p>{t('detail.calibrationPending')}</p>
      </section>

      <section class="panel">
        <h3>{t('detail.tilt')}</h3>
        {shade.tiltMode === TILT_NONE ? (
          <p>{t('detail.tiltNone')}</p>
        ) : (
          <p>{t('shade.openPercent', { percent: openPercent(shade.tiltPosition) })}</p>
        )}
      </section>

      <section class="panel panel--pending">
        <h3>{t('detail.linkedRemotes')}</h3>
        <p>{t('detail.linkedRemotesPending')}</p>
      </section>

      <DeleteShade shade={shade} onDeleted={device.reload} />
    </div>
  );
}

function favouriteLabel(shade: ShadeDto, t: Translate): string {
  return shade.myPosition === null
    ? t('shade.noFavourite')
    : t('shade.favourite', { percent: openPercent(shade.myPosition) });
}
