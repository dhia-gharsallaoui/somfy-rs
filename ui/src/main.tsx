import { render } from 'preact';

import { App } from './app';
import { detectLocale, I18nProvider } from './i18n';
import './styles.css';

// Set before the first paint so assistive technology and the browser's own
// hyphenation see the right language immediately.
document.documentElement.lang = detectLocale();

const root = document.getElementById('app');
if (!root) throw new Error('#app is missing from index.html');

render(
  <I18nProvider>
    <App />
  </I18nProvider>,
  root,
);
