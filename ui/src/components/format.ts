/**
 * Display formatting shared by more than one screen.
 *
 * The address format is deliberately the one every other tool in this project
 * prints — `0x8ACE01`, upper-case, six digits — so a value read off this UI can
 * be compared with a serial log or a provisioning run without anyone having to
 * re-base it in their head.
 */

/** A 24-bit remote address, as the rest of the project writes it. */
export const formatAddress = (address: number): string =>
  `0x${address.toString(16).toUpperCase().padStart(6, '0')}`;

/** Whole seconds and tenths, for a travel time held in milliseconds. */
export const seconds = (ms: number): string => (ms / 1000).toFixed(1);

/** `m:ss`, for the pairing window's countdown. */
export function clock(totalSeconds: number): string {
  const safe = Math.max(0, Math.round(totalSeconds));
  const minutes = Math.floor(safe / 60);
  return `${minutes}:${String(safe % 60).padStart(2, '0')}`;
}
