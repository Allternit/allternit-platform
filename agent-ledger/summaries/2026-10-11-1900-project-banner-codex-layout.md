# Project banner and customization layout

Eoj reported project tops did not match bot banners and requested a shorter, neatly arranged customization modal.

Shared ProjectBanner renders a full-width identity band for BaseProjectView and ProjectCoordinatorWorkspace. Bot projects keep their coordinator/team view; other modes retain their execution views and header actions. The same saved appearance recipe drives icon and independent banner color. Bot project title no longer adds a duplicate small folder icon. Phone layouts stack actions and allow main content to shrink.

AppearanceEditor has a smaller preview, preview/import/transforms left and colors/catalog right, compact two-column sliders, bounded catalog and scroll body, and fixed visible footer. No appearance persistence or save semantics changed.

Validation: 42 existing targeted project/appearance tests pass, including phone coordinator checks. Corrected missing registerProjectContextResolver in two coordinator test mocks. TypeScript/build and final shipping evidence recorded in handoffs/bot-redesign-2026-10-11/project-banner-completion.md after publication. Docs links and dependency map checked; shared WIP remains untouched.
