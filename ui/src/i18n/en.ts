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
  'dashboard.add': 'Add a shade',

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

  'tilt.none': 'No tilt',
  'tilt.motor': 'Separate tilt motor',
  'tilt.integrated': 'Built-in tilt',
  'tilt.tiltOnly': 'Tilt only',
  'tilt.euro': 'European mode',

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
  'calib.factoryDefault': 'not calibrated',
  'calib.operatorSupplied': 'entered by hand',
  'calib.measured': 'measured by the device',
  'calib.uncalibratedWarning':
    'One or more of these is still the factory default. Nobody has measured this shade, so a command to a part-open position will be wrong — often badly. Identical values across several shades are evidence that nobody chose them, not that they are correct.',
  'calib.hint':
    'Time a full travel with a stopwatch, in each direction separately: closing is usually faster, because gravity helps. Ten seconds apart on a thirty-second shade is a third of the travel.',
  'calib.save': 'Save travel times',
  'calib.saving': 'Saving…',
  'calib.revert': 'Undo changes',
  'calib.saved': 'Saved.',
  'calib.failed': 'The device refused these values: {reason}',
  'calib.autoPending':
    'Automatic measurement — where the device sweeps the shade and times it — is not built yet. Until it is, hand-measured values are the accurate ones.',

  'detail.linkedRemotes': 'Linked remotes',
  'detail.linkedRemotesPending': 'Not built yet.',
  'detail.origin': 'Where this address came from',
  'detail.originAllocated': 'Allocated by this controller',
  'detail.originImported': 'Imported from another controller',
  'detail.originAllocatedNote':
    'No other controller uses this address. A motor only obeys it once it has been paired.',
  'detail.originImportedNote':
    'This address belongs to the controller it was imported from. If that controller is still running, both are now one remote keeping two separate counters, and whichever falls behind will stop being obeyed. Pairing cannot fix that — it would teach the motor the same shared address again.',
  'detail.pair': 'Pair this shade',
  'detail.dangerZone': 'Remove',
  'detail.delete': 'Remove {name}',
  'detail.deleteWarning':
    'This removes {name} from this controller only. The motor is not told, and cannot be: it keeps obeying every remote it has learned, including this one. There is deliberately no unpairing command here — on a physical remote unpairing is a held PROG press, and a burst even slightly too long removes a remote from a shade that was working.',
  'detail.deleteConfirm': 'Yes, remove {name}',
  'detail.deleteCancel': 'Keep it',
  'detail.deleting': 'Removing…',

  'add.title': 'Add a shade',
  'add.intro':
    'The controller will give this shade a remote address of its own. No motor knows that address yet, so the next step is teaching one — the shade will not move until you do.',
  'add.name': 'Name',
  'add.nameHint': '{used} of {max} bytes used. Accented letters take two.',
  'add.kind': 'Type',
  'add.tiltMode': 'Tilt',
  'add.tiltHint':
    'Stored for now, and not yet acted on: this firmware drives the lift axis only.',
  'add.times': 'Travel times',
  'add.timesHint':
    'How long the shade takes to travel end to end, in seconds. The position estimate is computed from these, so a rough measurement is much better than a guess. They can be corrected later.',
  'add.upTime': 'Opening',
  'add.downTime': 'Closing',
  'add.tiltTime': 'Tilt',
  'add.submit': 'Add shade',
  'add.submitting': 'Adding…',
  'add.cancel': 'Cancel',
  'add.failed': 'The device refused this shade: {reason}',

  'add.createdTitle': '{name} added',
  'add.createdAddress': 'Remote address {address}',
  'add.createdBody':
    'That address is new and no motor has heard it yet, so {name} will not move. Teaching one motor to answer to it is the next step, and it needs you at the shade with a remote that already works.',
  'add.createdPair': 'Pair it now',
  'add.createdLater': 'Pair it later',

  'pair.title': 'Pairing {name}',
  'pair.progress': 'Step {step} of {total}',
  'pair.additive':
    'Nothing is taken away. Pairing adds this controller to the motor; every remote that works today keeps working.',

  'pair.step1Title': 'Before you start',
  'pair.step1Remote':
    'You need a remote that already drives this shade — a wall remote, or another controller. This controller cannot stand in for it: a motor that has never heard of it ignores everything it sends, including the pairing signal.',
  'pair.step1See':
    'You need to be able to see the shade. The only acknowledgement this procedure has is the shade moving, and nobody else is watching.',
  'pair.step1Still':
    'Ideally the shade is stopped. Pairing one that is mid-travel leaves its position estimate wrong until the next full open or close, which corrects it.',
  'pair.step1Next': 'I have a working remote — continue',

  'pair.step2Title': 'Put the motor into programming mode',
  'pair.step2Hold':
    'At the shade, press and hold the PROG button on the existing remote for about two seconds, until the shade jogs — a short up-and-down movement.',
  'pair.step2Recessed':
    'On most remotes PROG is a small recessed button on the back, and needs a pen or a paperclip.',
  'pair.step2Channel':
    'On a multi-channel remote, select this shade’s channel before pressing PROG.',
  'pair.step2Window': 'The motor then stays in programming mode for about two minutes.',
  'pair.step2Next': 'The shade jogged — continue',
  'pair.step2Back': 'Back',

  'pair.step3Title': 'Send the pairing signal',
  'pair.step3Body':
    'The controller will transmit the pairing signal to {name}. Send it while the motor is still in programming mode.',
  'pair.step3Send': 'Send the pairing signal',
  'pair.step3Sending': 'Sending…',
  'pair.step3Remaining': 'About {time} of the programming window left.',
  'pair.step3Expired':
    'The two minutes are probably up. Put the motor back into programming mode before sending again.',
  'pair.step3Sent': 'Sent. Watch the shade now.',
  'pair.step3NoFeedback':
    'Nothing more will appear here. The controller transmits and never hears back, so it cannot tell you whether the motor accepted the signal — only you can see that.',
  'pair.step3Question': 'Did the shade jog?',
  'pair.step3Yes': 'Yes, it jogged',
  'pair.step3No': 'No, nothing happened',
  'pair.step3Failed': 'The device would not send it: {reason}',

  'pair.doneTitle': '{name} is paired',
  'pair.doneWitness':
    'You saw the acknowledgement; the controller did not, and could not. That second jog was the motor confirming it has learned this controller.',
  'pair.doneEnd':
    'Programming mode ends by itself after a minute or two. To end it now, press PROG on the existing remote again.',
  'pair.doneTest': 'Now test it: open and close {name} and check that it obeys.',
  'pair.doneBack': 'Go to {name}',

  'pair.retryTitle': 'Nothing happened',
  'pair.retryIntro': 'In this order, because the cheap checks come first.',
  'pair.retryWindow':
    'The programming window closed. Two minutes is generous but not unlimited. Put the motor back into programming mode and send again — this is much the most common cause.',
  'pair.retryChannel':
    'The remote was on another channel, so a different shade went into programming mode — or none did.',
  'pair.retryRange':
    'The signal did not reach the motor. Move the controller closer to the shade, or check its antenna.',
  'pair.retryAgain': 'Start again from programming mode',
  'pair.retryStop': 'Stop for now',

  'pair.blockedTitle': 'Pairing is not available for this shade',
  'pair.blockedBody':
    '{name}’s remote address was imported from another controller, so the motor already knows it — and so does that controller. Pairing would teach the motor the same shared address again, which is the problem rather than the fix.',
  'pair.blockedAdvice':
    'To bring {name} under this controller, add it again as a new shade — the device will allocate an address of its own — pair that one, then remove this entry once it works.',
  'pair.blockedBack': 'Back to {name}',

  'error.nameEmpty': 'the name is empty',
  'error.nameTooLong': 'the name is longer than 32 bytes',
  'error.invalidKind': 'this firmware does not model that type of shade',
  'error.invalidTiltMode': 'this firmware does not model that tilt mode',
  'error.travelTimeZero': 'a travel time of zero leaves the position estimate with no scale',
  'error.invalidAddress': 'the address the device allocated is not a usable one',
  'error.registryFull': 'this controller is full — 32 shades is the limit',
  'error.notFound': 'that shade no longer exists',
  'error.addressNotAllocated': 'this shade’s address belongs to another controller',
  'error.unknown': 'the device did not say why',

  'stub.heading': '{screen}',
  'stub.body': 'This screen is not built yet.',
  'stub.settings': 'Settings',
  'stub.backup': 'Backup & restore',
  'stub.diagnostics': 'Diagnostics',
  'stub.onboarding': 'Setup',

  'route.notFound': 'That page does not exist.',
} as const;

/** Every message key in the app. Locales must cover all of them. */
export type MessageKey = keyof typeof en;
