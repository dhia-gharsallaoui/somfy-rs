/**
 * English messages — the **source catalogue**.
 *
 * Its keys define {@link MessageKey}, and every other locale is typed as a
 * total `Record<MessageKey, string>`. Adding a string here without translating
 * it is a TypeScript error, not a screen that silently falls back to English.
 *
 * `{name}`-style placeholders are substituted by `t()`; see `./index.tsx`.
 */
export const en = {
  'app.name': 'somfy-rs',

  'nav.settings': 'Settings',
  'nav.diagnostics': 'Diagnostics',
  'nav.language': 'Language',

  'conn.open': 'Live',
  'conn.connecting': 'Connecting…',
  'conn.closed': 'Reconnecting…',

  'dashboard.title': 'Shades',
  'dashboard.loading': 'Loading shades…',
  'dashboard.empty': 'No shades are configured yet.',
  'dashboard.error': 'Could not reach the device: {detail}',
  'dashboard.retry': 'Try again',
  'dashboard.unassigned': 'Not in a room',
  'dashboard.groupCount': '{count} shades',

  'shade.open': 'Open',
  'shade.closed': 'Closed',
  'shade.openPercent': '{percent}% open',
  'shade.opening': 'Opening',
  'shade.closing': 'Closing',
  'shade.idle': 'Stopped',
  'shade.favourite': 'Favourite at {percent}% open',
  'shade.noFavourite': 'No favourite set',

  'command.up': 'Open',
  'command.my': 'Favourite',
  'command.down': 'Close',
  'command.upAria': 'Open {name}',
  'command.myAria': 'Move {name} to its favourite position, or stop it',
  'command.downAria': 'Close {name}',
  'command.sliderAria': 'Openness of {name}, percent',

  'kind.roller': 'Roller',
  'kind.blind': 'Blind',
  'kind.draperyLeft': 'Drapery (left)',
  'kind.awning': 'Awning',
  'kind.shutter': 'Shutter',
  'kind.draperyRight': 'Drapery (right)',
  'kind.draperyCenter': 'Drapery (centre)',
  'kind.unknown': 'Shade',

  'detail.back': 'Back to dashboard',
  'detail.notFound': 'No shade with id {id}.',
  'detail.address': 'Remote address',
  'detail.travelTimes': 'Travel times',
  'detail.upTime': 'Opening',
  'detail.downTime': 'Closing',
  'detail.tiltTime': 'Tilt',
  'detail.seconds': '{seconds} s',
  'detail.tilt': 'Tilt',
  'detail.tiltNone': 'This shade has no tilt.',
  'detail.calibration': 'Travel-time calibration',
  'detail.calibrationPending':
    'Guided calibration measures the opening and closing times separately. Not built yet — see the position-accuracy requirements, R2.',
  'detail.linkedRemotes': 'Linked remotes',
  'detail.linkedRemotesPending': 'Not built yet.',

  'stub.heading': '{screen}',
  'stub.body': 'This screen is not built yet.',
  'stub.pairing': 'Pairing assistant',
  'stub.settings': 'Settings',
  'stub.backup': 'Backup & restore',
  'stub.diagnostics': 'Diagnostics',
  'stub.onboarding': 'Setup',

  'route.notFound': 'That page does not exist.',
} as const;

/** Every message key in the app. Locales must cover all of them. */
export type MessageKey = keyof typeof en;
