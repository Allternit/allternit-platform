# Project and folder appearance

Source implementation: `src/components/appearance/` and `src/lib/appearance/`.
User guide and runtime contract live in the platform docs:

- `allternit/surfaces/docs/guides/project-appearance.mdx`
- `allternit/surfaces/docs/architecture/project-appearance.mdx`

The UI requires the runtime appearance route in `cmd/allternit-api/src/runtime_settings_routes.rs`.
Recipes are per authenticated user and connected runtime, separate from execution settings.
Desktop, web and phone/PWA share the editor. No OS folder changes occur.

Reference inspected: `/Users/joe/Desktop/tranmautritam_2.mp4` (iFolder).
Implemented color/paper layers, searchable symbols/brands, text/emoji, raster logo upload/paste,
overlay drag/position/scale/rotation, draft cancel, persisted reset, PNG and clipboard export.

Validation (2026-10-07): 66 unique frontend tests passed across appearance, Library,
project workspaces, bot project home/rail, and phone projects. Changed UI files pass
ESLint, both repositories pass diff whitespace checks, and the docs link check reports
0 problems. Rust source parses with rustfmt and reset SQL isolation was checked in SQLite.
Rust compilation and its added API tests have not run under the no-build instruction.
No commit, push, production build, or deploy was performed.

Landing verification: applied the scoped patch onto current main, retained newer Board/Flow views, reran 66 frontend tests successfully, and validated the dependency map (33 products, 1139 components, 5315 relationships).
