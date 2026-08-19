/**
 * Diagnostics: what the device can say about its own past.
 *
 * ## What this screen is for
 *
 * Every hard failure this project has had was diagnosed over a serial cable,
 * and the person holding the device does not have one. So the job here is not
 * to look like a dashboard; it is to turn "it stopped working" into a sentence
 * somebody can act on, or paste into an issue. Three consequences follow, and
 * they are most of the layout:
 *
 * 1. **The panic comes first and is allowed to shout.** It is the one thing on
 *    the page that is not a statistic — it is the device saying it fell over,
 *    with the line of source that did it. Everything else is context for it.
 * 2. **Every number is shown next to the claim it tests.** `used` beside
 *    `required`, `peak` beside `size`, `bytes` beside `capacity`. A lone figure
 *    is unreadable; a pair is an argument. This mirrors the boot line the
 *    serial console prints, which is the model for a good line here.
 * 3. **Copying is a first-class action.** What a person pastes into an issue is
 *    the log *plus* who they are — firmware, chip, uptime, reset reason — and
 *    asking them to assemble that themselves is asking for a report with the
 *    interesting half missing.
 *
 * ## Why there is no auto-refresh
 *
 * A timer here would fight the reader. The log is a scrollable block somebody
 * selects text inside, and redrawing it every few seconds moves the ground
 * under them; worse, it would make "copy" mean something different from what
 * was on screen when they pressed it. The one figure that changes on its own is
 * the uptime, and nobody reads an uptime to the second. So refreshing is
 * explicit, and pressing it re-reads both halves together — which also keeps
 * `log.bytes` and the text it describes from being read a minute apart.
 *
 * There is a device-side reason too: `picoserve` costs about 13 KB of DRAM per
 * connection task on this part, paid out of the same heap the Wi-Fi driver
 * needs, so a page nobody is looking at should not be holding one open.
 */
import { useEffect, useState } from 'preact/hooks';

import { forgetSystem, getSystem, getSystemLog } from '../api/client';
import { ApiError, errorMessageKey } from '../api/errors';
import type { ChipDto } from '../api/generated/ChipDto';
import type { ResetReasonDto } from '../api/generated/ResetReasonDto';
import type { SystemDto } from '../api/generated/SystemDto';
import { useT, type Translate } from '../i18n';
import type { MessageKey } from '../i18n/en';

/**
 * What each reset reason means, and what to do about it.
 *
 * Total over the generated {@link ResetReasonDto}, so a seventh cause added in
 * Rust fails `tsc` here rather than rendering as a blank. The `note` is the
 * half that earns its place: `ResetReasonDto`'s own documentation says the enum
 * is coarse precisely because "every one of those six has an action behind it",
 * and a bare label — "Brownout" — leaves that action unsaid to the one person
 * who needs it.
 */
const RESET_TEXT: Record<ResetReasonDto, { label: MessageKey; note: MessageKey }> = {
  powerOn: { label: 'diag.resetPowerOn', note: 'diag.resetPowerOnNote' },
  software: { label: 'diag.resetSoftware', note: 'diag.resetSoftwareNote' },
  watchdog: { label: 'diag.resetWatchdog', note: 'diag.resetWatchdogNote' },
  brownout: { label: 'diag.resetBrownout', note: 'diag.resetBrownoutNote' },
  debugger: { label: 'diag.resetDebugger', note: 'diag.resetDebuggerNote' },
  other: { label: 'diag.resetOther', note: 'diag.resetOtherNote' },
};

/**
 * The part names, and deliberately **not** translated.
 *
 * These are product names printed on the silicon; "ESP32-S3" is ESP32-S3 in
 * every language, and putting them through the catalogue would invite somebody
 * to translate one. It is still a total `Record<ChipDto, …>`, so the drift gate
 * is intact: a second part added in Rust fails `tsc` here.
 */
const CHIP_NAME: Record<ChipDto, string> = {
  esp32S3: 'ESP32-S3',
};

/** Which reset causes are faults rather than routine. */
const ALARMING: ReadonlySet<ResetReasonDto> = new Set<ResetReasonDto>(['watchdog', 'brownout']);

/** The device's self-report, or why there is not one. */
type Loaded =
  | { at: 'loading' }
  | { at: 'ready'; system: SystemDto }
  | { at: 'unreachable'; detail: string };

/**
 * The log, held separately from the summary above.
 *
 * Two requests, so two ways to fail: the summary is small JSON and the log is a
 * chunked plain-text stream of everything the device has said. A single state
 * would mean a log that timed out took the panic report down with it, and the
 * panic report is the more important of the two.
 */
type LogState =
  | { at: 'loading' }
  | { at: 'ready'; text: string }
  | { at: 'failed'; detail: string };

export function Diagnostics() {
  const t = useT();
  const [loaded, setLoaded] = useState<Loaded>({ at: 'loading' });
  const [log, setLog] = useState<LogState>({ at: 'loading' });
  const [reloadToken, setReloadToken] = useState(0);
  const [forgotten, setForgotten] = useState(false);

  const reload = () => {
    setForgotten(false);
    setReloadToken((token) => token + 1);
  };

  useEffect(() => {
    let cancelled = false;
    setLoaded((previous) => (previous.at === 'ready' ? previous : { at: 'loading' }));
    getSystem()
      .then((system) => {
        if (!cancelled) setLoaded({ at: 'ready', system });
      })
      .catch((cause: unknown) => {
        if (!cancelled) setLoaded({ at: 'unreachable', detail: detailOf(cause) });
      });
    getSystemLog()
      .then((text) => {
        if (!cancelled) setLog({ at: 'ready', text });
      })
      .catch((cause: unknown) => {
        if (!cancelled) setLog({ at: 'failed', detail: detailOf(cause) });
      });
    return () => {
      cancelled = true;
    };
  }, [reloadToken]);

  const busy = loaded.at === 'loading' || log.at === 'loading';

  if (loaded.at === 'loading') {
    return (
      <div class="detail">
        <Head t={t} busy={busy} onRefresh={reload} />
        <section class="panel">
          <p>{t('diag.loading')}</p>
        </section>
      </div>
    );
  }

  if (loaded.at === 'unreachable') {
    return (
      <div class="detail">
        <Head t={t} busy={busy} onRefresh={reload} />
        <section class="panel panel--error" role="alert">
          <p>{t('diag.unreachable', { detail: loaded.detail })}</p>
          <button type="button" class="btn" onClick={reload}>
            {t('diag.retry')}
          </button>
        </section>
      </div>
    );
  }

  const { system } = loaded;
  const logText = log.at === 'ready' ? log.text : '';

  return (
    <div class="detail">
      <Head t={t} busy={busy} onRefresh={reload} />
      <p class="prose">{t('diag.intro')}</p>

      {forgotten && (
        <p class="note" role="status">
          {t('diag.forgetDone')}
        </p>
      )}

      {/*
        The panic outranks everything, including identity: somebody who opened
        this screen because the device misbehaved should not have to scroll past
        a firmware version to find out that it crashed.
      */}
      <PanicPanel system={system} t={t} />
      <IdentityPanel system={system} t={t} />
      <MemoryPanel system={system} t={t} />
      <LogPanel system={system} log={log} report={() => report(system, logText)} t={t} />

      <ForgetPanel
        onForgotten={() => {
          setForgotten(true);
          setReloadToken((token) => token + 1);
        }}
        t={t}
      />
    </div>
  );
}

/**
 * Title bar, with refresh beside the title.
 *
 * Refresh belongs here rather than at the foot of one panel because it re-reads
 * the *page* — the summary and the log together, so that `log.bytes` and the
 * text it describes are never a minute apart. A button sitting under the log
 * would have read as belonging to the log alone.
 */
function Head({ t, busy, onRefresh }: { t: Translate; busy: boolean; onRefresh: () => void }) {
  return (
    <>
      <nav class="detail__nav">
        <a class="link" href="/">
          ← {t('detail.back')}
        </a>
      </nav>
      <header class="detail__head detail__head--action">
        <h2>{t('diag.title')}</h2>
        <button type="button" class="btn" disabled={busy} onClick={onRefresh}>
          {busy ? t('diag.refreshing') : t('diag.refresh')}
        </button>
      </header>
    </>
  );
}

// ---------------------------------------------------------------------------
// The panic
// ---------------------------------------------------------------------------

function PanicPanel({ system, t }: { system: SystemDto; t: Translate }) {
  const panic = system.lastPanic;

  if (!panic) {
    return (
      <section class="panel">
        <h3>{t('diag.panicNoneTitle')}</h3>
        {/*
          The absence is qualified rather than celebrated. `PanicDto`'s record
          lives in RTC memory, which a power cut zeroes — so "no panic" and "no
          panic *that survived*" are the same reading, and a screen that said
          only the first would be over-claiming.
        */}
        <p class="prose">{t('diag.panicNone')}</p>
      </section>
    );
  }

  return (
    <section class="panel panel--error panel--panic" role="alert">
      <h3>{t('diag.panicTitle')}</h3>

      {/*
        `bootsSince === 0` is not a count, it is a statement: the device
        restarted itself and this page is being served by the boot that restart
        produced. Rendering it as "0 restarts ago" would bury the one fact that
        tells the reader the fault is happening now.
      */}
      <p class="outcome__sent">
        {panic.bootsSince === 0
          ? t('diag.panicThisBoot')
          : panic.bootsSince === 1
            ? t('diag.panicBootsSinceOne')
            : t('diag.panicBootsSince', { boots: panic.bootsSince })}
      </p>

      <p>{t('diag.panicWhen', { uptime: duration(panic.uptimeS, t) })}</p>

      {/*
        Two facts together — it panicked seconds in, and this boot is the one
        that panic caused — are the *shape* of a boot loop, so the message says
        that and tells the reader how to confirm it, rather than asserting a
        loop from a single sample. The threshold is a minute: past that the
        device reached a steady state before it fell over, which is a different
        problem with a different cause.
      */}
      {panic.bootsSince === 0 && panic.uptimeS < 60 && <p class="note note--warn">{t('diag.panicLoop')}</p>}

      <h4 class="field__label">{t('diag.panicWhat')}</h4>
      <pre class="log log--panic">{panic.text}</pre>

      {panic.truncated && <p class="note note--warn">{t('diag.panicTruncated')}</p>}
      <p class="note">{t('diag.panicVolatile')}</p>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

function IdentityPanel({ system, t }: { system: SystemDto; t: Translate }) {
  const reset = RESET_TEXT[system.resetReason];
  return (
    <section class="panel">
      <h3>{t('diag.identityTitle')}</h3>
      <dl class="facts">
        <dt>{t('diag.firmware')}</dt>
        <dd class="mono">{system.firmware}</dd>
        <dt>{t('diag.chip')}</dt>
        <dd>{CHIP_NAME[system.chip]}</dd>
        <dt>{t('diag.host')}</dt>
        <dd class="mono">{system.host}</dd>
        <dt>{t('diag.uptime')}</dt>
        <dd>{duration(system.uptimeS, t)}</dd>
        <dt>{t('diag.resetReason')}</dt>
        <dd>{t(reset.label)}</dd>
      </dl>
      <p class={ALARMING.has(system.resetReason) ? 'note note--warn' : 'prose'}>{t(reset.note)}</p>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

function MemoryPanel({ system, t }: { system: SystemDto; t: Translate }) {
  const { stack, heap } = system;
  // Saturating, exactly as `report_stack_use` computes it — a build whose
  // requirement has gone stale must not produce a negative "unspent" here, it
  // must produce a zero and the warning underneath.
  const unspent = Math.max(0, stack.required - (stack.used ?? stack.required));
  const stale = stack.used !== null && stack.used > stack.required;

  return (
    <section class="panel">
      <h3>{t('diag.memoryTitle')}</h3>

      <h4 class="field__label">{t('diag.stackTitle')}</h4>
      {stack.used === null ? (
        <p>{t('diag.stackUnmeasured')}</p>
      ) : (
        <p class="mono">
          {t('diag.stackLine', {
            used: group(stack.used),
            required: group(stack.required),
            unspent: group(unspent),
          })}
        </p>
      )}
      <p class="mono">{t('diag.stackAvailable', { available: group(stack.available) })}</p>
      {stale && (
        <p class="note note--warn" role="alert">
          {t('diag.stackStale')}
        </p>
      )}
      {/*
        Only shown once there is a measurement, because the sentence is about
        one: "only `used` was measured" is false on a boot where nothing has
        been, and the paragraph's whole argument — the gap between the first two
        figures — has no gap to point at.
      */}
      {stack.used !== null && <p class="prose">{t('diag.stackWhy')}</p>}

      <h4 class="field__label">{t('diag.heapTitle')}</h4>
      <p class="mono">{t('diag.heapPeak', { peak: group(heap.peak), size: group(heap.size) })}</p>
      <p class="mono">{t('diag.heapUsed', { used: group(heap.used) })}</p>
      <p class="prose">{t('diag.heapWhy')}</p>
    </section>
  );
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

function LogPanel({
  system,
  log,
  report: buildReport,
  t,
}: {
  system: SystemDto;
  log: LogState;
  report: () => string;
  t: Translate;
}) {
  const [copied, setCopied] = useState<'yes' | 'no' | undefined>(undefined);
  const ring = system.log;

  // "Copied" is a claim about bytes that are no longer on screen once the log
  // is re-read — after a refresh, and especially after a Forget, which empties
  // it. Leaving the note up would tell somebody their clipboard holds what they
  // are looking at, which by then is the one thing it does not.
  useEffect(() => setCopied(undefined), [log]);

  const copy = () => {
    void writeClipboard(buildReport()).then((ok) => setCopied(ok ? 'yes' : 'no'));
  };

  return (
    <section class="panel">
      <h3>{t('diag.logTitle')}</h3>
      <p class="mono">
        {t('diag.logRing', {
          bytes: group(ring.bytes),
          capacity: group(ring.capacity),
          lines: group(ring.lines),
        })}
      </p>

      {/*
        `dropped` is called out rather than listed with the others. A non-zero
        value means the oldest output is gone, and the oldest output is the boot
        — which is the part somebody diagnosing a fault most wants. It is also
        the only figure on this screen that is a message to *us*: it says the
        ring is too small for what this firmware prints.
      */}
      {ring.dropped > 0 ? (
        <p class="note note--warn" role="status">
          {t('diag.logDropped', { dropped: group(ring.dropped) })}
        </p>
      ) : (
        <p class="note">{t('diag.logIntact')}</p>
      )}

      {log.at === 'loading' && <p>{t('diag.logLoading')}</p>}
      {log.at === 'failed' && (
        <p class="note note--warn" role="alert">
          {t('diag.logFailed', { detail: log.detail })}
        </p>
      )}
      {log.at === 'ready' &&
        (log.text.length === 0 ? (
          <p>{t('diag.logEmpty')}</p>
        ) : (
          <pre class="log">{log.text}</pre>
        ))}

      <div class="actions">
        <button type="button" class="btn" disabled={log.at !== 'ready'} onClick={copy}>
          {t('diag.logCopy')}
        </button>
      </div>
      {copied === 'yes' && (
        <p class="note" role="status">
          {t('diag.logCopied')}
        </p>
      )}
      {copied === 'no' && (
        <p class="note note--warn" role="status">
          {t('diag.logCopyFailed')}
        </p>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Forget
// ---------------------------------------------------------------------------

/**
 * Clearing the panic record and the log.
 *
 * Two clicks with the warning between them, the same idiom as
 * `delete-shade.tsx` and `settings.tsx`'s broker removal, and for the same
 * reason: `confirm()` cannot show the warning, and the warning is the point.
 * What makes this one worth the ceremony is that there is no copy anywhere —
 * the panic lives in RTC memory and the ring lives in RAM, so neither a reboot
 * nor a backup brings them back.
 */
function ForgetPanel({ onForgotten, t }: { onForgotten: () => void; t: Translate }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const forget = () => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    forgetSystem()
      .then(() => {
        setConfirming(false);
        onForgotten();
      })
      .catch((cause: unknown) => setFailure(t(errorMessageKey(cause))))
      .finally(() => setBusy(false));
  };

  return (
    <section class="panel panel--danger">
      <h3>{t('diag.forgetTitle')}</h3>

      {failure !== undefined && (
        <p class="note note--warn" role="alert">
          {t('diag.forgetFailed', { reason: failure })}
        </p>
      )}

      {confirming ? (
        <>
          <p class="prose">{t('diag.forgetWarning')}</p>
          <div class="actions">
            <button type="button" class="btn btn--danger" disabled={busy} onClick={forget}>
              {busy ? t('diag.forgetting') : t('diag.forgetConfirm')}
            </button>
            <button
              type="button"
              class="btn btn--ghost"
              disabled={busy}
              onClick={() => setConfirming(false)}
            >
              {t('diag.forgetCancel')}
            </button>
          </div>
        </>
      ) : (
        <div class="actions">
          <button type="button" class="btn" onClick={() => setConfirming(true)}>
            {t('diag.forget')}
          </button>
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Formatting, and the report
// ---------------------------------------------------------------------------

/**
 * An uptime, in units a person reads.
 *
 * Raw seconds are unreadable past a few minutes — `276220` is a number nobody
 * converts in their head, and the difference between a device that has been up
 * three days and one that has been up three minutes is the whole diagnosis. The
 * coarsest two units are enough: nobody troubleshooting cares that it is three
 * days, four hours and eleven minutes.
 *
 * The one plural in it — "1 day" against "3 days" — is two catalogue keys
 * rather than a rule. `i18n/index.tsx` records the ruling: reach for
 * `Intl.PluralRules` when a message needs a *real* plural rule, and English and
 * French agree on this one, so a second key is cheaper than either.
 */
function duration(totalSeconds: number, t: Translate): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);

  if (days === 1) return t('diag.durationDay', { hours });
  if (days > 1) return t('diag.durationDays', { days, hours });
  if (hours > 0) return t('diag.durationHours', { hours, minutes });
  if (minutes > 0) return t('diag.durationMinutes', { minutes });
  return t('diag.durationSeconds', { seconds });
}

/**
 * A byte count with thin spaces every three digits.
 *
 * Grouped because these numbers are read *against each other* — 54,064 against
 * 55,792 — and ungrouped five-digit runs make that comparison a character-by-
 * character exercise. `Intl.NumberFormat` would have done it, but it groups by
 * the *browser's* locale rather than the one this app is displaying, so a French
 * page in an English browser would have printed commas among its French. The
 * separator here is U+2009 THIN SPACE, which both conventions accept.
 */
function group(value: number): string {
  return String(value).replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
}

/**
 * Everything on this page, as text.
 *
 * **Deliberately not translated, and that is a decision rather than an
 * oversight.** This string is not read on screen; it is pasted into an issue,
 * a chat message or an email to somebody who is going to look at the firmware.
 * Its field names are the DTO's field names, so a maintainer can match each
 * line to `SystemDto` without a dictionary, and a French bug report that a
 * maintainer cannot read helps nobody — least of all the person who filed it.
 * Everything the *user* reads about this action does go through the catalogue.
 *
 * The identity block leads because a log without it is unattributable: which
 * chip, which firmware, how long up, and why it started are the four questions
 * every reply to a bug report otherwise begins with.
 */
function report(system: SystemDto, log: string): string {
  const { stack, heap, log: ring, lastPanic } = system;
  const lines = [
    `somfy-rs ${system.firmware} on ${CHIP_NAME[system.chip]}`,
    `host: ${system.host}`,
    `uptime: ${system.uptimeS} s`,
    `reset: ${system.resetReason}`,
    `stack: ${stack.used ?? 'unmeasured'} used / ${stack.required} required / ${stack.available} available`,
    `heap: ${heap.peak} peak / ${heap.used} used / ${heap.size} total`,
    `log: ${ring.bytes} of ${ring.capacity} bytes, ${ring.lines} lines, ${ring.dropped} dropped`,
  ];
  if (lastPanic) {
    lines.push(
      `panic: ${lastPanic.bootsSince} boots ago, at ${lastPanic.uptimeS} s uptime` +
        (lastPanic.truncated ? ', truncated' : ''),
      lastPanic.text,
    );
  } else {
    lines.push('panic: none recorded');
  }
  lines.push('--- log ---', log);
  return lines.join('\n');
}

/**
 * Put text on the clipboard, or report that it could not be done.
 *
 * **`navigator.clipboard` cannot be assumed to exist here.** This app is served
 * by the device over plain HTTP on a LAN address, which is not a secure context
 * in most browsers, and in a non-secure context the whole `clipboard` object is
 * simply absent — not a rejected promise, an `undefined` property, so reaching
 * for `.writeText` on it throws. That is the ordinary case for this device, not
 * an edge one.
 *
 * The fallback is the pre-`Clipboard`-API method: a textarea holding the text,
 * selected, and `document.execCommand('copy')`. It is deprecated and it still
 * works in every browser that withholds the modern API, which is exactly the
 * set of browsers that need it. Both can fail — a user gesture requirement, a
 * permission policy — so the caller is told, and the screen's answer is "select
 * the text and copy it by hand", which always works.
 */
async function writeClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall through: a rejection here (permissions policy, no user gesture
    // recognised) is not a reason to give up while the older path is untried.
  }

  const area = document.createElement('textarea');
  area.value = text;
  // Off-screen rather than `display: none`: an unrendered textarea cannot hold
  // a selection, and the selection is what `execCommand` copies. `readOnly`
  // keeps a mobile keyboard from appearing for the fraction of a second it is
  // focused.
  area.setAttribute('readonly', '');
  area.style.position = 'fixed';
  area.style.opacity = '0';
  area.style.insetBlockStart = '0';
  document.body.appendChild(area);
  try {
    area.select();
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    area.remove();
  }
}

/** Whatever the device or the network said, as something printable. */
function detailOf(cause: unknown): string {
  return cause instanceof ApiError ? cause.message : String(cause);
}
