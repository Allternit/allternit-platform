// Preloaded into electron-builder on macOS (scripts/build-local.cjs).
// @electron/osx-sign checks every file in the bundle (~26k) in parallel
// before ad-hoc signing; that runs past the per-process open-file ceiling
// with EMFILE. graceful-fs queues opens and retries them instead.
'use strict';

require(require.resolve('graceful-fs', { paths: [__dirname] })).gracefulify(require('fs'));
