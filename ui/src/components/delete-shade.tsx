/**
 * Removing a shade from the controller.
 *
 * ## Why the warning is long
 *
 * Delete looks like the inverse of add and is not. Adding a shade invents an
 * address and pairing teaches it to a motor; deleting the shade only forgets
 * the address here. **The motor is not told, and cannot be** — there is no
 * "forget this remote" a controller may send safely, because on a physical
 * remote unpairing is a *held* PROG press and the burst length is the only
 * thing separating it from a pairing tap. `somfy_domain::PAIR_REPEATS` pins
 * ours to a tap and the firmware exposes no unpair at all, so the motor keeps
 * obeying whatever it has learned, including this controller.
 *
 * That is not a caveat to bury. Somebody deleting a shade to "unpair it" is
 * about to be surprised, and the surprise is only discoverable at the window.
 *
 * ## Why an inline confirmation rather than a dialog
 *
 * The warning is the point, and a `confirm()` cannot show it. An expanding
 * panel keeps the destructive action two deliberate clicks away with the reason
 * visible in between, and needs no focus-trap of its own.
 */
import { useState } from 'preact/hooks';
import { useLocation } from 'preact-iso/router';

import { deleteShade } from '../api/client';
import { errorMessageKey } from '../api/errors';
import type { ShadeDto } from '../api/generated/ShadeDto';
import { useT } from '../i18n';

export function DeleteShade({ shade, onDeleted }: { shade: ShadeDto; onDeleted: () => void }) {
  const t = useT();
  const { route } = useLocation();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const remove = () => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    deleteShade(shade.id)
      .then(() => {
        onDeleted();
        route('/');
      })
      .catch((cause: unknown) => {
        setFailure(t(errorMessageKey(cause)));
        setBusy(false);
      });
  };

  return (
    <section class="panel panel--danger">
      <h3>{t('detail.dangerZone')}</h3>

      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {failure}
        </p>
      )}

      {confirming ? (
        <>
          <p class="prose">{t('detail.deleteWarning', { name: shade.name })}</p>
          <div class="actions">
            <button type="button" class="btn btn--danger" disabled={busy} onClick={remove}>
              {busy ? t('detail.deleting') : t('detail.deleteConfirm', { name: shade.name })}
            </button>
            <button
              type="button"
              class="btn btn--ghost"
              disabled={busy}
              onClick={() => setConfirming(false)}
            >
              {t('detail.deleteCancel')}
            </button>
          </div>
        </>
      ) : (
        <div class="actions">
          <button type="button" class="btn" onClick={() => setConfirming(true)}>
            {t('detail.delete', { name: shade.name })}
          </button>
        </div>
      )}
    </section>
  );
}
