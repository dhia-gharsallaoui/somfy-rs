/**
 * `ShadeDto.kind` and `ShadeDto.tiltMode` are raw numeric discriminants — the
 * values deployed devices already emit — so the UI needs the one mapping from
 * those bytes to labels. Kept next to the components that render them, and
 * total: an unrecognised byte falls back to the generic "Shade" rather than
 * rendering a number at the user.
 *
 * The **create form's options are derived from these maps** rather than listed
 * again, so a type that can be displayed can be chosen and vice versa. The
 * authority on both sets remains Rust (`ShadeKind::from_raw`,
 * `TiltMode::from_raw`), which refuses anything else on the way in; a
 * discriminant added there but not here is a picker missing an entry, not a
 * request the device would mis-handle.
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

const TILT_LABELS: Record<number, MessageKey> = {
  0x00: 'tilt.none',
  0x01: 'tilt.motor',
  0x02: 'tilt.integrated',
  0x03: 'tilt.tiltOnly',
  0x04: 'tilt.euro',
};

export const kindKey = (kind: number): MessageKey => KIND_LABELS[kind] ?? 'kind.unknown';

export const tiltKey = (tiltMode: number): MessageKey => TILT_LABELS[tiltMode] ?? 'tilt.none';

/** One selectable option: the discriminant that goes on the wire, and its label. */
export interface DiscriminantOption {
  value: number;
  label: MessageKey;
}

const options = (labels: Record<number, MessageKey>): DiscriminantOption[] =>
  Object.entries(labels)
    .map(([value, label]) => ({ value: Number(value), label }))
    .sort((a, b) => a.value - b.value);

export const KIND_OPTIONS: DiscriminantOption[] = options(KIND_LABELS);
export const TILT_OPTIONS: DiscriminantOption[] = options(TILT_LABELS);

/** `TiltMode::None` — the tilt-time field is meaningless without a tilt axis. */
export const TILT_NONE = 0x00;
