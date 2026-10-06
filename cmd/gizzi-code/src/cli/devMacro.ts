/**
 * Dev-mode definition of the MACRO global.
 *
 * The production build (script/build-production.js) injects `var MACRO = …`
 * at the top of the bundle. When running from source (bun run dev) nothing
 * defines it, and modules referencing `MACRO.VERSION` / `MACRO.*` throw
 * `MACRO is not defined`. Keep the shape in sync with build-production.js.
 */
import pkg from '../../package.json'

;(globalThis as any).MACRO ??= {
  VERSION: pkg.version,
  BUILD_TIME: new Date().toISOString(),
  PACKAGE_URL: pkg.name,
  ISSUES_EXPLAINER: 'https://github.com/Allternit/allternit-platform/issues',
}
