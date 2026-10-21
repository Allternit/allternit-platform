/**
 * Allternit-IX (Allternit Interface eXecution)
 * 
 * Declarative UI generation with Vercel Labs json-render compatibility.
 * 
 * @example
 * ```typescript
 * import { createUIRenderer, jsonRenderAdapter } from '@allternit/ix';
 * 
 * const ui = createUIRenderer({
 *   components: jsonRenderAdapter.components
 * });
 * 
 * ui.render({
 *   version: '1.0.0',
 *   components: [...],
 *   state: { variables: [...] },
 *   actions: [...]
 * });
 * ```
 */

// Types
export type {
  UIRoot,
  UIComponent,
  UIState,
  UIAction,
  StateVariable,
  ComputedValue,
  StateBinding,
  EventHandler,
  RepeatBinding,
  ActionHandler,
  Expression,
  ValidationRule,
  PersistenceConfig,
  UIMetadata,
  CatalogEntry,
  PropSchema,
  UIPatch,
  CustomHandlerContext,
  CustomHandlerFn,
} from './types';

// Catalog
export {
  ComponentCatalog,
  BUILT_INS,
  createDefaultCatalog,
  type CatalogComponent,
  type PropDefinition,
} from './catalog/registry';

// State Store
export {
  createStateStore,
  getStateStore,
  clearStateStore,
  type StateStore,
  type StateStoreConfig,
} from './state/store';

// JSON Patch Engine
export {
  applyPatch,
  applyPatchToStore,
  applyOperation,
  generatePatch,
  invertPatch,
  validatePatch,
  validateOperation,
  getValueAtPath,
  setValueAtPath,
  decodeJsonPointer,
  encodeJsonPointer,
  type JSONPatch,
  type JSONPatchOperation,
  type PatchResult,
} from './state/patch';

// Vercel Labs json-render adapter
export {
  jsonRenderAdapter,
  convertToJsonRender,
  convertFromJsonRender,
  globalHandlerRegistry,
  type JsonRenderNode,
  type JsonRenderComponent,
  type JsonRenderSchema,
  type CustomHandlerFn as JsonRenderHandlerFn,
  type HandlerRegistryEntry,
} from './adapters/json-render';

// React Renderer
export {
  createReactRenderer,
  type ReactRenderer,
  type ReactRendererConfig,
} from './react/renderer';

// Runtime
export {
  createIXCapsule,
  IXCapsuleRegistry,
  globalCapsuleRegistry,
  type IXCapsule,
  type IXCapsuleConfig,
  type IXCapsuleEvent,
  type IXCapsuleMetrics,
  type IXAggregatedMetrics,
  type IXMetricsExport,
} from './runtime/capsule-runtime';

// Policy Gates
export {
  createRateLimitGate,
  createActionAllowlistGate,
  createComponentRestrictionsGate,
  createStateValidationGate,
  createPropSanitizationGate,
  createResourceLimitGate,
  createAuditLogGate,
  composePolicyGates,
  createDefaultPolicyGates,
  type PolicyGate,
  type PolicyContext,
  type PolicyResult,
} from './runtime/policy-gates';

// LLM-to-IX Pipeline
export {
  convertLLMToIX,
  llmToIX,
  llmToIxPipeline,
  type LLMToIXOptions,
  type LLMToIXResult,
  type LlmToIxPipelineOptions,
  type LlmToIxPipelineResult,
  type IxRenderTree,
  type IxRenderNode,
  type IxTheme,
} from './pipeline/llm-to-ix';

// SDK
export * from './sdk';

// Utils
export { evaluateExpression, UI_IR_VERSION, validateUIRoot, createEmptyRoot } from './types';

/** Package version */
export const VERSION = '0.1.0';

/** Default schema version */
export const SCHEMA_VERSION = '1.0.0';
