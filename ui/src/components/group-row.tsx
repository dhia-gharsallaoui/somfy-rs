/**
 * A group's three commands. Group commands are per-shade fan-out in v1.0 — the
 * device applies the command to each member rather than transmitting a group
 * frame — so this is a convenience over the same `CommandDto`, not a different
 * kind of request.
 */
import { commandGroup } from '../api/client';
import type { GroupDto } from '../api/generated/GroupDto';
import { useT } from '../i18n';

export function GroupRow({ group }: { group: GroupDto }) {
  const t = useT();
  const send = (action: 'up' | 'my' | 'down') => void commandGroup(group.id, { action });

  return (
    <div class="group">
      <div class="group__label">
        <h3 class="group__name">{group.name}</h3>
        <p class="group__count">{t('dashboard.groupCount', { count: group.shadeIds.length })}</p>
      </div>
      <div class="group__buttons" role="group" aria-label={group.name}>
        <button type="button" class="btn btn--ghost" onClick={() => send('up')}>
          {t('command.up')}
        </button>
        <button type="button" class="btn btn--ghost" onClick={() => send('my')}>
          {t('command.my')}
        </button>
        <button type="button" class="btn btn--ghost" onClick={() => send('down')}>
          {t('command.down')}
        </button>
      </div>
    </div>
  );
}
