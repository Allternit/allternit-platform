/**
 * React Renderer
 * 
 * @example
 * ```tsx
 * import { createReactRenderer, createDefaultCatalog } from '@allternit/ix/react';
 * 
 * const renderer = createReactRenderer({
 *   catalog: createDefaultCatalog(),
 * });
 * 
 * function App() {
 *   return <renderer.UIRoot root={uiDefinition} />;
 * }
 * ```
 */

export {
  createReactRenderer,
  type ReactRenderer,
  type ReactRendererConfig,
} from './renderer';
