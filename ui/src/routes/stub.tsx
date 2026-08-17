/**
 * Placeholder for the screens design spec §8 lists but this plan does not
 * build: the pairing assistant, settings, backup/restore, diagnostics and the
 * captive-portal onboarding mode.
 *
 * They are routes rather than nothing so the shell's navigation, the router
 * and the translations are exercised end to end — and so that the next plan
 * adds a screen instead of also adding routing.
 */
import { useT } from '../i18n';
import type { MessageKey } from '../i18n/en';

export function Stub({ screen }: { screen: MessageKey }) {
  const t = useT();
  return (
    <section class="panel panel--pending">
      <h2>{t('stub.heading', { screen: t(screen) })}</h2>
      <p>{t('stub.body')}</p>
      <a class="link" href="/">
        ← {t('detail.back')}
      </a>
    </section>
  );
}
