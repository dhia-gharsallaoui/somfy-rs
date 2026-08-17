import preact from '@preact/preset-vite';
import { defineConfig } from 'vite';

import { mockApi } from './mock/plugin.ts';

/**
 * Build shape is dictated by where this app ships: inside the firmware image,
 * embedded with `include_bytes!` and served from flash with no filesystem.
 *
 * That rules out anything fetched at runtime and rewards a *small, fixed* set
 * of output files, because every file is a separate `include_bytes!` + route in
 * the firmware. So:
 *
 * - one JS chunk (`manualChunks` collapses vendor code into the entry),
 * - one CSS file (`cssCodeSplit: false`),
 * - stable, hash-free names, so the firmware's route table is written once,
 * - no `modulePreload` polyfill and no preload links — there is nothing to
 *   preload when everything is one file,
 * - assets under 8 KiB inlined as data URIs rather than becoming another route.
 *
 * The gzipping itself is not done here: an `xtask` pre-compresses `dist/` into
 * the image (design spec §8). `bun run size` reports what that will cost.
 */
export default defineConfig({
  plugins: [preact(), mockApi()],
  build: {
    target: 'es2022',
    cssCodeSplit: false,
    modulePreload: false,
    assetsInlineLimit: 8192,
    reportCompressedSize: true,
    rollupOptions: {
      output: {
        manualChunks: () => 'app',
        entryFileNames: 'assets/app.js',
        chunkFileNames: 'assets/app.js',
        assetFileNames: 'assets/app.[ext]',
      },
    },
  },
});
