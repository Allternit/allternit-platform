/**
 * Vue Renderer
 * 
 * @example
 * ```typescript
 * import { createVueRenderer } from '@allternit/ix/vue';
 * import { createApp, h } from 'vue';
 * 
 * const renderer = createVueRenderer({
 *   stateStore: createStateStore(),
 * });
 * 
 * const App = renderer.render(uiDefinition);
 * createApp(App).mount('#app');
 * ```
 */

export {
  createVueRenderer,
  type VueRenderer,
  type VueRendererConfig,
} from './renderer';
