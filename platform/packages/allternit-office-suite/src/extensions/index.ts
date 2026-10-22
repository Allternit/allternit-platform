/**
 * Extension slot for the per-app AI chat section.
 *
 * Hosts register `OfficeExtensionDescriptor`s on `OfficeHost.extensions`;
 * `OfficeAiSlot` occupies the app's AI chat section with the registered
 * agent panes (no built-in fallback tab). With no registered extensions the
 * app's own panel renders unchanged.
 */
export { OfficeAiSlot, type OfficeAiSlotProps } from './OfficeAiSlot';
export { useOfficeExtensions } from './useOfficeExtensions';
export {
  registerActiveDocument,
  reportActiveDocument,
  getActiveDocument,
  useActiveDocument,
} from './activeDocument';
export type { ActiveDocumentInfo } from './activeDocument';
export { createAllternitAssistantExtension } from './allternit-assistant';
export { requestAssistantPreset, onAssistantPreset } from './assistantPreset';
export {
  registerPaneAgent,
  getPaneAgent,
  paneAgentTools,
  paneAgentGuide,
  readPaneAgentContext,
  readPaneAgentAnchor,
  runPaneAgentTool,
  paneAgentAuthor,
  registerPaneAgentPersist,
  persistPaneAgent,
} from './paneAgent';
export type { PaneAgentSkill, PaneAgentToolDef } from './paneAgent';
export type { AssistantPresetDetail } from './assistantPreset';
export { AllternitBrandMark } from '../components/AllternitBrandMark';
export type { AllternitBrandMarkProps } from '../components/AllternitBrandMark';
