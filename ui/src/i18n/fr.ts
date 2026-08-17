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

  'shade.open': 'Ouvert',
  'shade.closed': 'Fermé',
  'shade.openPercent': 'ouvert à {percent} %',
  'shade.opening': 'Ouverture',
  'shade.closing': 'Fermeture',
  'shade.idle': 'Arrêté',
  'shade.favourite': 'Position favorite : ouvert à {percent} %',
  'shade.noFavourite': 'Aucune position favorite',

  'command.up': 'Ouvrir',
  'command.my': 'Favori',
  'command.down': 'Fermer',
  'command.upAria': 'Ouvrir {name}',
  'command.myAria': 'Mettre {name} en position favorite, ou l’arrêter',
  'command.downAria': 'Fermer {name}',
  'command.sliderAria': 'Ouverture de {name}, en pourcentage',

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
  'calib.autoPending':
    'La mesure automatique — l’appareil fait parcourir le volet et le chronomètre — n’est pas encore implémentée. En attendant, les valeurs mesurées à la main sont les valeurs justes.',

  'detail.linkedRemotes': 'Télécommandes associées',
  'detail.linkedRemotesPending': 'Pas encore implémenté.',
  'detail.origin': 'Origine de cette adresse',
  'detail.originAllocated': 'Attribuée par ce contrôleur',
  'detail.originImported': 'Importée d’un autre contrôleur',
  'detail.originAllocatedNote':
    'Aucun autre contrôleur n’utilise cette adresse. Un moteur ne lui obéit qu’une fois l’appairage effectué.',
  'detail.originImportedNote':
    'Cette adresse appartient au contrôleur dont elle a été importée. Si celui-ci fonctionne encore, les deux forment désormais une seule télécommande avec deux compteurs distincts, et le premier à prendre du retard cessera d’être obéi. L’appairage n’y change rien : il réapprendrait au moteur cette même adresse partagée.',
  'detail.pair': 'Appairer ce volet',
  'detail.dangerZone': 'Supprimer',
  'detail.delete': 'Supprimer {name}',
  'detail.deleteWarning':
    'Ceci retire {name} de ce contrôleur uniquement. Le moteur n’en est pas informé, et ne peut pas l’être : il continue d’obéir à toutes les télécommandes qu’il a apprises, y compris celle-ci. Il n’existe volontairement aucune commande de désappairage ici — sur une télécommande physique, désappairer se fait par un appui long sur PROG, et une salve à peine trop longue supprime une télécommande d’un volet qui fonctionnait.',
  'detail.deleteConfirm': 'Oui, supprimer {name}',
  'detail.deleteCancel': 'Conserver',
  'detail.deleting': 'Suppression…',

  'add.title': 'Ajouter un volet',
  'add.intro':
    'Le contrôleur attribuera à ce volet sa propre adresse de télécommande. Aucun moteur ne connaît encore cette adresse : l’étape suivante consiste à l’apprendre à l’un d’eux, et le volet ne bougera pas avant.',
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

  'add.createdTitle': '{name} ajouté',
  'add.createdAddress': 'Adresse de télécommande {address}',
  'add.createdBody':
    'Cette adresse est nouvelle et aucun moteur ne l’a encore entendue : {name} ne bougera pas. L’étape suivante est d’apprendre à un moteur à y répondre, et elle demande votre présence au volet avec une télécommande qui fonctionne déjà.',
  'add.createdPair': 'Appairer maintenant',
  'add.createdLater': 'Appairer plus tard',

  'pair.title': 'Appairage de {name}',
  'pair.progress': 'Étape {step} sur {total}',
  'pair.additive':
    'Rien n’est supprimé. L’appairage ajoute ce contrôleur au moteur ; toutes les télécommandes qui fonctionnent aujourd’hui continueront de fonctionner.',

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
  'pair.step3Sent': 'Envoyé. Regardez le volet maintenant.',
  'pair.step3NoFeedback':
    'Rien de plus n’apparaîtra ici. Le contrôleur émet et ne reçoit jamais de réponse : il ne peut pas vous dire si le moteur a accepté le signal — vous seul pouvez le constater.',
  'pair.step3Question': 'Le volet a-t-il bougé ?',
  'pair.step3Yes': 'Oui, il a bougé',
  'pair.step3No': 'Non, rien ne s’est passé',
  'pair.step3Failed': 'L’appareil n’a pas voulu l’envoyer : {reason}',

  'pair.doneTitle': '{name} est appairé',
  'pair.doneWitness':
    'C’est vous qui avez vu la confirmation ; le contrôleur ne l’a pas vue et ne le pouvait pas. Ce second va-et-vient est le moteur confirmant qu’il a appris ce contrôleur.',
  'pair.doneEnd':
    'Le mode programmation se termine de lui-même après une ou deux minutes. Pour y mettre fin tout de suite, appuyez à nouveau sur PROG de la télécommande existante.',
  'pair.doneTest':
    'Testez maintenant : ouvrez et fermez {name}, et vérifiez qu’il obéit.',
  'pair.doneBack': 'Aller à {name}',

  'pair.retryTitle': 'Rien ne s’est passé',
  'pair.retryIntro': 'Dans cet ordre, en commençant par les vérifications les moins coûteuses.',
  'pair.retryWindow':
    'La fenêtre de programmation s’est refermée. Deux minutes, c’est confortable mais pas illimité. Remettez le moteur en mode programmation et renvoyez le signal — c’est de loin la cause la plus fréquente.',
  'pair.retryChannel':
    'La télécommande était sur un autre canal : un autre volet est passé en mode programmation, ou aucun.',
  'pair.retryRange':
    'Le signal n’a pas atteint le moteur. Rapprochez le contrôleur du volet, ou vérifiez son antenne.',
  'pair.retryAgain': 'Reprendre au mode programmation',
  'pair.retryStop': 'Arrêter pour l’instant',

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
  'error.unknown': 'l’appareil n’en a pas donné la raison',

  'stub.heading': '{screen}',
  'stub.body': 'Cet écran n’est pas encore implémenté.',
  'stub.settings': 'Réglages',
  'stub.backup': 'Sauvegarde et restauration',
  'stub.diagnostics': 'Diagnostics',
  'stub.onboarding': 'Configuration',

  'route.notFound': 'Cette page n’existe pas.',
};
