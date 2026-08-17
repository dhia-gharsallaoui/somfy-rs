/**
 * Measure the built app against design spec §8's "≤ 200 KB gzipped total".
 *
 * The number that matters is the **whole `dist/` tree gzipped**, not the JS
 * chunk: the firmware embeds every file with `include_bytes!` and every file
 * costs flash. gzip level 9 is used because the xtask that pre-compresses these
 * assets has no reason to use anything else — nothing here is compressed at
 * request time.
 *
 * Exits non-zero over budget, so `bun run check` fails rather than reporting.
 */
import { gzipSync } from 'node:zlib';

const BUDGET_BYTES = 200 * 1024;
const DIST = new URL('../dist/', import.meta.url);

const files = [...new Bun.Glob('**/*').scanSync({ cwd: Bun.fileURLToPath(DIST) })].sort();
if (files.length === 0) {
  console.error('dist/ is empty — run `bun run build` first.');
  process.exit(1);
}

let rawTotal = 0;
let gzipTotal = 0;
const rows: string[] = [];

for (const name of files) {
  const bytes = new Uint8Array(await Bun.file(new URL(name, DIST)).arrayBuffer());
  const gzipped = gzipSync(bytes, { level: 9 }).byteLength;
  rawTotal += bytes.byteLength;
  gzipTotal += gzipped;
  rows.push(`  ${name.padEnd(22)} ${kib(bytes.byteLength).padStart(10)} ${kib(gzipped).padStart(11)}`);
}

const percent = ((gzipTotal / BUDGET_BYTES) * 100).toFixed(1);

console.log(`  ${'file'.padEnd(22)} ${'raw'.padStart(10)} ${'gzip -9'.padStart(11)}`);
console.log(rows.join('\n'));
console.log(`  ${'total'.padEnd(22)} ${kib(rawTotal).padStart(10)} ${kib(gzipTotal).padStart(11)}`);
console.log(`\n  budget ${kib(BUDGET_BYTES)} gzipped — using ${percent}%`);

if (gzipTotal > BUDGET_BYTES) {
  console.error(`\n  OVER BUDGET by ${kib(gzipTotal - BUDGET_BYTES)}`);
  process.exit(1);
}

function kib(bytes: number): string {
  return `${(bytes / 1024).toFixed(1)} KiB`;
}
