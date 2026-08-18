/**
 * Backup and restore: the one screen whose value is measured in walks to a
 * window that did not have to happen.
 *
 * ## What the copy on this page is actually for
 *
 * Three hazards, and the layout exists to handle them rather than to look like
 * a settings pane.
 *
 * 1. **A backup's worth is the rolling codes.** Everything else in it — names,
 *    rooms, groups, travel times — can be retyped in a few minutes. A rolling
 *    code cannot: a motor only obeys a remote whose counter it recognises, so
 *    losing one costs a physical re-pairing at every motor. That sentence is at
 *    the top of the page because it is the reason to press the export button,
 *    and a person who never reads it will never press it.
 * 2. **An upload is staged, not applied.** `POST` answers `202` and the device
 *    restarts; the boot path is what reads, validates and applies the file.
 *    `RestoreReportDto` carries the arithmetic behind that. The failure mode to
 *    avoid is a screen that says "restored!" on the `202` and then loses its
 *    connection — so the button says "upload and restart", and what follows a
 *    `202` is a wait, not a claim.
 * 3. **A restore cannot move a code backwards.** `somfy_store::seed_if_absent`
 *    cannot express an overwrite at all, so restoring an old file onto the board
 *    it came from keeps every code the board already had. That is the fact that
 *    makes pressing the button safe, and it belongs next to the button rather
 *    than in a manual nobody has.
 *
 * ## Why the report moves
 *
 * When the outcome is anything but `none` the report is rendered **above** the
 * export and import panels, and below them when it is `none`. Same argument as
 * the panic on the diagnostics screen: somebody who opened this page because a
 * restore just happened should not have to scroll past two explanatory panels to
 * find out whether it worked. On a device that has never been restored, the
 * report is the least interesting thing here and goes last.
 *
 * ## Why the wait polls rather than listens
 *
 * The event stream would be the obvious place, but it carries `ShadeStateEvent`
 * and nothing else, and the device that would emit a restore event is the one
 * that is rebooting. So the wait is a poll with the same bounded backoff
 * `api/events.ts` uses for its socket, for the same reason: the device is
 * *expected* to be unreachable for a few seconds, and a failed request during
 * that window is the normal case rather than an error to report.
 */
import { useEffect, useState } from 'preact/hooks';

import { BACKUP_URL, getRestoreReport, uploadBackup } from '../api/client';
import { ApiError, ERROR_MESSAGE, errorMessageKey } from '../api/errors';
import type { BackupContentsDto } from '../api/generated/BackupContentsDto';
import type { BackupFormatDto } from '../api/generated/BackupFormatDto';
import type { RestoreOutcomeDto } from '../api/generated/RestoreOutcomeDto';
import type { RestoreReportDto } from '../api/generated/RestoreReportDto';
import { useT, type Translate } from '../i18n';
import type { MessageKey } from '../i18n/en';

/**
 * What the file picker offers by default.
 *
 * `.rtsb` is what this device writes and `.backup` is what an ESPSomfy-RTS
 * controller exports — the two formats `firmware::restore::recognisable`
 * accepts. Naming a firmware image's `.bin` here would be actively harmful: the
 * commonest mistake this endpoint exists to refuse is a firmware image uploaded
 * to the restore route, and a picker that greys it out is the cheapest place to
 * prevent it.
 */
const ACCEPT = '.rtsb,.backup';

/**
 * How the wait for a restarting device is paced.
 *
 * The same shape as `api/events.ts`'s reconnect — start small, double, stop
 * doubling — because it is the same situation: the device is expected to be
 * gone and then to come back. Two differences, both deliberate. The ceiling is
 * four seconds rather than ten, because a person is watching this one and a
 * ten-second ceiling means up to ten seconds of staring at a device that has
 * already answered. And there is a deadline at all, because a state record that
 * says `staged` and a device that never reads it would otherwise poll for ever;
 * ninety seconds is generous against the boot this device's own log describes
 * (boot, associate, DHCP lease) and short enough that a person is not left
 * watching a spinner.
 */
const POLL_MIN_MS = 500;
const POLL_MAX_MS = 4_000;
const POLL_DEADLINE_MS = 90_000;

/**
 * The headline for each outcome. Total over the generated
 * {@link RestoreOutcomeDto}, so a fifth outcome added in Rust fails `tsc` here
 * rather than rendering as a blank heading.
 */
const OUTCOME_TITLE: Record<RestoreOutcomeDto, MessageKey> = {
  none: 'backup.outcomeNoneTitle',
  staged: 'backup.outcomeStagedTitle',
  applied: 'backup.outcomeAppliedTitle',
  refused: 'backup.outcomeRefusedTitle',
};

/**
 * Which panel treatment each outcome gets.
 *
 * `applied` is the only good news on this screen and `refused` the only bad;
 * `staged` is neither — it is a statement that nothing has happened yet, and
 * colouring it would make a pending file look like a finished one.
 */
const OUTCOME_PANEL: Record<RestoreOutcomeDto, string> = {
  none: 'panel',
  staged: 'panel',
  applied: 'panel panel--good',
  refused: 'panel panel--error',
};

/** Total over the generated {@link BackupFormatDto}, for the same reason. */
const FORMAT_NAME: Record<BackupFormatDto, MessageKey> = {
  somfyRs: 'backup.formatSomfyRs',
  espSomfyRts: 'backup.formatEspSomfyRts',
};

/** The device's account of the last upload, or why there is not one. */
type Loaded =
  | { at: 'loading' }
  | { at: 'ready'; report: RestoreReportDto }
  | { at: 'unreachable'; detail: string };

/**
 * Where an upload has got to.
 *
 * `waiting` is the state this whole screen is shaped around: the device has
 * taken the file and is restarting, so it is *expected* to be unreachable and
 * the outcome does not exist yet. It is deliberately not called "restoring" —
 * nothing is being restored during it, and the boot that follows may well
 * refuse the file.
 */
type Upload =
  | { at: 'idle' }
  | { at: 'sending' }
  | { at: 'refused'; reason: MessageKey }
  | { at: 'waiting' }
  | { at: 'lost' };

export function Backup() {
  const t = useT();
  const [loaded, setLoaded] = useState<Loaded>({ at: 'loading' });
  const [upload, setUpload] = useState<Upload>({ at: 'idle' });
  const [reloadToken, setReloadToken] = useState(0);

  const reload = () => setReloadToken((token) => token + 1);

  useEffect(() => {
    let cancelled = false;
    setLoaded((previous) => (previous.at === 'ready' ? previous : { at: 'loading' }));
    getRestoreReport()
      .then((report) => {
        if (!cancelled) setLoaded({ at: 'ready', report });
      })
      .catch((cause: unknown) => {
        if (!cancelled) setLoaded({ at: 'unreachable', detail: detailOf(cause) });
      });
    return () => {
      cancelled = true;
    };
  }, [reloadToken]);

  /**
   * Wait out the restart the `202` promised.
   *
   * The subtle part is **not accepting the previous restore's answer**. A second
   * upload onto a device whose report already reads `applied` would otherwise
   * settle on the first poll, before the device had even rebooted, and show last
   * week's counts as this upload's result. So a settled outcome is only believed
   * once this loop has seen the device pass through the states only *this*
   * upload can produce: the `staged` the accepted upload wrote, or a request
   * that failed because the device had already gone. One of the two always
   * happens — the report is set to `staged` before the restart is signalled, and
   * the restart follows within milliseconds — and until then an `applied` is
   * treated as stale and polled past.
   */
  useEffect(() => {
    if (upload.at !== 'waiting') return;

    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let delay = POLL_MIN_MS;
    let restarted = false;
    const began = Date.now();

    const again = () => {
      if (cancelled) return;
      if (Date.now() - began > POLL_DEADLINE_MS) {
        setUpload({ at: 'lost' });
        return;
      }
      timer = setTimeout(tick, delay);
      delay = Math.min(delay * 2, POLL_MAX_MS);
    };

    const tick = () => {
      getRestoreReport()
        .then((report) => {
          if (cancelled) return;
          setLoaded({ at: 'ready', report });
          if (report.outcome === 'staged' || report.outcome === 'none') {
            // Neither is a settled answer, and neither can be *this* device's
            // answer from before the upload — the accepted `POST` wrote
            // `staged`. So reaching one of them is proof the report has moved
            // since, and an `applied` after it belongs to this upload.
            restarted = true;
            again();
            return;
          }
          if (restarted) {
            setUpload({ at: 'idle' });
            return;
          }
          again();
        })
        .catch(() => {
          if (cancelled) return;
          // The device going away is the expected middle of this wait, not a
          // failure — and it is the other proof that the restart happened.
          restarted = true;
          again();
        });
    };

    // The first poll waits rather than firing immediately: at the instant the
    // `202` arrives the device has not restarted yet, so an immediate request
    // would land on the boot that is about to go away.
    again();

    return () => {
      cancelled = true;
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [upload.at]);

  const send = (file: File) => {
    setUpload({ at: 'sending' });
    uploadBackup(file)
      .then(() => setUpload({ at: 'waiting' }))
      .catch((cause: unknown) => setUpload({ at: 'refused', reason: errorMessageKey(cause) }));
  };

  if (loaded.at === 'loading') {
    return (
      <div class="detail">
        <Head t={t} busy onRefresh={reload} />
        <section class="panel">
          <p>{t('backup.loading')}</p>
        </section>
      </div>
    );
  }

  if (loaded.at === 'unreachable' && upload.at !== 'waiting') {
    return (
      <div class="detail">
        <Head t={t} busy={false} onRefresh={reload} />
        <section class="panel panel--error" role="alert">
          <p>{t('backup.unreachable', { detail: loaded.detail })}</p>
          <button type="button" class="btn" onClick={reload}>
            {t('backup.retry')}
          </button>
        </section>
      </div>
    );
  }

  const report = loaded.at === 'ready' ? loaded.report : undefined;
  // Rendered once and placed twice — see this file's header for why its position
  // depends on whether it has anything to say.
  const reportPanel = report ? <ReportPanel report={report} t={t} /> : undefined;
  const newsworthy = report !== undefined && report.outcome !== 'none';

  return (
    <div class="detail">
      <Head t={t} busy={false} onRefresh={reload} />
      <p class="prose">{t('backup.intro')}</p>

      {newsworthy && reportPanel}
      <ExportPanel t={t} />
      <ImportPanel
        upload={upload}
        onSend={send}
        onGiveUp={() => {
          setUpload({ at: 'idle' });
          reload();
        }}
        t={t}
      />
      {!newsworthy && reportPanel}
    </div>
  );
}

/** Title bar, with the page-level refresh beside the title — `diagnostics.tsx`'s idiom. */
function Head({ t, busy, onRefresh }: { t: Translate; busy: boolean; onRefresh: () => void }) {
  return (
    <>
      <nav class="detail__nav">
        <a class="link" href="/">
          ← {t('detail.back')}
        </a>
      </nav>
      <header class="detail__head detail__head--action">
        <h2>{t('backup.title')}</h2>
        <button type="button" class="btn" disabled={busy} onClick={onRefresh}>
          {busy ? t('backup.refreshing') : t('backup.refresh')}
        </button>
      </header>
    </>
  );
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/**
 * One link, and three paragraphs saying what is and is not behind it.
 *
 * The link is an ordinary `<a download>` rather than a `fetch` — `BACKUP_URL`'s
 * documentation carries the argument, and the short version is that the device
 * already names the file in a `Content-Disposition` header and a browser
 * following a link honours it.
 */
function ExportPanel({ t }: { t: Translate }) {
  return (
    <section class="panel">
      <h3>{t('backup.exportTitle')}</h3>
      <p class="prose">{t('backup.exportWhat')}</p>
      {/*
        The omission is the half worth reading, so it is body text and not a
        note: an unauthenticated `GET` that carried the Wi-Fi passphrase would be
        a way to read it off the device over the LAN, and the two names the file
        *does* keep are what turn "retype your credentials" into a lookup. Setting
        the most important paragraph on the panel in the smallest type would have
        been exactly backwards.
      */}
      <p class="prose">{t('backup.exportNotSecrets')}</p>
      {/* An aside about hygiene rather than about the file, so it is a note. */}
      <p class="note">{t('backup.exportWhen')}</p>
      <div class="actions">
        <a class="btn btn--primary" href={BACKUP_URL} download>
          {t('backup.export')}
        </a>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

function ImportPanel({
  upload,
  onSend,
  onGiveUp,
  t,
}: {
  upload: Upload;
  onSend: (file: File) => void;
  onGiveUp: () => void;
  t: Translate;
}) {
  const [file, setFile] = useState<File | undefined>(undefined);
  /**
   * Whether the picker's extension filter has been switched off.
   *
   * A real fallback rather than a hint, because a picker that greys out the file
   * somebody has is a dead end: not every browser lets the filter be widened
   * from inside the dialog, and a backup that arrived by email or came off
   * another machine may well have been renamed. Turning the filter off is safe
   * because it was never the check — `firmware::restore::recognisable` decides
   * by looking at the first bytes, and a wrong file is refused immediately with
   * a code rather than staged.
   */
  const [anyFile, setAnyFile] = useState(false);

  const busy = upload.at === 'sending' || upload.at === 'waiting';

  return (
    <section class="panel">
      <h3>{t('backup.importTitle')}</h3>
      <p class="prose">{t('backup.importWhat')}</p>

      {/*
        These two outrank the control, and both sit above it for that reason.
        The first is what stops a `202` being read as success and is the one
        thing on the panel that is a *warning* — the device is about to restart
        and this page will go dark. The second is what makes pressing the button
        safe, and somebody weighing "will this break my shades?" reads it here or
        nowhere, so it is body text rather than fine print.
      */}
      <p class="note note--warn">{t('backup.importStaged')}</p>
      <p class="prose">{t('backup.importCodes')}</p>
      <p class="prose">{t('backup.importWhole')}</p>

      <label class="field">
        <span class="field__label">{t('backup.file')}</span>
        <input
          class="field__input"
          type="file"
          disabled={busy}
          {...(anyFile ? {} : { accept: ACCEPT })}
          onChange={(event) => setFile(event.currentTarget.files?.[0])}
        />
      </label>
      <label class="field field--check">
        <input
          type="checkbox"
          checked={anyFile}
          disabled={busy}
          onChange={(event) => setAnyFile(event.currentTarget.checked)}
        />
        <span class="field__hint">{t('backup.anyFile')}</span>
      </label>
      <p class="field__hint">{t('backup.fileHint')}</p>

      {file && <p class="mono">{t('backup.chosen', { name: file.name, bytes: file.size })}</p>}

      <div class="actions">
        <button
          type="button"
          class="btn btn--primary"
          disabled={busy || file === undefined}
          onClick={() => {
            if (file) onSend(file);
          }}
        >
          {upload.at === 'sending' ? t('backup.uploading') : t('backup.upload')}
        </button>
      </div>

      {upload.at === 'refused' && (
        <p class="note note--warn" role="alert">
          {t('backup.uploadRefused', { reason: t(upload.reason) })}
        </p>
      )}

      {/*
        The wait. `role="status"` rather than `alert`: nothing has gone wrong,
        the device is doing exactly what the button said it would.
      */}
      {upload.at === 'waiting' && (
        <p class="note" role="status">
          {t('backup.waiting')}
        </p>
      )}

      {upload.at === 'lost' && (
        <>
          <p class="note note--warn" role="status">
            {t('backup.lost')}
          </p>
          <div class="actions">
            <button type="button" class="btn" onClick={onGiveUp}>
              {t('backup.checkAgain')}
            </button>
          </div>
        </>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

function ReportPanel({ report, t }: { report: RestoreReportDto; t: Translate }) {
  const refused = report.outcome === 'refused';
  return (
    <section class={OUTCOME_PANEL[report.outcome]} {...(refused ? { role: 'alert' } : {})}>
      <h3>{t('backup.reportTitle')}</h3>
      <p class="outcome__sent">{t(OUTCOME_TITLE[report.outcome])}</p>

      {report.format !== null && (
        <p class="prose">{t('backup.format', { format: t(FORMAT_NAME[report.format]) })}</p>
      )}

      {report.outcome === 'none' && <p class="prose">{t('backup.outcomeNone')}</p>}
      {report.outcome === 'staged' && <p class="prose">{t('backup.outcomeStaged')}</p>}
      {report.outcome === 'applied' && <Applied report={report} t={t} />}
      {refused && <Refused report={report} t={t} />}
    </section>
  );
}

/**
 * What a successful restore put on the device, and what it could not.
 *
 * The counts come first because they are the answer to "did it work"; the
 * caveats and the retyping follow because they are the answer to "what is left
 * to do".
 */
function Applied({ report, t }: { report: RestoreReportDto; t: Translate }) {
  return (
    <>
      {/*
        A list rather than a sentence, and the reason is plurals. "4 shades, 2
        rooms and 1 groups" is what a single interpolated sentence produces, and
        three independently-varying counts would need eight catalogue entries per
        language to say properly — for a line whose whole content is three
        numbers. `i18n/index.tsx` records the ruling that a real plural rule
        means reaching for `Intl.PluralRules`; this is the cheaper answer, which
        is to write a sentence that does not need one. It is also the diagnostics
        screen's `.facts` idiom, where every number sits beside the thing it
        counts.
      */}
      <dl class="facts">
        <dt>{t('backup.appliedShades')}</dt>
        <dd class="mono">{report.shades}</dd>
        <dt>{t('backup.appliedRooms')}</dt>
        <dd class="mono">{report.rooms}</dd>
        <dt>{t('backup.appliedGroups')}</dt>
        <dd class="mono">{report.groups}</dd>
      </dl>

      {/*
        `warnings` is a count and not a list, and that is the device's decision
        rather than this screen's: every one of them is already a line in the log
        with the record and the reason, and carrying them here would be a second,
        narrower vocabulary for the same facts. So the count links to the place
        the sentences are.
      */}
      {report.warnings === 0 ? (
        <p class="note">{t('backup.warningsNone')}</p>
      ) : (
        <>
          <p class="note note--warn">
            {report.warnings === 1
              ? t('backup.warningsOne')
              : t('backup.warnings', { warnings: report.warnings })}
          </p>
          <p>
            <a class="link" href="/diagnostics">
              {t('backup.warningsLink')} →
            </a>
          </p>
        </>
      )}

      <h4 class="field__label">{t('backup.retypeTitle')}</h4>
      <p class="prose">{t('backup.retypeWhy')}</p>
      {report.contents === null ? (
        <p class="prose">{t('backup.retypeUnknown')}</p>
      ) : (
        <Retype contents={report.contents} t={t} />
      )}
      <p>
        <a class="link" href="/settings">
          {t('backup.retypeLink')} →
        </a>
      </p>
    </>
  );
}

/**
 * The two values a backup deliberately cannot carry, and which two they are.
 *
 * Each half has three readings and all three are said differently, because they
 * ask different things of the reader: a secret that has to be retyped, a network
 * that never had one, and nothing configured at all. Collapsing them into "check
 * your settings" would put the person who has nothing to do through the same
 * work as the person who does.
 */
function Retype({ contents, t }: { contents: BackupContentsDto; t: Translate }) {
  return (
    <>
      <p class="prose">
        {contents.ssid === null
          ? t('backup.retypeNoSsid')
          : contents.pskWasSet
            ? t('backup.retypeSsid', { ssid: contents.ssid })
            : t('backup.retypeSsidOpen', { ssid: contents.ssid })}
      </p>
      <p class="prose">
        {contents.broker === null
          ? t('backup.retypeNoBroker')
          : contents.brokerPasswordWasSet
            ? t('backup.retypeBroker', { broker: contents.broker })
            : t('backup.retypeBrokerOpen', { broker: contents.broker })}
      </p>
    </>
  );
}

/**
 * A refusal, and the sentence that matters most on this screen.
 *
 * "Nothing was written" is not reassurance, it is the state of the device: the
 * applier is all-or-nothing, so a refused file leaves the board running exactly
 * what it was running before the upload. Somebody reading a refusal is deciding
 * whether they have just lost their configuration, and this is the answer.
 */
function Refused({ report, t }: { report: RestoreReportDto; t: Translate }) {
  const reason: MessageKey = report.error ? ERROR_MESSAGE[report.error.code] : 'error.unknown';
  return (
    <>
      <p>{t('backup.refusedWhy', { reason: t(reason) })}</p>
      {/*
        `row` counts shades from zero and is null when the refusal is about the
        file rather than a record in it — a checksum that does not match, a
        format version this firmware does not read. Saying which of the two it is
        matters: one sends somebody to look at a record, the other tells them not
        to bother.
      */}
      <p class="prose">
        {report.row === null
          ? t('backup.refusedFile')
          : t('backup.refusedRow', { row: report.row })}
      </p>
      <p class="note note--warn">{t('backup.refusedNothing')}</p>
    </>
  );
}

/** Whatever the device or the network said, as something printable. */
function detailOf(cause: unknown): string {
  return cause instanceof ApiError ? cause.message : String(cause);
}
