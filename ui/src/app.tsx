/**
 * App shell: header, routes, and the one `useDevice()` that every screen reads
 * from.
 *
 * Routing comes from `preact-iso/router` rather than the `preact-iso` barrel —
 * the barrel re-exports `prerender`, which pulls `preact-render-to-string` in,
 * and this app has no server to render on. Importing the subpath keeps the SSR
 * half out of the bundle by construction rather than by trusting tree-shaking.
 */
import { LocationProvider, Route, Router, useLocation, useRoute } from 'preact-iso/router';

import { useI18n, LOCALES, LOCALE_NAMES, type Locale } from './i18n';
import { Dashboard } from './routes/dashboard';
import { ShadeDetail } from './routes/shade-detail';
import { ShadeNew } from './routes/shade-new';
import { ShadePair } from './routes/shade-pair';
import { Stub } from './routes/stub';
import { useDevice, type DeviceState } from './state/device';

export function App() {
  return (
    <LocationProvider>
      <Shell />
    </LocationProvider>
  );
}

function Shell() {
  const device = useDevice();
  const { t } = useI18n();

  return (
    <>
      <header class="topbar">
        <a class="topbar__brand" href="/">
          {t('app.name')}
        </a>
        <nav class="topbar__nav">
          <a class="link" href="/settings">
            {t('nav.settings')}
          </a>
          <a class="link" href="/diagnostics">
            {t('nav.diagnostics')}
          </a>
        </nav>
        <ConnectionPill state={device.connection} />
        <LanguagePicker />
      </header>

      <main class="main">
        <Router>
          <Route path="/" component={Dashboard} device={device} />
          {/*
            `preact-iso`'s Router takes the *first* matching child, so the
            literal segment must precede the parameter or `/shades/new` would
            be read as a shade with the id "new".
          */}
          <Route path="/shades/new" component={ShadeNew} device={device} />
          <Route path="/shades/:id/pair" component={ShadePairRoute} device={device} />
          <Route path="/shades/:id" component={ShadeRoute} device={device} />
          <Route path="/settings" component={Stub} screen="stub.settings" />
          <Route path="/backup" component={Stub} screen="stub.backup" />
          <Route path="/diagnostics" component={Stub} screen="stub.diagnostics" />
          <Route path="/onboarding" component={Stub} screen="stub.onboarding" />
          <Route default component={NotFound} />
        </Router>
      </main>
    </>
  );
}

/** Reads `:id` off the route and hands the detail screen a number, not a string. */
function ShadeRoute({ device }: { device: DeviceState }) {
  const { params } = useRoute();
  return <ShadeDetail device={device} id={Number(params['id'])} />;
}

/** The same, for the pairing assistant. */
function ShadePairRoute({ device }: { device: DeviceState }) {
  const { params } = useRoute();
  return <ShadePair device={device} id={Number(params['id'])} />;
}

function NotFound() {
  const { t } = useI18n();
  const { route } = useLocation();
  return (
    <section class="panel">
      <p>{t('route.notFound')}</p>
      <button type="button" class="btn" onClick={() => route('/')}>
        {t('detail.back')}
      </button>
    </section>
  );
}

function ConnectionPill({ state }: { state: DeviceState['connection'] }) {
  const { t } = useI18n();
  const label =
    state === 'open' ? t('conn.open') : state === 'connecting' ? t('conn.connecting') : t('conn.closed');
  return (
    <span class={`pill pill--${state}`} role="status">
      <span class="pill__dot" aria-hidden="true" />
      {label}
    </span>
  );
}

function LanguagePicker() {
  const { locale, setLocale, t } = useI18n();
  return (
    <label class="lang">
      <span class="visually-hidden">{t('nav.language')}</span>
      <select
        value={locale}
        onChange={(event) => setLocale(event.currentTarget.value as Locale)}
      >
        {LOCALES.map((code) => (
          <option key={code} value={code}>
            {LOCALE_NAMES[code]}
          </option>
        ))}
      </select>
    </label>
  );
}
