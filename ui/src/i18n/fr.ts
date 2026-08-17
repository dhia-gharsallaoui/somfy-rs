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
  'detail.calibration': 'Étalonnage des temps de course',
  'detail.calibrationPending':
    'L’étalonnage guidé mesure séparément les temps d’ouverture et de fermeture. Pas encore implémenté — voir les exigences de précision de position, R2.',
  'detail.linkedRemotes': 'Télécommandes associées',
  'detail.linkedRemotesPending': 'Pas encore implémenté.',

  'stub.heading': '{screen}',
  'stub.body': 'Cet écran n’est pas encore implémenté.',
  'stub.pairing': 'Assistant d’appairage',
  'stub.settings': 'Réglages',
  'stub.backup': 'Sauvegarde et restauration',
  'stub.diagnostics': 'Diagnostics',
  'stub.onboarding': 'Configuration',

  'route.notFound': 'Cette page n’existe pas.',
};
