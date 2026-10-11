# Project banner and customization layout

Eoj reported project tops did not match bot banners and requested a shorter, neatly arranged customization modal.

Shared ProjectBanner renders a full-width identity band for BaseProjectView and ProjectCoordinatorWorkspace. Bot projects keep their coordinator/team view; other modes retain their execution views and header actions. The same saved appearance recipe drives icon and independent banner color. Bot project title no longer adds a duplicate small folder icon. Phone layouts stack actions and allow main content to shrink.

AppearanceEditor has a smaller preview, preview/import/transforms left and colors/catalog right, compact two-column sliders, bounded catalog and scroll body, and fixed visible footer. No appearance persistence or save semantics changed.

Validation: 42 existing targeted project/appearance tests pass, including phone coordinator checks. Corrected missing registerProjectContextResolver in two coordinator test mocks. TypeScript/build and final shipping evidence recorded in handoffs/bot-redesign-2026-10-11/project-banner-completion.md after publication. Docs links and dependency map checked; shared WIP remains untouched.

Follow-up: live review exposed shared dialog class sizing overriding requested width; explicit flex/width/body sizing fixes clipping and scrolling. All project modes now open one Appearance/Configuration dialog; duplicate device icon/color selectors removed. Configuration remains available during appearance errors. Bot phone uses width container queries for Workspace & Config, with stacked cards, horizontally scrolling file list and narrower editor/chat. Added section-switch draft-preservation regression coverage.
