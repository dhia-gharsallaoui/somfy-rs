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
  'dashboard.unfinished': 'Finish setting up',
  'dashboard.unfinishedWhy':
    'These shades have been added but no motor has been taught to answer them yet, so they will not move and they are not in Home Assistant. Finishing takes a couple of minutes at the shade.',
  'dashboard.unfinishedResume': 'Finish setup',

  'shade.open': 'Open',
  'shade.closed': 'Closed',
  'shade.openPercent': '{percent}% open',
  'shade.opening': 'Opening',
  'shade.closing': 'Closing',
  'shade.idle': 'Stopped',
  'shade.favourite': 'Favourite at {percent}% open',
  'shade.noFavourite': 'No favourite set',
  'shade.openPercentApprox': 'about {percent}% open',
  'shade.uncertainAria':
    'The device has not seen this shade reach a limit since it last moved part way, so this figure may be up to {margin} percentage points out. Opening or closing it fully makes it exact again.',

  'command.up': 'Open',
  'command.my': 'Favourite',
  'command.down': 'Close',
  'command.upAria': 'Open {name}',
  'command.myAria': 'Move {name} to its favourite position, or stop it',
  'command.downAria': 'Close {name}',
  'command.sliderAria': 'Openness of {name}, percent',
  'command.vent': 'Vent',
  'command.ventAria':
    'Close {name} fully, then open the slats just enough to let light through',
  'command.ventUnavailable':
    'The slat-separation time has not been measured, so there is nothing for a vent to aim at. Measure it under Travel times.',

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

  'calib.startLag': 'Start delay',
  'calib.ventBand': 'Slat separation',
  'calib.closeBand': 'Slat compression',
  'calib.bandsHint':
    'Not all of a travel moves the curtain. The first fraction of a second goes on getting the command to the motor and starting it; on a perforated shutter the first few seconds of opening only separate the slats, and the last few seconds of closing only squeeze them shut again. These three are parts of the times above, not extra time on top — measuring one makes part-open positions more accurate without changing how long a full travel takes.',
  'calib.ventBandHint':
    'This is also where Vent stops. Leave it at zero and the Vent control is not offered.',

  'calib.autoTitle': 'Measure automatically',
  'calib.autoHint':
    'The device times the shade while you watch it. Put the shade at the far limit first, start the run, and tap as each thing happens. Nothing is stored until you finish, and cancelling stores nothing.',
  'calib.autoUp': 'Measure opening',
  'calib.autoDown': 'Measure closing',
  'calib.autoUpPrep':
    'Close the shade fully and wait for it to stop. Then start — the shade will open, and you will be asked to tap three times.',
  'calib.autoDownPrep':
    'Open the shade fully and wait for it to stop. Then start — the shade will close, and you will be asked to tap twice.',
  'calib.autoStart': 'Start and open',
  'calib.autoStartDown': 'Start and close',
  'calib.autoRunning': 'Running — {elapsed} s',
  'calib.autoMarkMotion': 'It has started moving',
  'calib.autoMarkCurtainUp': 'The curtain has started to rise',
  'calib.autoMarkCurtainDown': 'The curtain has reached the bottom',
  'calib.autoFinish': 'It has stopped',
  'calib.autoCancel': 'Cancel',
  'calib.autoMarked': 'Noted.',
  'calib.autoDone':
    'Measured. The times above have been updated, and the shade is at a limit, so its position is exact again.',
  'calib.autoOptional': 'Each tap is optional — skip one and that value is left as it was.',

  'detail.linkedRemotes': 'Linked remotes',
  'detail.linkedRemotesPending': 'Not built yet.',
  'detail.origin': 'Where this address came from',
  'detail.originAllocated': 'Allocated by this controller',
  'detail.originImported': 'Imported from another controller',
  'detail.originAllocatedNote':
    'No other controller uses this address. A motor only obeys it once it has been paired.',
  'detail.originImportedNote':
    'This address belongs to the controller it was imported from. If that controller is still running, both are now one remote keeping two separate counters, and whichever falls behind will stop being obeyed. Pairing cannot fix that — it would teach the motor the same shared address again.',
  'detail.pair': 'Pair this shade again',
  'detail.unfinishedTitle': 'Setup is not finished',
  'detail.unfinishedBody':
    '{name} has a remote address of its own, and no motor has been taught it yet — so nothing responds to these controls and there is no entity for it in Home Assistant. Finishing means standing at the shade with a remote that already works.',
  'detail.unfinishedResume': 'Finish setting up {name}',
  'detail.dangerZone': 'Remove',
  'detail.delete': 'Remove {name}',
  'detail.deleteWarning':
    'This removes {name} from this controller only. The motor is not told, and cannot be: it keeps obeying every remote it has learned, including this one. There is deliberately no unpairing command here — on a physical remote unpairing is a held PROG press, and a burst even slightly too long removes a remote from a shade that was working.',
  'detail.deleteConfirm': 'Yes, remove {name}',
  'detail.deleteCancel': 'Keep it',
  'detail.deleting': 'Removing…',

  'add.title': 'Add a shade',
  'add.progress': 'Step 1: what it is',
  'add.intro':
    'The controller will give this shade a remote address of its own. No motor knows that address yet, so the next step is teaching one — and this takes you straight there. The shade will not appear in Home Assistant until it is set up and you have seen it move.',
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

  'pair.title': 'Setting up {name}',
  'pair.progress': 'Step {step} of {total}',
  'pair.additive':
    'Nothing is taken away. Pairing adds this controller to the motor; every remote that works today keeps working.',
  'pair.unfinished':
    '{name} is not finished. Until you have paired it and seen it move, it does not appear in Home Assistant and nothing responds to it.',

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
  'pair.step3Sent': 'Sent. Watch the shade — a short up-and-down jog means the motor took it.',
  'pair.step3NoFeedback':
    'Nothing more will appear here. The controller transmits and never hears back, so it cannot tell you whether the motor accepted the signal. The jog is a good sign and it is easy to miss, so it is not what decides — the next step is the real test.',
  'pair.step3Next': 'Continue — test the shade',
  'pair.step3No': 'Nothing happened at all',
  'pair.step3Failed': 'The device would not send it: {reason}',

  'pair.step4Title': 'Test it',
  'pair.step4Body':
    'Open and close {name} from here, and watch the shade itself. This is the same command Home Assistant will send, so it tests the whole path rather than just whether a signal arrived.',
  'pair.step4Limit':
    'If the shade is already fully open, Open does nothing visible — which looks exactly like a failure. Try the other direction before concluding anything.',
  'pair.step4Why':
    'Nothing on this screen tells you whether it worked, on purpose. The controller cannot hear the motor, so any position it showed here would just be its own guess. You are the only instrument this has.',
  'pair.step4Question': 'Did {name} actually move?',
  'pair.step4OnlyYou':
    'Answer for what you saw at the shade, not for what happened on screen. Saying yes is what adds {name} to Home Assistant.',
  'pair.step4Yes': 'Yes — it moved',
  'pair.step4No': 'No — it did not move',
  'pair.confirming': 'Finishing…',
  'pair.confirmFailed': 'The device could not record that: {reason}',

  'pair.doneTitle': '{name} is set up',
  'pair.doneWitness':
    'You watched it move; the controller did not, and could not. That is why it asked, and it is the only evidence this protocol has.',
  'pair.doneAnnounced':
    '{name} is now in Home Assistant, and its entities will come back on their own after a restart of either the device or the broker.',
  'pair.doneEnd':
    'Programming mode ends by itself after a minute or two. To end it now, press PROG on the existing remote again.',
  'pair.doneBack': 'Go to {name}',

  'pair.retryTitle': 'It did not work',
  'pair.retryIntro': 'In this order, because the cheap checks come first.',
  'pair.retryWindow':
    'The programming window closed. Two minutes is generous but not unlimited. Put the motor back into programming mode and send again — this is much the most common cause.',
  'pair.retryChannel':
    'The remote was on another channel, so a different shade went into programming mode — or none did.',
  'pair.retryCode':
    'The pairing worked and the shade still ignores commands. That is usually the rolling code: a motor refuses any code at or below the last one it accepted. Pairing again fixes it, because pairing teaches the motor whatever is being sent now.',
  'pair.retryRange':
    'The signal did not reach the motor. Move the controller closer to the shade, or check its antenna.',
  'pair.retryAgain': 'Start again from programming mode',
  'pair.retryStop': 'Stop for now',

  'pair.abandon': 'Discard this shade',
  'pair.abandonWarning':
    'This removes {name} from the controller. Nothing else is affected: it was never added to Home Assistant, so there is nothing there to clean up, and no remote that works today stops working. You can add it again whenever you like.',
  'pair.abandonConfirm': 'Yes, discard {name}',
  'pair.abandonCancel': 'Keep it',
  'pair.abandoning': 'Discarding…',

  'pair.alreadyTitle': 'Does it already work?',
  'pair.alreadyBody':
    'This address came from another controller, so a motor may already answer it. Try opening and closing {name}; if it obeys, its setup is finished and you can say so.',
  'pair.alreadyConfirm': 'It moved — finish setup',

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
  'error.invalidDeadBand':
    'the start delay and slat times have to leave some travel behind them — they are parts of a travel time, not extra time on top',
  'error.ventBandNotMeasured':
    'the slat-separation time has never been measured, and it is the only thing a vent aims at',
  'error.notCalibrating': 'that measurement is no longer running',
  'error.calibrationImplausible':
    'the device will not store those numbers — a travel of zero, or longer than three minutes, or taps that leave no travel between them',
  'error.unknown': 'the device did not say why',


  'error.valueEmpty': '{field} must not be empty',
  'error.valueTooLong': '{field} is longer than the device can store',
  'error.valueTooShort': '{field} is too short — a Wi-Fi passphrase needs at least 8 characters',
  'error.valueInteriorNul':
    '{field} contains a NUL character, which MQTT does not allow in a string',
  'error.brokerAddressMalformed': '{field} must be four numbers separated by dots, like 192.168.1.10',
  'error.brokerAddressUnroutable':
    '{field} is an address no connection can reach — not 0.0.0.0, not a loopback, not a multicast or broadcast address',
  'error.brokerPortZero': '{field} must not be zero',
  'error.passwordWithoutUsername': 'a broker password needs a {field} to go with it',
  'error.topicWildcard': '{field} must not contain # or +, which belong in subscriptions',
  'error.topicLeadingSlash': '{field} must not start with /',
  'error.topicTrailingSlash': '{field} must not end with /',
  'error.topicEmptySegment': '{field} must not contain //',
  'error.topicIllegalCharacter': '{field} may only contain letters, digits, _, - and / as a separator',
  'error.namespacesOverlap':
    '{field} must not be the same as the discovery prefix, or sit inside it — the device would publish its availability onto Home Assistant’s own topic',
  'error.secretNotSet': 'there is no stored {field} to keep — type one, or choose to have none',
  'error.noTrialInProgress': 'that network test has already finished',
  'error.trialInProgress': 'a network test is already running — finish or cancel it first',
  'error.trialNotAssociated': 'the device is not on the new network yet, so it cannot be confirmed',
  'error.settingsUnwritable': 'the device could not store the settings — nothing was changed',

  'settings.title': 'Settings',
  'settings.loading': 'Reading the device’s settings…',
  'settings.unreachable': 'Could not reach the device: {detail}',
  'settings.retry': 'Try again',

  'settings.wifiTitle': 'Wi-Fi',
  'settings.wifiIntro':
    'The device joins this network on every boot. Changing it is done as a test you have to confirm from the new network — see below.',
  'settings.wifiNone': 'No network is stored. This device was provisioned over USB.',
  'settings.wifiSsid': 'network name',
  'settings.wifiPsk': 'passphrase',
  'settings.wifiPskStored': 'A passphrase is stored.',
  'settings.wifiPskOpen': 'No passphrase — this is an open network.',
  'settings.secretKeep': 'Keep the stored one',
  'settings.secretSet': 'Type a new one',
  'settings.secretClear': 'There should not be one',
  'settings.wifiWarn':
    'The device will leave this network to try {ssid}. Join {ssid} yourself and open this page again within {minutes} minutes to keep it. If nobody does, the device restarts onto {current} and nothing is stored.',
  'settings.wifiWarnNoCurrent':
    'The device will leave this network to try {ssid}. Join {ssid} yourself and open this page again within {minutes} minutes to keep it. If nobody does, the device restarts and nothing is stored.',
  'settings.wifiSubmit': 'Test this network',
  'settings.wifiSubmitting': 'Starting the test…',

  'settings.trialTitle': 'Testing {ssid}',
  'settings.trialAssociating':
    'The device is joining {ssid}. It has left the network you were on.',
  'settings.trialAwaiting':
    'The device is on {ssid} and has an address. Confirm within {seconds} s to keep it.',
  'settings.trialLeft':
    'The device has left this network. Join {ssid}, open this page again, and confirm — otherwise it restarts onto the stored network on its own.',
  'settings.trialRemaining': '{seconds} s left',
  'settings.trialConfirm': 'I can reach it — keep this network',
  'settings.trialConfirming': 'Storing…',
  'settings.trialCancel': 'Cancel and go back',
  'settings.trialCancelled': 'Going back to the stored network. The device is restarting.',
  'settings.trialSaved': 'Stored. This is the device’s network now.',

  'settings.mqttTitle': 'Home Assistant broker (MQTT)',
  'settings.mqttIntro':
    'Optional. Without a broker the device still receives, decodes and tracks every shade — it just publishes nothing.',
  'settings.mqttNone': 'No broker is configured.',
  'settings.mqttAddress': 'broker address',
  'settings.mqttPort': 'broker port',
  'settings.mqttUsername': 'broker username',
  'settings.mqttUsernameHint': 'Leave empty for an anonymous connection.',
  'settings.mqttPassword': 'broker password',
  'settings.mqttPasswordStored': 'A password is stored.',
  'settings.mqttPasswordNone': 'No password — the connection is anonymous.',
  'settings.mqttDiscoveryPrefix': 'discovery prefix',
  'settings.mqttDiscoveryPrefixHint':
    'Where Home Assistant looks for device configs. Global to your whole Home Assistant — leave it at homeassistant unless you know it was changed.',
  'settings.mqttStateRoot': 'state topic root',
  'settings.mqttStateRootHint':
    'Where this device publishes its own topics. It must not be the discovery prefix, or sit inside it.',
  'settings.mqttWarn':
    'Saving restarts the device. That is what clears the retained Home Assistant entities published under the previous topics, before the new ones go out.',
  'settings.mqttSubmit': 'Save and restart',
  'settings.mqttSubmitting': 'Saving…',
  'settings.mqttClear': 'Run without a broker',
  'settings.mqttClearing': 'Clearing…',
  'settings.mqttConfirmClear': 'Remove the broker and restart?',
  'settings.mqttCleared': 'Stored. The device is restarting without a broker.',
  'settings.mqttSaved': 'Stored. The device is restarting.',
  'settings.restarting':
    'The device is restarting. This page will come back on its own in a few seconds.',
  'settings.failed': 'Refused: {reason}',

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
