/**
 * French messages.
 *
 * Typed as a **total** `Record<MessageKey, string>`: a key added to `en.ts` and
 * not translated here fails `bun run typecheck`. That is the whole reason the
 * spec asks for both languages on day one rather than retrofitting the second.
 *
 * Typography note: French uses a narrow no-break space before `%`, `:`, `?` and
 * `!` (U+202F). It is written literally in these strings.
 */
import type { MessageKey } from './en';

export const fr: Record<MessageKey, string> = {
  'app.name': 'somfy-rs',

  'nav.settings': 'Réglages',
  'nav.backup': 'Sauvegarde',
  'nav.diagnostics': 'Diagnostics',
  'nav.language': 'Langue',

  'conn.open': 'En direct',
  'conn.connecting': 'Connexion…',
  'conn.closed': 'Reconnexion…',

  'dashboard.title': 'Volets',
  'dashboard.loading': 'Chargement des volets…',
  'dashboard.empty': 'Aucun volet n’est encore configuré.',
  'dashboard.error': 'Appareil injoignable : {detail}',
  'dashboard.retry': 'Réessayer',
  'dashboard.unassigned': 'Sans pièce',
  'dashboard.groupCount': '{count} volets',
  'dashboard.add': 'Ajouter un volet',
  'dashboard.unfinished': 'Configuration à terminer',
  'dashboard.unfinishedWhy':
    'Ces volets ont été ajoutés, mais aucun moteur n’a encore appris à leur répondre : ils ne bougeront pas et n’apparaissent pas dans Home Assistant. Terminer prend quelques minutes, au volet.',
  'dashboard.unfinishedResume': 'Terminer la configuration',

  'shade.open': 'Ouvert',
  'shade.closed': 'Fermé',
  'shade.openPercent': 'ouvert à {percent} %',
  'shade.opening': 'Ouverture',
  'shade.closing': 'Fermeture',
  'shade.idle': 'Arrêté',
  'shade.favourite': 'Position favorite : ouvert à {percent} %',
  'shade.noFavourite': 'Aucune position favorite',
  'shade.openPercentApprox': 'ouvert à environ {percent} %',
  'shade.uncertainAria':
    'L’appareil n’a pas vu ce volet atteindre une butée depuis son dernier déplacement partiel : ce chiffre peut être faux de {margin} points de pourcentage. L’ouvrir ou le fermer complètement le rendra de nouveau exact.',

  'command.up': 'Ouvrir',
  'command.my': 'Favori',
  'command.down': 'Fermer',
  'command.upAria': 'Ouvrir {name}',
  'command.myAria': 'Mettre {name} en position favorite, ou l’arrêter',
  'command.downAria': 'Fermer {name}',
  'command.sliderAria': 'Ouverture de {name}, en pourcentage',
  'command.vent': 'Aération',
  'command.ventAria':
    'Fermer complètement {name}, puis écarter les lames juste assez pour laisser passer la lumière',
  'command.ventUnavailable':
    'Le temps d’écartement des lames n’a pas été mesuré : l’aération n’a donc rien à viser. Mesurez-le dans Temps de course.',

  'tilt.none': 'Sans inclinaison',
  'tilt.motor': 'Moteur d’inclinaison séparé',
  'tilt.integrated': 'Inclinaison intégrée',
  'tilt.tiltOnly': 'Inclinaison seule',
  'tilt.euro': 'Mode européen',

  'kind.roller': 'Volet roulant',
  'kind.blind': 'Store vénitien',
  'kind.draperyLeft': 'Rideau (gauche)',
  'kind.awning': 'Store banne',
  'kind.shutter': 'Volet battant',
  'kind.draperyRight': 'Rideau (droite)',
  'kind.draperyCenter': 'Rideau (centre)',
  'kind.unknown': 'Volet',

  'detail.back': 'Retour au tableau de bord',
  'detail.notFound': 'Aucun volet avec l’identifiant {id}.',
  'detail.address': 'Adresse de la télécommande',
  'detail.travelTimes': 'Temps de course',
  'detail.upTime': 'Ouverture',
  'detail.downTime': 'Fermeture',
  'detail.tiltTime': 'Inclinaison',
  'detail.seconds': '{seconds} s',
  'detail.tilt': 'Inclinaison',
  'detail.tiltNone': 'Ce volet n’a pas d’inclinaison.',
  'calib.factoryDefault': 'non étalonné',
  'calib.operatorSupplied': 'saisi à la main',
  'calib.measured': 'mesuré par l’appareil',
  'calib.uncalibratedWarning':
    'Au moins une de ces valeurs est encore celle d’usine. Personne n’a mesuré ce volet : une commande vers une position intermédiaire sera fausse, souvent très fausse. Des valeurs identiques sur plusieurs volets prouvent que personne ne les a choisies, pas qu’elles sont justes.',
  'calib.hint':
    'Chronométrez une course complète, séparément dans chaque sens : la fermeture est généralement plus rapide, car la gravité aide. Dix secondes d’écart sur un volet de trente secondes représentent un tiers de la course.',
  'calib.save': 'Enregistrer les temps de course',
  'calib.saving': 'Enregistrement…',
  'calib.revert': 'Annuler les modifications',
  'calib.saved': 'Enregistré.',
  'calib.failed': 'L’appareil a refusé ces valeurs : {reason}',

  'calib.startLag': 'Délai de démarrage',
  'calib.ventBand': 'Écartement des lames',
  'calib.closeBand': 'Serrage des lames',
  'calib.bandsHint':
    'Une course ne fait pas bouger le tablier sur toute sa durée. La première fraction de seconde sert à transmettre la commande au moteur et à le démarrer ; sur un volet à lames perforées, les premières secondes d’ouverture ne font qu’écarter les lames, et les dernières secondes de fermeture ne font que les resserrer. Ces trois durées font partie des temps ci-dessus, elles ne s’y ajoutent pas : les mesurer rend les positions intermédiaires plus justes sans changer la durée d’une course complète.',
  'calib.ventBandHint':
    'C’est aussi là que s’arrête la commande Aération. Laissée à zéro, la commande n’est pas proposée.',

  'calib.autoTitle': 'Mesurer automatiquement',
  'calib.autoHint':
    'L’appareil chronomètre le volet pendant que vous le regardez. Placez d’abord le volet à la butée opposée, lancez la mesure, puis appuyez au fur et à mesure. Rien n’est enregistré avant la fin, et annuler n’enregistre rien.',
  'calib.autoUp': 'Mesurer l’ouverture',
  'calib.autoDown': 'Mesurer la fermeture',
  'calib.autoUpPrep':
    'Fermez complètement le volet et attendez son arrêt. Lancez ensuite : le volet s’ouvrira et il vous sera demandé d’appuyer trois fois.',
  'calib.autoDownPrep':
    'Ouvrez complètement le volet et attendez son arrêt. Lancez ensuite : le volet se fermera et il vous sera demandé d’appuyer deux fois.',
  'calib.autoStart': 'Lancer et ouvrir',
  'calib.autoStartDown': 'Lancer et fermer',
  'calib.autoRunning': 'En cours — {elapsed} s',
  'calib.autoMarkMotion': 'Il a commencé à bouger',
  'calib.autoMarkCurtainUp': 'Le tablier commence à monter',
  'calib.autoMarkCurtainDown': 'Le tablier est arrivé en bas',
  'calib.autoFinish': 'Il s’est arrêté',
  'calib.autoCancel': 'Annuler',
  'calib.autoMarked': 'Noté.',
  'calib.autoDone':
    'Mesuré. Les temps ci-dessus ont été mis à jour, et le volet est à une butée : sa position est de nouveau exacte.',
  'calib.autoOptional':
    'Chaque appui est facultatif — si vous en sautez un, la valeur correspondante reste inchangée.',

  'detail.linkedRemotes': 'Télécommandes associées',
  'detail.linkedRemotesPending': 'Pas encore implémenté.',
  'detail.origin': 'Origine de cette adresse',
  'detail.originAllocated': 'Attribuée par ce contrôleur',
  'detail.originImported': 'Importée d’un autre contrôleur',
  'detail.originAllocatedNote':
    'Aucun autre contrôleur n’utilise cette adresse. Un moteur ne lui obéit qu’une fois l’appairage effectué.',
  'detail.originImportedNote':
    'Cette adresse appartient au contrôleur dont elle a été importée. Si celui-ci fonctionne encore, les deux forment désormais une seule télécommande avec deux compteurs distincts, et le premier à prendre du retard cessera d’être obéi. L’appairage n’y change rien : il réapprendrait au moteur cette même adresse partagée.',
  'detail.pair': 'Réappairer ce volet',
  'detail.unfinishedTitle': 'Configuration inachevée',
  'detail.unfinishedBody':
    '{name} possède sa propre adresse de télécommande, et aucun moteur ne l’a encore apprise : rien ne répond à ces commandes et aucune entité n’existe dans Home Assistant. Pour terminer, il faut être au volet avec une télécommande qui fonctionne déjà.',
  'detail.unfinishedResume': 'Terminer la configuration de {name}',
  'detail.dangerZone': 'Supprimer',
  'detail.delete': 'Supprimer {name}',
  'detail.deleteWarning':
    'Ceci retire {name} de ce contrôleur uniquement. Le moteur n’en est pas informé, et ne peut pas l’être : il continue d’obéir à toutes les télécommandes qu’il a apprises, y compris celle-ci. Il n’existe volontairement aucune commande de désappairage ici — sur une télécommande physique, désappairer se fait par un appui long sur PROG, et une salve à peine trop longue supprime une télécommande d’un volet qui fonctionnait.',
  'detail.deleteConfirm': 'Oui, supprimer {name}',
  'detail.deleteCancel': 'Conserver',
  'detail.deleting': 'Suppression…',

  'add.title': 'Ajouter un volet',
  'add.progress': 'Étape 1 : de quoi il s’agit',
  'add.intro':
    'Le contrôleur attribuera à ce volet sa propre adresse de télécommande. Aucun moteur ne connaît encore cette adresse : l’étape suivante consiste à l’apprendre à l’un d’eux, et vous y serez conduit directement. Le volet n’apparaîtra dans Home Assistant qu’une fois configuré et après que vous l’aurez vu bouger.',
  'add.name': 'Nom',
  'add.nameHint': '{used} octets sur {max}. Les lettres accentuées en comptent deux.',
  'add.kind': 'Type',
  'add.tiltMode': 'Inclinaison',
  'add.tiltHint':
    'Enregistrée mais pas encore exploitée : ce micrologiciel ne pilote que l’axe de montée et descente.',
  'add.times': 'Temps de course',
  'add.timesHint':
    'Durée d’une course complète, en secondes. L’estimation de position en découle : une mesure approximative vaut bien mieux qu’une supposition. Ces valeurs restent modifiables ensuite.',
  'add.upTime': 'Ouverture',
  'add.downTime': 'Fermeture',
  'add.tiltTime': 'Inclinaison',
  'add.submit': 'Ajouter le volet',
  'add.submitting': 'Ajout…',
  'add.cancel': 'Annuler',
  'add.failed': 'L’appareil a refusé ce volet : {reason}',

  'pair.title': 'Configuration de {name}',
  'pair.progress': 'Étape {step} sur {total}',
  'pair.additive':
    'Rien n’est supprimé. L’appairage ajoute ce contrôleur au moteur ; toutes les télécommandes qui fonctionnent aujourd’hui continueront de fonctionner.',
  'pair.unfinished':
    '{name} n’est pas terminé. Tant que vous ne l’avez pas appairé et vu bouger, il n’apparaît pas dans Home Assistant et rien ne lui répond.',

  'pair.step1Title': 'Avant de commencer',
  'pair.step1Remote':
    'Il vous faut une télécommande qui pilote déjà ce volet — une télécommande murale, ou un autre contrôleur. Celui-ci ne peut pas la remplacer : un moteur qui ne le connaît pas ignore tout ce qu’il émet, y compris le signal d’appairage.',
  'pair.step1See':
    'Vous devez pouvoir voir le volet. La seule confirmation de cette procédure est le mouvement du volet, et personne d’autre ne le surveille.',
  'pair.step1Still':
    'Idéalement le volet est à l’arrêt. Appairer un volet en pleine course fausse son estimation de position jusqu’à la prochaine ouverture ou fermeture complète, qui la corrige.',
  'pair.step1Next': 'J’ai une télécommande qui fonctionne — continuer',

  'pair.step2Title': 'Mettre le moteur en mode programmation',
  'pair.step2Hold':
    'Au volet, maintenez le bouton PROG de la télécommande existante enfoncé environ deux secondes, jusqu’à ce que le volet fasse un va-et-vient — un bref mouvement de haut en bas.',
  'pair.step2Recessed':
    'Sur la plupart des télécommandes, PROG est un petit bouton encastré à l’arrière : il faut un stylo ou un trombone.',
  'pair.step2Channel':
    'Sur une télécommande multicanal, sélectionnez le canal de ce volet avant d’appuyer sur PROG.',
  'pair.step2Window':
    'Le moteur reste ensuite en mode programmation pendant environ deux minutes.',
  'pair.step2Next': 'Le volet a bougé — continuer',
  'pair.step2Back': 'Retour',

  'pair.step3Title': 'Envoyer le signal d’appairage',
  'pair.step3Body':
    'Le contrôleur va émettre le signal d’appairage vers {name}. Envoyez-le pendant que le moteur est encore en mode programmation.',
  'pair.step3Send': 'Envoyer le signal d’appairage',
  'pair.step3Sending': 'Envoi…',
  'pair.step3Remaining': 'Environ {time} restantes sur la fenêtre de programmation.',
  'pair.step3Expired':
    'Les deux minutes sont probablement écoulées. Remettez le moteur en mode programmation avant de renvoyer le signal.',
  'pair.step3Sent':
    'Envoyé. Regardez le volet : un bref va-et-vient signifie que le moteur a reçu le signal.',
  'pair.step3NoFeedback':
    'Rien de plus n’apparaîtra ici. Le contrôleur émet et ne reçoit jamais de réponse : il ne peut pas vous dire si le moteur a accepté le signal. Le va-et-vient est bon signe et facile à manquer : ce n’est donc pas lui qui décide — le vrai test est à l’étape suivante.',
  'pair.step3Next': 'Continuer — tester le volet',
  'pair.step3No': 'Rien ne s’est passé du tout',
  'pair.step3Failed': 'L’appareil n’a pas voulu l’envoyer : {reason}',

  'pair.step4Title': 'Le tester',
  'pair.step4Body':
    'Ouvrez et fermez {name} depuis ici, en regardant le volet lui-même. C’est exactement la commande qu’enverra Home Assistant : ce test valide toute la chaîne, et pas seulement l’arrivée d’un signal.',
  'pair.step4Limit':
    'Si le volet est déjà complètement ouvert, « Ouvrir » ne produit aucun mouvement visible — ce qui ressemble exactement à un échec. Essayez l’autre sens avant de conclure.',
  'pair.step4Why':
    'Rien sur cet écran ne vous dira si cela a fonctionné, et c’est volontaire. Le contrôleur n’entend pas le moteur : toute position affichée ici ne serait que sa propre estimation. Vous êtes le seul instrument disponible.',
  'pair.step4Question': '{name} a-t-il vraiment bougé ?',
  'pair.step4OnlyYou':
    'Répondez d’après ce que vous avez vu au volet, pas d’après ce qui s’est passé à l’écran. Répondre oui est ce qui ajoute {name} à Home Assistant.',
  'pair.step4Yes': 'Oui — il a bougé',
  'pair.step4No': 'Non — il n’a pas bougé',
  'pair.confirming': 'Finalisation…',
  'pair.confirmFailed': 'L’appareil n’a pas pu l’enregistrer : {reason}',

  'pair.doneTitle': '{name} est configuré',
  'pair.doneWitness':
    'C’est vous qui l’avez vu bouger ; le contrôleur ne l’a pas vu et ne le pouvait pas. C’est pourquoi il vous l’a demandé : c’est la seule preuve que permet ce protocole.',
  'pair.doneAnnounced':
    '{name} est désormais dans Home Assistant, et ses entités réapparaîtront d’elles-mêmes après un redémarrage de l’appareil ou du serveur MQTT.',
  'pair.doneEnd':
    'Le mode programmation se termine de lui-même après une ou deux minutes. Pour y mettre fin tout de suite, appuyez à nouveau sur PROG de la télécommande existante.',
  'pair.doneBack': 'Aller à {name}',

  'pair.retryTitle': 'Cela n’a pas fonctionné',
  'pair.retryIntro': 'Dans cet ordre, en commençant par les vérifications les moins coûteuses.',
  'pair.retryWindow':
    'La fenêtre de programmation s’est refermée. Deux minutes, c’est confortable mais pas illimité. Remettez le moteur en mode programmation et renvoyez le signal — c’est de loin la cause la plus fréquente.',
  'pair.retryChannel':
    'La télécommande était sur un autre canal : un autre volet est passé en mode programmation, ou aucun.',
  'pair.retryCode':
    'L’appairage a réussi et le volet ignore quand même les commandes. C’est généralement le code tournant : un moteur refuse tout code inférieur ou égal au dernier qu’il a accepté. Refaire l’appairage corrige cela, car l’appairage apprend au moteur ce qui est émis maintenant.',
  'pair.retryRange':
    'Le signal n’a pas atteint le moteur. Rapprochez le contrôleur du volet, ou vérifiez son antenne.',
  'pair.retryAgain': 'Reprendre au mode programmation',
  'pair.retryStop': 'Arrêter pour l’instant',

  'pair.abandon': 'Abandonner ce volet',
  'pair.abandonWarning':
    'Ceci retire {name} du contrôleur. Rien d’autre n’est affecté : il n’a jamais été ajouté à Home Assistant, il n’y a donc rien à y nettoyer, et aucune télécommande qui fonctionne aujourd’hui ne cessera de fonctionner. Vous pourrez l’ajouter de nouveau quand vous voudrez.',
  'pair.abandonConfirm': 'Oui, abandonner {name}',
  'pair.abandonCancel': 'Le conserver',
  'pair.abandoning': 'Abandon…',

  'pair.alreadyTitle': 'Fonctionne-t-il déjà ?',
  'pair.alreadyBody':
    'Cette adresse provient d’un autre contrôleur : un moteur y répond peut-être déjà. Essayez d’ouvrir et de fermer {name} ; s’il obéit, sa configuration est terminée et vous pouvez le confirmer.',
  'pair.alreadyConfirm': 'Il a bougé — terminer la configuration',

  'pair.blockedTitle': 'L’appairage n’est pas disponible pour ce volet',
  'pair.blockedBody':
    'L’adresse de télécommande de {name} a été importée d’un autre contrôleur : le moteur la connaît déjà, et cet autre contrôleur aussi. L’appairage réapprendrait au moteur cette même adresse partagée, ce qui est le problème et non la solution.',
  'pair.blockedAdvice':
    'Pour rattacher {name} à ce contrôleur, ajoutez-le de nouveau comme un volet neuf — l’appareil lui attribuera sa propre adresse — appairez celui-là, puis supprimez cette entrée une fois qu’il fonctionne.',
  'pair.blockedBack': 'Retour à {name}',

  'error.nameEmpty': 'le nom est vide',
  'error.nameTooLong': 'le nom dépasse 32 octets',
  'error.invalidKind': 'ce micrologiciel ne gère pas ce type de volet',
  'error.invalidTiltMode': 'ce micrologiciel ne gère pas ce mode d’inclinaison',
  'error.travelTimeZero':
    'un temps de course nul prive l’estimation de position de toute échelle',
  'error.invalidAddress': 'l’adresse attribuée par l’appareil n’est pas utilisable',
  'error.registryFull': 'ce contrôleur est plein — 32 volets au maximum',
  'error.notFound': 'ce volet n’existe plus',
  'error.addressNotAllocated': 'l’adresse de ce volet appartient à un autre contrôleur',
  'error.invalidDeadBand':
    'le délai de démarrage et les temps de lames doivent laisser de la course derrière eux — ce sont des parties d’un temps de course, pas du temps en plus',
  'error.ventBandNotMeasured':
    'le temps d’écartement des lames n’a jamais été mesuré, et c’est la seule chose que vise l’aération',
  'error.notCalibrating': 'cette mesure n’est plus en cours',
  'error.calibrationImplausible':
    'l’appareil refuse ces valeurs — une course nulle, ou de plus de trois minutes, ou des appuis ne laissant aucune course entre eux',
  'error.commandNotAtThisWidth':
    'ce store est appairé en trames 56 bits, qui n’ont aucune commande de pas vers le haut — en envoyer une le ferait descendre d’un pas',
  'error.commandRateLimited':
    'ce volet a reçu trop de commandes — chacune est écrite en mémoire flash avant d’être émise, l’appareil les espace donc. Patientez un instant et réessayez',
  'error.hostNotThisDevice':
    'cette requête visait un nom auquel l’appareil ne répond pas — utilisez son adresse ou son nom somfy-xxxx.local',
  'error.originNotThisDevice':
    'cette requête provient d’une page que l’appareil n’a pas servie, et il refuse d’y donner suite',
  'error.unknown': 'l’appareil n’en a pas donné la raison',


  'error.valueEmpty': '{field} ne doit pas être vide',
  'error.valueTooLong': '{field} dépasse ce que l’appareil peut enregistrer',
  'error.valueTooShort':
    '{field} est trop court — une phrase secrète Wi-Fi demande au moins 8 caractères',
  'error.valueInteriorNul':
    '{field} contient un caractère NUL, que MQTT n’autorise pas dans une chaîne',
  'error.brokerAddressMalformed':
    '{field} doit être quatre nombres séparés par des points, comme 192.168.1.10',
  'error.brokerAddressUnroutable':
    '{field} est une adresse qu’aucune connexion ne peut atteindre — ni 0.0.0.0, ni une boucle locale, ni une adresse de multidiffusion ou de diffusion',
  'error.brokerPortZero': '{field} ne doit pas être zéro',
  'error.passwordWithoutUsername': 'un mot de passe de courtier exige un {field}',
  'error.topicWildcard':
    '{field} ne doit pas contenir # ni +, qui n’ont leur place que dans un abonnement',
  'error.topicLeadingSlash': '{field} ne doit pas commencer par /',
  'error.topicTrailingSlash': '{field} ne doit pas se terminer par /',
  'error.topicEmptySegment': '{field} ne doit pas contenir //',
  'error.topicIllegalCharacter':
    '{field} ne peut contenir que des lettres, des chiffres, _, - et / comme séparateur',
  'error.namespacesOverlap':
    '{field} ne doit pas être identique au préfixe de découverte ni se trouver à l’intérieur — l’appareil publierait sa disponibilité sur le sujet propre à Home Assistant',
  'error.secretNotSet':
    'aucun {field} n’est enregistré à conserver — saisissez-en un, ou choisissez de ne pas en avoir',
  'error.noTrialInProgress': 'ce test de réseau est déjà terminé',
  'error.trialInProgress': 'un test de réseau est déjà en cours — terminez-le ou annulez-le d’abord',
  'error.trialNotAssociated':
    'l’appareil n’est pas encore sur le nouveau réseau : impossible de confirmer',
  'error.settingsUnwritable':
    'l’appareil n’a pas pu enregistrer les réglages — rien n’a été modifié',
  'error.imageNotFirmware':
    'ce fichier n’est pas une image de micrologiciel pour cet appareil — créez-en une avec « espflash save-image »',
  'error.imageForAnotherChip': 'ce micrologiciel a été compilé pour une autre puce',
  'error.imageTooLarge':
    'ce micrologiciel dépasse l’espace réservé à une mise à jour',
  'error.imageDamaged':
    'l’envoi n’est pas arrivé intact — rien n’a été modifié, réessayez',
  'error.updateInProgress': 'une mise à jour est déjà en cours d’envoi',
  'error.updateUnwritable':
    'l’appareil n’a pas pu enregistrer la mise à jour — il fonctionne toujours avec le micrologiciel qu’il avait',

  'error.backupNotRecognised':
    'ce fichier n’est pas une sauvegarde que cet appareil sache lire — une sauvegarde est le fichier qu’il produit lui-même, ou celui du contrôleur qu’il remplace',
  'error.backupTooLarge': 'cette sauvegarde dépasse la place dont dispose l’appareil pour la vérifier',
  'error.backupDamaged':
    'la sauvegarde n’est pas arrivée intacte — sa somme de contrôle ne correspond pas, et rien n’a été restauré',
  'error.backupUnsupportedVersion':
    'cette sauvegarde est écrite dans un format que ce micrologiciel ne sait pas lire',
  'error.restoreInProgress': 'une restauration est déjà en cours',
  'error.backupUnwritable':
    'l’appareil n’a pas pu enregistrer la configuration restaurée — il conserve celle qu’il avait',
  'error.addressInUse': 'un volet de ce contrôleur possède déjà cette adresse de télécommande',

  'settings.title': 'Réglages',
  'settings.loading': 'Lecture des réglages de l’appareil…',
  'settings.unreachable': 'Appareil injoignable : {detail}',
  'settings.retry': 'Réessayer',

  'settings.wifiTitle': 'Wi-Fi',
  'settings.wifiIntro':
    'L’appareil rejoint ce réseau à chaque démarrage. Le changer se fait sous forme d’un test que vous devez confirmer depuis le nouveau réseau — voir ci-dessous.',
  'settings.wifiNone': 'Aucun réseau enregistré. Cet appareil a été configuré par USB.',
  'settings.wifiSsid': 'nom du réseau',
  'settings.wifiPsk': 'phrase secrète',
  'settings.wifiPskStored': 'Une phrase secrète est enregistrée.',
  'settings.wifiPskOpen': 'Aucune phrase secrète — réseau ouvert.',
  'settings.secretKeep': 'Conserver la valeur enregistrée',
  'settings.secretSet': 'Saisir une nouvelle valeur',
  'settings.secretClear': 'Il ne doit pas y en avoir',
  'settings.wifiWarn':
    'L’appareil va quitter ce réseau pour essayer {ssid}. Rejoignez {ssid} vous-même et rouvrez cette page dans les {minutes} minutes pour le conserver. Si personne ne le fait, l’appareil redémarre sur {current} et rien n’est enregistré.',
  'settings.wifiWarnNoCurrent':
    'L’appareil va quitter ce réseau pour essayer {ssid}. Rejoignez {ssid} vous-même et rouvrez cette page dans les {minutes} minutes pour le conserver. Si personne ne le fait, l’appareil redémarre et rien n’est enregistré.',
  'settings.wifiSubmit': 'Tester ce réseau',
  'settings.wifiSubmitting': 'Démarrage du test…',

  'settings.trialTitle': 'Test de {ssid}',
  'settings.trialAssociating':
    'L’appareil rejoint {ssid}. Il a quitté le réseau sur lequel vous étiez.',
  'settings.trialAwaiting':
    'L’appareil est sur {ssid} et a reçu une adresse. Confirmez dans les {seconds} s pour le conserver.',
  'settings.trialLeft':
    'L’appareil a quitté ce réseau. Rejoignez {ssid}, rouvrez cette page et confirmez — sinon il redémarrera de lui-même sur le réseau enregistré.',
  'settings.trialRemaining': '{seconds} s restantes',
  'settings.trialConfirm': 'Je l’atteins — conserver ce réseau',
  'settings.trialConfirming': 'Enregistrement…',
  'settings.trialCancel': 'Annuler et revenir en arrière',
  'settings.trialCancelled':
    'Retour au réseau enregistré. L’appareil redémarre.',
  'settings.trialSaved': 'Enregistré. C’est désormais le réseau de l’appareil.',

  'settings.mqttTitle': 'Courtier Home Assistant (MQTT)',
  'settings.mqttIntro':
    'Facultatif. Sans courtier, l’appareil continue de recevoir, décoder et suivre chaque store — il ne publie simplement rien.',
  'settings.mqttNone': 'Aucun courtier configuré.',
  'settings.mqttAddress': 'adresse du courtier',
  'settings.mqttPort': 'port du courtier',
  'settings.mqttUsername': 'identifiant du courtier',
  'settings.mqttUsernameHint': 'Laisser vide pour une connexion anonyme.',
  'settings.mqttPassword': 'mot de passe du courtier',
  'settings.mqttPasswordStored': 'Un mot de passe est enregistré.',
  'settings.mqttPasswordNone': 'Aucun mot de passe — la connexion est anonyme.',
  'settings.mqttDiscoveryPrefix': 'préfixe de découverte',
  'settings.mqttDiscoveryPrefixHint':
    'Là où Home Assistant cherche les configurations d’appareils. Global à tout votre Home Assistant — laissez homeassistant sauf si vous savez qu’il a été changé.',
  'settings.mqttStateRoot': 'racine des sujets d’état',
  'settings.mqttStateRootHint':
    'Là où cet appareil publie ses propres sujets. Elle ne doit pas être le préfixe de découverte, ni se trouver à l’intérieur.',
  'settings.mqttWarn':
    'Enregistrer redémarre l’appareil. C’est ce qui efface les entités Home Assistant retenues publiées sous les anciens sujets, avant que les nouvelles ne partent.',
  'settings.mqttSubmit': 'Enregistrer et redémarrer',
  'settings.mqttSubmitting': 'Enregistrement…',
  'settings.mqttClear': 'Fonctionner sans courtier',
  'settings.mqttClearing': 'Suppression…',
  'settings.mqttConfirmClear': 'Supprimer le courtier et redémarrer ?',
  'settings.mqttCleared': 'Enregistré. L’appareil redémarre sans courtier.',
  'settings.mqttSaved': 'Enregistré. L’appareil redémarre.',
  'settings.restarting':
    'L’appareil redémarre. Cette page reviendra d’elle-même dans quelques secondes.',
  'settings.failed': 'Refusé : {reason}',

  'diag.title': 'Diagnostics',
  'diag.intro':
    'Ce que l’appareil peut dire de lui-même. En cas de problème, le journal et la panique ci-dessous sont les pièces à conviction — copiez-les avant d’effacer quoi que ce soit.',
  'diag.loading': 'Lecture de l’appareil…',
  'diag.unreachable': 'Appareil injoignable : {detail}',
  'diag.retry': 'Réessayer',
  'diag.refresh': 'Actualiser',
  'diag.refreshing': 'Lecture…',

  'diag.identityTitle': 'Cet appareil',
  'diag.firmware': 'Micrologiciel',
  'diag.chip': 'Puce',
  'diag.host': 'Nom',
  'diag.uptime': 'En marche depuis',
  'diag.resetReason': 'Démarré par',

  'diag.resetPowerOn': 'La mise sous tension',
  'diag.resetPowerOnNote':
    'Le courant a été coupé puis rétabli, ou quelqu’un a branché l’appareil. Tout ce qu’il gardait en mémoire d’une panique a été effacé avec.',
  'diag.resetSoftware': 'Le micrologiciel lui-même',
  'diag.resetSoftwareNote':
    'L’appareil a demandé ce redémarrage. Enregistrer des réglages le fait, et se relever d’une panique aussi.',
  'diag.resetWatchdog': 'Le chien de garde',
  'diag.resetWatchdogNote':
    'Quelque chose a cessé de répondre assez longtemps pour paraître bloqué, et le matériel a redémarré la carte. C’est une anomalie, pas une routine — la cause se trouve dans le journal ci-dessous, si elle a eu le temps d’être écrite.',
  'diag.resetBrownout': 'Une baisse de tension',
  'diag.resetBrownoutNote':
    'La tension d’alimentation est descendue sous ce dont la puce a besoin. C’est presque toujours l’alimentation ou le câble, pas le micrologiciel — et cela se reproduira tant que l’alimentation ne changera pas.',
  'diag.resetDebugger': 'Un débogueur',
  'diag.resetDebuggerNote': 'Un débogueur ou un outil de flashage l’a redémarré.',
  'diag.resetOther': 'Autre chose',
  'diag.resetOtherNote':
    'La puce a signalé une cause à laquelle ce micrologiciel ne sait pas donner de nom. Assez rare pour mériter d’être mentionné dans un rapport d’anomalie.',

  'diag.panicTitle': 'L’appareil s’est effondré',
  'diag.panicWhat': 'Ce qu’il a dit',
  'diag.panicThisBoot':
    'Ce démarrage-ci est celui qu’a provoqué la panique. L’appareil s’est redémarré tout seul et est revenu sous la forme que vous avez sous les yeux — ce qui l’a causée s’est donc produit quelques minutes, voire quelques secondes, avant l’ouverture de cette page.',
  'diag.panicBootsSinceOne': 'Il y a un redémarrage.',
  'diag.panicBootsSince': 'Il y a {boots} redémarrages.',
  'diag.panicWhen': 'Il fonctionnait depuis {uptime} au moment des faits.',
  'diag.panicLoop':
    'Il s’est effondré quelques secondes après un démarrage, et celui auquel vous parlez est celui qu’a produit ce redémarrage. C’est la forme d’une boucle de démarrage : l’appareil repart, atteint le même point et s’effondre de nouveau. Surveillez la durée de marche ci-dessus — si elle ne dépasse jamais une minute, c’est bien ce qui se passe, et personne n’aura le temps de changer quoi que ce soit.',
  'diag.panicTruncated':
    'Ce texte est tronqué — l’appareil n’en conserve que le début. Le message complet est parti dans le journal ci-dessous et sur la liaison série, si quelque chose écoutait.',
  'diag.panicVolatile':
    'Cet enregistrement réside dans une mémoire que la puce conserve d’un redémarrage à l’autre et vide à la coupure du courant : débrancher l’appareil l’efface. Copiez-le avant.',
  'diag.panicNoneTitle': 'Aucune panique enregistrée',
  'diag.panicNone':
    'Soit l’appareil ne s’est pas effondré, soit il a été débranché depuis — l’enregistrement ne survit pas à une coupure de courant, seulement à un redémarrage.',

  'diag.memoryTitle': 'Mémoire',
  'diag.stackTitle': 'Pile',
  'diag.stackLine':
    '{used} octets utilisés au point le plus profond de ce démarrage, sur {required} requis — {unspent} inutilisés.',
  'diag.stackAvailable': 'L’éditeur de liens lui a réservé {available} octets.',
  'diag.stackUnmeasured':
    'Pas encore mesuré sur ce démarrage. La valeur se lit sur une pile peinte, une fois le contrôleur en marche.',
  'diag.stackWhy':
    'Seul « utilisés » a été mesuré ; les deux autres sont inscrits dans la compilation. L’écart entre les deux premiers est tout l’intérêt de cette ligne — une exigence devenue caduque ne dit rien du tout tant qu’un démarrage ne la contredit pas.',
  'diag.stackStale':
    'Ce démarrage a consommé plus que ce que la compilation déclare nécessaire : l’exigence est caduque. C’est exactement l’état qui a produit ici, une fois, une boucle de démarrage silencieuse. À signaler.',

  'diag.heapTitle': 'Tas',
  'diag.heapPeak': '{peak} octets au maximum depuis le démarrage, sur {size}.',
  'diag.heapUsed': '{used} octets utilisés en ce moment.',
  'diag.heapWhy':
    'Le tas existe pour le pilote Wi-Fi ; rien d’autre dans ce micrologiciel n’alloue, si bien que ce maximum mesure le code de quelqu’un d’autre. Une carte qui redémarre quelques secondes après chaque démarrage, avec un maximum proche de la totalité du tas, en a manqué — et cela ressemble en tout point à un mauvais point d’accès tant que ces deux nombres ne sont pas vus côte à côte.',

  'diag.logTitle': 'Journal',
  'diag.logRing': '{bytes} octets sur {capacity}, {lines} lignes.',
  'diag.logIntact':
    'Rien n’a été jeté : voici donc tout ce qui s’est écrit depuis le dernier vidage.',
  'diag.logDropped':
    '{dropped} lignes ont été jetées pour faire place aux suivantes. Les plus anciennes sont perdues — et les plus anciennes, ce sont celles du démarrage, généralement les plus intéressantes. Si vous signalez une anomalie, dites-le : cela veut dire que cette mémoire tampon est trop petite.',
  'diag.logEmpty': 'Le journal est vide.',
  'diag.logLoading': 'Lecture du journal…',
  'diag.logFailed': 'Lecture du journal impossible : {detail}',
  'diag.logCopy': 'Copier le journal et ces informations',
  'diag.logCopied': 'Copié — toute la page sous forme de texte, prête à être collée dans un rapport.',
  'diag.logCopyFailed':
    'Ce navigateur refuse à la page l’accès au presse-papiers. Une desserte en HTTP simple suffit à ce que certains navigateurs le retirent. Sélectionnez le texte ci-dessus et copiez-le à la main.',

  'diag.forgetTitle': 'Oublier',
  'diag.forget': 'Oublier la panique et vider le journal',
  'diag.forgetWarning':
    'Une seule action, parce qu’il s’agit d’une seule chose : l’enregistrement de la panique et chaque ligne du journal sont ce que cet appareil garde de son propre passé, et les deux s’en vont. Rien n’en est conservé ailleurs — ni en mémoire flash, ni dans une sauvegarde. Si vous n’avez pas copié le journal, il est perdu. À faire après avoir signalé une anomalie, pas avant.',
  'diag.forgetConfirm': 'Oui, oublier',
  'diag.forgetCancel': 'Conserver',
  'diag.forgetting': 'Effacement…',
  'diag.forgetDone': 'Oublié. L’enregistrement de la panique est effacé et le journal est vide.',
  'diag.forgetFailed': 'L’appareil a refusé : {reason}',

  'diag.durationSeconds': '{seconds} s',
  'diag.durationMinutes': '{minutes} min',
  'diag.durationHours': '{hours} h {minutes} min',
  'diag.durationDay': '1 jour, {hours} h',
  'diag.durationDays': '{days} jours, {hours} h',

  'backup.title': 'Sauvegarde et restauration',
  'backup.intro':
    'Une sauvegarde contient la table des volets, les pièces, les groupes — et les codes tournants, qui en sont toute la raison d’être. Un moteur n’obéit qu’à une télécommande dont il reconnaît le compteur : un code tournant perdu, et il faut retourner à chaque fenêtre réappairer chaque moteur. Tout le reste de cette page se ressaisit en quelques minutes ; cela, non.',
  'backup.loading': 'Lecture de l’appareil…',
  'backup.unreachable': 'Appareil injoignable : {detail}',
  'backup.retry': 'Réessayer',
  'backup.refresh': 'Actualiser',
  'backup.refreshing': 'Lecture…',

  'backup.exportTitle': 'Enregistrer une sauvegarde',
  'backup.exportWhat':
    'Le fichier fait environ quatre kilo-octets : la table des volets, les pièces, les groupes, et le code tournant de chaque volet. L’appareil le nomme somfy-rs.rtsb en l’envoyant.',
  'backup.exportNotSecrets':
    'Il ne contient délibérément ni la phrase secrète Wi-Fi ni le mot de passe du courtier. Rien sur cet appareil ne demande qui vous êtes : n’importe quoi sur le réseau peut donc réclamer ce fichier, et un export contenant des secrets reviendrait à pouvoir lire la phrase secrète de l’appareil depuis le réseau local. Ce qu’il conserve en revanche, c’est le nom du réseau et l’adresse du courtier — ainsi la personne qui restaure sur une carte neuve sait exactement quelles deux valeurs ressaisir, au lieu de devoir deviner sur quel réseau se trouvait l’ancienne.',
  'backup.exportWhen':
    'Reprenez-en une après avoir ajouté ou réappairé un volet. Les codes tournants avancent à chaque commande, et un fichier du mois dernier installe les codes du mois dernier sur une carte qui n’en a aucun — ce qu’un moteur rejette comme un rejeu.',
  'backup.export': 'Télécharger la sauvegarde',

  'backup.importTitle': 'Restaurer depuis un fichier',
  'backup.importWhat':
    'Deux types de fichiers sont acceptés : le .rtsb que cet appareil produit, et le .backup qu’exporte un contrôleur ESPSomfy-RTS. Le second est la façon dont une installation existante est reprise sans rien réappairer.',
  'backup.importStaged':
    'L’envoi ne restaure rien sur-le-champ. L’appareil met le fichier de côté, redémarre, et le lit en revenant — cette page perd donc sa connexion quelques secondes, et la réponse arrive ensuite. Tant que ce démarrage ne l’a pas lu, rien n’a été vérifié et rien n’a été écrit.',
  'backup.importCodes':
    'Une restauration ne peut pas faire reculer un code tournant. L’appareil n’installe un code que pour une adresse qui n’en a aucun : restaurer une vieille sauvegarde sur la carte dont elle provient ne change donc aucun code, et peut être tenté sans risque. C’est la restauration sur une carte neuve qui les installe, et c’est à cela qu’une sauvegarde sert.',
  'backup.importWhole':
    'Un fichier est appliqué en entier ou pas du tout. Un seul enregistrement que l’appareil refuse fait refuser l’ensemble, et la carte conserve la configuration qu’elle avait déjà.',
  'backup.file': 'Fichier de sauvegarde',
  'backup.anyFile': 'Afficher tous les fichiers, pas seulement .rtsb et .backup',
  'backup.fileHint':
    'L’extension ne fait que déterminer ce que le sélecteur propose : c’est en regardant les premiers octets que l’appareil tranche, et il refuse avant de le ranger tout fichier qu’il ne reconnaît pas. Désactivez le filtre si le fichier voulu a été renommé.',
  'backup.chosen': '{name} — {bytes} octets',
  'backup.upload': 'Envoyer et redémarrer',
  'backup.uploading': 'Envoi…',
  'backup.uploadRefused':
    'L’appareil a refusé le fichier : {reason}. Rien n’a été mis de côté et rien n’a changé.',
  'backup.waiting':
    'Accepté. L’appareil redémarre pour lire le fichier — il sera injoignable quelques secondes, et cette page dira ce qui s’est passé dès qu’il répondra de nouveau.',
  'backup.lost':
    'L’appareil n’a pas répondu depuis un moment. Il démarre peut-être encore, ou il est revenu sur une autre adresse. Rouvrez cette page dès que vous pourrez le joindre : le fichier est mis de côté, et le démarrage qui le lira consignera ce qu’il en a fait.',
  'backup.checkAgain': 'Vérifier de nouveau',

  'backup.reportTitle': 'La dernière restauration',
  'backup.outcomeNoneTitle': 'Aucune restauration',
  'backup.outcomeNone':
    'Aucune sauvegarde n’a jamais été envoyée à cet appareil. Tout ce qu’il contient a été configuré ici.',
  'backup.outcomeStagedTitle': 'Une sauvegarde attend d’être appliquée',
  'backup.outcomeStaged':
    'Un fichier est mis de côté et sera lu au prochain démarrage. Rien n’a encore été vérifié, et ce qui tourne en ce moment est la configuration que l’appareil avait déjà.',
  'backup.outcomeAppliedTitle': 'Restauré',
  'backup.appliedShades': 'Volets écrits',
  'backup.appliedRooms': 'Pièces écrites',
  'backup.appliedGroups': 'Groupes écrits',
  'backup.outcomeRefusedTitle': 'La sauvegarde a été refusée',
  'backup.refusedWhy': 'L’appareil l’a refusée : {reason}.',
  'backup.refusedNothing':
    'Rien n’a été écrit. L’appareil fonctionne avec la configuration qu’il avait avant l’envoi.',
  'backup.refusedRow':
    'Le refus vient de l’enregistrement {row} du fichier, les volets étant comptés à partir de zéro.',
  'backup.refusedFile':
    'Le refus porte sur le fichier lui-même, et non sur l’un de ses enregistrements.',

  'backup.format': 'Lu comme {format}.',
  'backup.formatSomfyRs': 'une sauvegarde issue d’un appareil somfy-rs',
  'backup.formatEspSomfyRts': 'une sauvegarde issue d’un contrôleur ESPSomfy-RTS',

  'backup.warningsNone': 'Chaque enregistrement a été repris exactement tel quel.',
  'backup.warningsOne':
    'Un enregistrement a été accepté sous réserve — un type de volet inconnu lu comme un enrouleur, un groupe dont l’ancien contrôleur n’a jamais écrit le code tournant, un membre désignant un volet qui n’existe plus. C’est une ligne du journal, avec l’enregistrement et la raison.',
  'backup.warnings':
    '{warnings} enregistrements ont été acceptés sous réserve — un type de volet inconnu lu comme un enrouleur, un groupe dont l’ancien contrôleur n’a jamais écrit le code tournant, un membre désignant un volet qui n’existe plus. Chacun est une ligne du journal, avec l’enregistrement et la raison.',
  'backup.warningsLink': 'Consulter le journal',

  'backup.retypeTitle': 'Ce qu’il vous reste à ressaisir',
  'backup.retypeWhy':
    'Une sauvegarde ne transporte aucun secret : voici donc les valeurs que la restauration n’a pas pu remettre en place à votre place.',
  'backup.retypeSsid':
    'L’appareil dont provient ce fichier était sur {ssid}. Ressaisissez la phrase secrète de ce réseau dans les réglages.',
  'backup.retypeSsidOpen':
    'L’appareil dont provient ce fichier était sur {ssid}, un réseau ouvert — il n’y a aucune phrase secrète à ressaisir.',
  'backup.retypeNoSsid':
    'Il n’avait aucun réseau enregistré : rien à ressaisir du côté Wi-Fi.',
  'backup.retypeBroker':
    'Il publiait vers le courtier {broker}. Ressaisissez le mot de passe de ce courtier dans les réglages.',
  'backup.retypeBrokerOpen':
    'Il publiait vers le courtier {broker}, qui ne demandait aucun mot de passe.',
  'backup.retypeNoBroker': 'Il n’avait aucun courtier configuré.',
  'backup.retypeUnknown':
    'Une sauvegarde issue d’un contrôleur ESPSomfy-RTS conserve ses identifiants réseau hors du fichier : elle ne dit donc rien du réseau ni du courtier qu’utilisait cet appareil. Consultez son propre écran de réglages tant qu’il fonctionne encore, si c’est possible.',
  'backup.retypeLink': 'Ouvrir les réglages',

  'stub.heading': '{screen}',
  'stub.body': 'Cet écran n’est pas encore implémenté.',
  'stub.settings': 'Réglages',
  'stub.backup': 'Sauvegarde et restauration',
  'stub.onboarding': 'Configuration',

  'route.notFound': 'Cette page n’existe pas.',
};
