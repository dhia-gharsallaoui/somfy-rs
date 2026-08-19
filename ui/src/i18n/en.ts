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
  'nav.backup': 'Backup',
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
  'calib.startLagHint':
    'The one figure a guided run measures less well than the other two: it comes from a single tap, so it carries your reaction time whole, where the slat figures are the difference of two taps and mostly cancel it. If a measured value here looks like a quarter of a second of you rather than of the motor, correct it here.',

  'calib.autoTitle': 'Measure with the device timing it',
  'calib.autoHint':
    'You stand where you can see the shade. The device sends the command and holds the stopwatch; you tap as each thing happens, and your taps are the measurement. Nothing is stored until you tap It has stopped, and cancelling stores nothing.',
  'calib.autoOneWay':
    'The device cannot see the shade. Somfy remotes only transmit — no motor ever reports back — so it knows when it sent a command and nothing else. That is why there is no progress bar here and why every tap matters.',
  'calib.autoCost':
    'This moves the shade through its whole range. Each direction takes two full travels: one to get it to the far limit, one to be timed. Doing both in one visit costs three rather than four, because each run ends exactly where the other has to start. If a full travel is not acceptable right now — a shade over a desk, a sleeping room, an awning in wind — enter the times by hand above instead.',
  'calib.autoUp': 'Measure opening',
  'calib.autoDown': 'Measure closing',
  'calib.autoUpPrep':
    'Close the shade fully and wait for it to stop. Then start: the shade will open, and you will be asked to tap three times.',
  'calib.autoDownPrep':
    'Open the shade fully and wait for it to stop. Then start: the shade will close, and you will be asked to tap twice.',
  'calib.autoUpWrites': 'Replaces Opening, Start delay and Slat separation.',
  'calib.autoDownWrites': 'Replaces Closing, Start delay and Slat compression.',
  'calib.autoRunning': 'Running — {elapsed} s',
  'calib.autoWatch':
    'Watch the shade, not this screen. Tap the moment it first stirs, tap again when the curtain itself starts to move, and tap It has stopped when it reaches the limit and the motor goes quiet.',
  'calib.autoMarkMotion': 'It has started moving',
  'calib.autoMarkCurtainUp': 'The curtain has started to rise',
  'calib.autoMarkCurtainDown': 'The curtain has reached the bottom',
  'calib.autoFinish': 'It has stopped',
  'calib.autoCancel': 'Cancel',
  'calib.autoCancelNote':
    'Cancel stores nothing, and it does not stop the shade — it is a measurement being abandoned, not a movement. Use Close or Open above once the run has been cancelled.',
  'calib.autoDoNotTouch':
    'While this is running, do not command this shade from anywhere else — the controls above, Home Assistant, or a wall remote in the house. Any of those ends the measurement, and you will only be told at your next tap.',
  'calib.autoMarked': 'Noted.',
  'calib.autoDoneUp':
    'The opening run took {seconds} s. The times above have been updated, and the shade is at a limit, so its position is exact again.',
  'calib.autoDoneDown':
    'The closing run took {seconds} s. The times above have been updated, and the shade is at a limit, so its position is exact again.',
  'calib.autoCheck':
    'Check that against a stopwatch, or against how long the shade felt like it took. A figure that is wildly out is a run something interrupted, and running it again costs nothing already stored.',
  'calib.autoNextDown':
    'The shade is now fully open, which is exactly where a closing run has to start. Measuring it now saves a whole travel.',
  'calib.autoNextUp':
    'The shade is now fully closed, which is exactly where an opening run has to start. Measuring it now saves a whole travel.',
  'calib.autoOptional':
    'Each tap is optional — skip one and that value is left as it was, which is better than storing a worse one.',
  'calib.autoSkipMotion':
    'The two curtain taps are a difference, so your reaction time mostly cancels out of the slat figure. The start delay is one tap and carries it whole, so treat that one as indicative. And if you skip It has started moving, the slat figure is measured from the moment the command was sent, so it will include the start delay.',
  'calib.autoImplausible':
    'The device will not store that: either the run was under a second or over three minutes, or the taps left no travel between them. Nothing has been changed and the run is still open — tap It has stopped again when the shade actually stops, or cancel and start over.',
  'calib.autoInterrupted':
    'This run is over. Something else commanded the shade — these controls, Home Assistant, or a wall remote — or this page was left open too long. Nothing was stored. Put the shade back at the far limit and start again.',
  'calib.autoUnpaired':
    'No motor answers {name} yet, so a guided run would time a shade that never moves. Finish setting it up first. The times above can still be entered by hand.',

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
  'error.commandNotAtThisWidth':
    'this shade is paired with 56-bit frames, which have no step-up command at all — sending one would step it down instead',
  'error.commandRateLimited':
    'that shade has been commanded too often — every command is written to flash before it is sent, so the device paces them. Wait a moment and try again',
  'error.hostNotThisDevice':
    'this request was addressed to a name the device does not answer to — reach it by its own address or its somfy-xxxx.local name',
  'error.originNotThisDevice':
    'this request came from a page the device did not serve, and the device will not act on one',
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
  'error.imageNotFirmware':
    'that file is not a firmware image for this device — build one with `espflash save-image`',
  'error.imageForAnotherChip': 'that firmware was built for a different chip',
  'error.imageTooLarge': 'that firmware is larger than the space reserved for an update',
  'error.imageDamaged': 'the upload did not arrive intact — nothing was changed, try again',
  'error.updateInProgress': 'an update is already being uploaded',
  'error.updateUnwritable':
    'the device could not store the update — it is still running the firmware it had',

  'error.backupNotRecognised':
    'that file is not a backup this device can read — a backup is the file this device writes, or one from the controller it replaces',
  'error.backupTooLarge': 'that backup is larger than this device has room to check',
  'error.backupDamaged':
    'the backup did not arrive intact — its checksum does not match, and nothing was restored',
  'error.backupUnsupportedVersion':
    'that backup was written in a format this firmware does not know how to read',
  'error.restoreInProgress': 'a restore is already running',
  'error.backupUnwritable':
    'the device could not store the restored configuration — it still has the one it had',
  'error.addressInUse': 'a shade on this controller already has that remote address',

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

  'diag.title': 'Diagnostics',
  'diag.intro':
    'What the device can tell you about itself. If something has gone wrong, the log and the panic below are the evidence — copy them before clearing anything.',
  'diag.loading': 'Reading the device…',
  'diag.unreachable': 'Could not reach the device: {detail}',
  'diag.retry': 'Try again',
  'diag.refresh': 'Refresh',
  'diag.refreshing': 'Reading…',

  'diag.identityTitle': 'This device',
  'diag.firmware': 'Firmware',
  'diag.chip': 'Chip',
  'diag.host': 'Name',
  'diag.uptime': 'Running for',
  'diag.resetReason': 'Started by',

  'diag.resetPowerOn': 'Power on',
  'diag.resetPowerOnNote':
    'The power was cut and came back, or somebody plugged it in. Anything the device remembered about a panic was erased with it.',
  'diag.resetSoftware': 'The firmware itself',
  'diag.resetSoftwareNote':
    'The device asked for the restart. Saving settings does that, and so does recovering from a panic.',
  'diag.resetWatchdog': 'The watchdog',
  'diag.resetWatchdogNote':
    'Something stopped answering for long enough to look hung, and the hardware restarted the board. That is a fault, not routine — the log below is where its cause will be, if anything got printed.',
  'diag.resetBrownout': 'A brownout',
  'diag.resetBrownoutNote':
    'The supply voltage fell below what the chip needs. That is almost always the power supply or the cable, not the firmware — and it repeats until the supply is changed.',
  'diag.resetDebugger': 'A debugger',
  'diag.resetDebuggerNote': 'A debugger or a flashing tool restarted it.',
  'diag.resetOther': 'Something else',
  'diag.resetOtherNote':
    'The chip reported a cause this firmware has no name for. Unusual enough to be worth mentioning if you are reporting a fault.',

  'diag.panicTitle': 'The device fell over',
  'diag.panicWhat': 'What it said',
  'diag.panicThisBoot':
    'This is the boot the panic caused. The device restarted itself and came back as what you are looking at now — so whatever led to it happened minutes or seconds before this page loaded.',
  'diag.panicBootsSinceOne': 'One restart ago.',
  'diag.panicBootsSince': '{boots} restarts ago.',
  'diag.panicWhen': 'It had been running for {uptime} when it happened.',
  'diag.panicLoop':
    'It fell over seconds into a boot, and the boot you are talking to now is the one that produced. That is the shape a boot loop has — the device restarts, reaches the same point, and falls over again. Watch the uptime above: if it never gets past a minute, that is what is happening, and nobody gets a window to change anything.',
  'diag.panicTruncated':
    'This text is cut short — the device keeps only its first part. The whole message went to the log below and to the serial line, if anything was listening.',
  'diag.panicVolatile':
    'This record lives in memory the chip keeps across a restart and clears on a power cut, so unplugging the device erases it. Copy it before you do.',
  'diag.panicNoneTitle': 'No panic recorded',
  'diag.panicNone':
    'Either the device has not fallen over, or it has been unplugged since — the record does not survive a power cut, only a restart.',

  'diag.memoryTitle': 'Memory',
  'diag.stackTitle': 'Stack',
  'diag.stackLine':
    '{used} bytes used at the deepest point of this boot, of {required} required — {unspent} unspent.',
  'diag.stackAvailable': 'The linker set aside {available} bytes for it.',
  'diag.stackUnmeasured':
    'Not measured yet on this boot. The figure is read off a painted stack once the controller is running.',
  'diag.stackWhy':
    'Only "used" was measured; the other two are written into the build. The gap between the first two is the whole point of the line — a requirement that has gone stale says nothing at all until a boot contradicts it.',
  'diag.stackStale':
    'This boot used more than the build says it needs, so the requirement is stale. That is the exact state that produced a silent boot loop here once before. Worth reporting.',

  'diag.heapTitle': 'Heap',
  'diag.heapPeak': '{peak} bytes at the highest since boot, of {size}.',
  'diag.heapUsed': '{used} bytes in use right now.',
  'diag.heapWhy':
    'The heap is there for the Wi-Fi driver; nothing else in this firmware allocates, so the peak is a measurement of somebody else’s code. A board that restarts a few seconds into every boot with a peak close to the whole heap has run out — and that looks exactly like a bad access point until these two numbers are seen together.',

  'diag.logTitle': 'Log',
  'diag.logRing': '{bytes} of {capacity} bytes, {lines} lines.',
  'diag.logIntact': 'Nothing has been thrown away, so this is everything since the ring last emptied.',
  'diag.logDropped':
    '{dropped} lines have been thrown away to make room for newer ones. The oldest output is gone — and the oldest output is the boot, which is usually the part worth reading. If you are reporting a fault, say so: it means this ring is too small.',
  'diag.logEmpty': 'The log is empty.',
  'diag.logLoading': 'Reading the log…',
  'diag.logFailed': 'Could not read the log: {detail}',
  'diag.logCopy': 'Copy the log and these details',
  'diag.logCopied': 'Copied — that is the whole page as text, ready to paste into a report.',
  'diag.logCopyFailed':
    'This browser would not let the page write to the clipboard. Serving over plain HTTP is enough for some browsers to withhold it. Select the text above and copy it by hand.',

  'diag.forgetTitle': 'Forget',
  'diag.forget': 'Forget the panic and empty the log',
  'diag.forgetWarning':
    'One action, because they are one thing: the panic record and every line of the log are what this device remembers about its own past, and both go. Nothing is kept anywhere else — not in flash, not in a backup. If you have not copied the log, it is gone. Do this after reporting a fault, not before.',
  'diag.forgetConfirm': 'Yes, forget it',
  'diag.forgetCancel': 'Keep it',
  'diag.forgetting': 'Forgetting…',
  'diag.forgetDone': 'Forgotten. The panic record is cleared and the log is empty.',
  'diag.forgetFailed': 'The device refused: {reason}',

  'diag.durationSeconds': '{seconds} s',
  'diag.durationMinutes': '{minutes} min',
  'diag.durationHours': '{hours} h {minutes} min',
  'diag.durationDay': '1 day, {hours} h',
  'diag.durationDays': '{days} days, {hours} h',

  'backup.title': 'Backup & restore',
  'backup.intro':
    'A backup holds the shade table, the rooms, the groups — and the rolling codes, which are the reason it is worth having. A motor only obeys a remote whose counter it recognises, so a lost rolling code costs a walk to every window and a fresh pairing at each motor. Everything else on this page can be retyped in a few minutes; that cannot.',
  'backup.loading': 'Reading the device…',
  'backup.unreachable': 'Could not reach the device: {detail}',
  'backup.retry': 'Try again',
  'backup.refresh': 'Refresh',
  'backup.refreshing': 'Reading…',

  'backup.exportTitle': 'Save a backup',
  'backup.exportWhat':
    'The file is about four kilobytes: the shade table, the rooms, the groups, and every shade’s rolling code. The device names it somfy-rs.rtsb as it sends it.',
  'backup.exportNotSecrets':
    'It deliberately does not contain the Wi-Fi passphrase or the broker password. Nothing on this device asks who you are, so anything on the network can request this file — and an export carrying secrets would be a way to read the passphrase off the device over the LAN. What it does keep is the network’s name and the broker’s address, so whoever restores it onto a fresh board is told exactly which two values to retype instead of having to guess which network the old one was on.',
  'backup.exportWhen':
    'Take a fresh one after adding or re-pairing a shade. Rolling codes move forward with every command, and a file from last month plants last month’s codes on a board that has none of its own — which a motor rejects as a replay.',
  'backup.export': 'Download the backup',

  'backup.importTitle': 'Restore from a file',
  'backup.importWhat':
    'Two kinds of file are accepted: the .rtsb this device writes, and the .backup an ESPSomfy-RTS controller exports. The second is how an existing installation moves across without re-pairing anything.',
  'backup.importStaged':
    'Uploading does not restore anything on the spot. The device stores the file, restarts, and reads it on the way back up — so this page loses its connection for a few seconds and the answer arrives afterwards. Until that boot has read it, nothing has been checked and nothing has been written.',
  'backup.importCodes':
    'A restore cannot move a rolling code backwards. The device only plants a code for an address it has none for, so restoring an old backup onto the board it came from changes no codes at all and is safe to try. Restoring onto a fresh board is the case that plants them, and it is what a backup is for.',
  'backup.importWhole':
    'A file is applied whole or not at all. One record the device will not accept refuses the lot, and the board keeps the configuration it already had.',
  'backup.file': 'Backup file',
  'backup.anyFile': 'Show every file, not only .rtsb and .backup',
  'backup.fileHint':
    'The extension is only what the picker offers — the device decides by looking at the first bytes, and refuses anything it does not recognise before storing it. Turn the filter off if the file you want has been renamed.',
  'backup.chosen': '{name} — {bytes} bytes',
  'backup.upload': 'Upload and restart',
  'backup.uploading': 'Sending…',
  'backup.uploadRefused':
    'The device refused the file: {reason}. Nothing was stored and nothing has changed.',
  'backup.waiting':
    'Taken. The device is restarting to read the file — it will be unreachable for a few seconds, and this page will say what happened as soon as it answers again.',
  'backup.lost':
    'The device has not answered for a while. It may still be starting, or it may have come back on a different address. Open this page again once you can reach it: the file is stored, and the boot that reads it records what it did.',
  'backup.checkAgain': 'Check again',

  'backup.reportTitle': 'The last restore',
  'backup.outcomeNoneTitle': 'Nothing has been restored',
  'backup.outcomeNone':
    'No backup has ever been uploaded to this device. Everything it holds was set up here.',
  'backup.outcomeStagedTitle': 'A backup is waiting to be applied',
  'backup.outcomeStaged':
    'A file is stored and will be read the next time the device starts. Nothing has been checked yet, and what is running now is the configuration the device already had.',
  'backup.outcomeAppliedTitle': 'Restored',
  'backup.appliedShades': 'Shades written',
  'backup.appliedRooms': 'Rooms written',
  'backup.appliedGroups': 'Groups written',
  'backup.outcomeRefusedTitle': 'The backup was refused',
  'backup.refusedWhy': 'The device refused it: {reason}.',
  'backup.refusedNothing':
    'Nothing was written. The device is running the configuration it had before the upload.',
  'backup.refusedRow': 'It came from record {row} of the file, counting shades from zero.',
  'backup.refusedFile': 'The refusal is about the file itself rather than any one record in it.',

  'backup.format': 'Read as {format}.',
  'backup.formatSomfyRs': 'a backup from a somfy-rs device',
  'backup.formatEspSomfyRts': 'a backup from an ESPSomfy-RTS controller',

  'backup.warningsNone': 'Every record was taken exactly as written.',
  'backup.warningsOne':
    'One record was accepted with a caveat — an unknown kind of shade read as a roller, a group whose rolling code the old controller never wrote out, a member naming a shade that is no longer there. It is a line in the log, with the record and the reason.',
  'backup.warnings':
    '{warnings} records were accepted with a caveat — an unknown kind of shade read as a roller, a group whose rolling code the old controller never wrote out, a member naming a shade that is no longer there. Each one is a line in the log, with the record and the reason.',
  'backup.warningsLink': 'Read the log',

  'backup.retypeTitle': 'What you have to retype',
  'backup.retypeWhy':
    'A backup carries no secrets, so these are the values the restore could not put back for you.',
  'backup.retypeSsid':
    'The device this file came from was on {ssid}. Type that network’s passphrase again under Settings.',
  'backup.retypeSsidOpen':
    'The device this file came from was on {ssid}, which is an open network — there is no passphrase to retype.',
  'backup.retypeNoSsid': 'It had no network stored, so there is nothing to retype for Wi-Fi.',
  'backup.retypeBroker':
    'It published to the broker at {broker}. Type that broker’s password again under Settings.',
  'backup.retypeBrokerOpen': 'It published to the broker at {broker}, which needed no password.',
  'backup.retypeNoBroker': 'It had no broker configured.',
  'backup.retypeUnknown':
    'A backup from an ESPSomfy-RTS controller keeps its network credentials outside the file, so it says nothing about which network or which broker that device used. Check its own settings screen while it is still running, if you can.',
  'backup.retypeLink': 'Open settings',

  'stub.heading': '{screen}',
  'stub.body': 'This screen is not built yet.',
  'stub.settings': 'Settings',
  'stub.backup': 'Backup & restore',
  'stub.onboarding': 'Setup',

  'route.notFound': 'That page does not exist.',
} as const;

/** Every message key in the app. Locales must cover all of them. */
export type MessageKey = keyof typeof en;
