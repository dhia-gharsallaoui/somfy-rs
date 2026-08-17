/**
 * `ShadeDto.kind` is a raw numeric discriminant — the value deployed devices
 * already emit — so the UI needs the one mapping from that byte to a label.
 * Kept next to the components that render it, and total: an unrecognised byte
 * falls back to the generic "Shade" rather than rendering a number at the user.
 */
import type { MessageKey } from '../i18n/en';

const KIND_LABELS: Record<number, MessageKey> = {
  0x00: 'kind.roller',
  0x01: 'kind.blind',
  0x02: 'kind.draperyLeft',
  0x03: 'kind.awning',
  0x04: 'kind.shutter',
  0x07: 'kind.draperyRight',
  0x08: 'kind.draperyCenter',
};

export const kindKey = (kind: number): MessageKey => KIND_LABELS[kind] ?? 'kind.unknown';
