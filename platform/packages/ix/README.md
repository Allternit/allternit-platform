# @allternit/ix

Allternit Interface eXecution - Declarative UI generation with Vercel Labs json-render compatibility.

## Overview

Allternit-IX enables LLM-generated UI through a JSON Intermediate Representation (IR) with full compatibility for Vercel Labs json-render patterns. This allows AI agents to generate user interfaces declaratively, which are then rendered by the platform.

## Features

- **UI IR Schema**: Structured declarative UI definitions
- **Component Catalog**: 15 built-in components with semantic versioning
- **State Store**: Scoped state management with persistence
- **JSON Patch Engine**: RFC 6902 compliant state synchronization
- **json-render Adapter**: Bidirectional compatibility with Vercel Labs format
- **React Renderer**: Full React component rendering
- **Vue Renderer**: Vue 3 component generation
- **Svelte Renderer**: Svelte component compilation
- **IX Capsule Runtime**: Sandboxed execution with P3.9 integration
- **Policy Gates**: Security controls (rate limiting, sanitization, validation)
- **LLM-to-IX Pipeline**: Multi-format conversion from LLM outputs
- **Real-time Collaboration**: Yjs/CRDT integration
- **Visual Editor**: Fluent API for building UI IR

## Installation

```bash
pnpm add @allternit/ix
```

## Quick Start

### Basic React Usage

```typescript
import { createReactRenderer, createDefaultCatalog, createStateStore } from '@allternit/ix';

const renderer = createReactRenderer({
  catalog: createDefaultCatalog(),
  stateStore: createStateStore(),
});

const ui = {
  version: '1.0.0',
  components: [
    {
      id: 'header',
      type: 'Heading',
      props: { children: 'Hello World', level: 1 },
    },
    {
      id: 'input',
      type: 'Input',
      props: { placeholder: 'Enter name' },
      bindings: [
        { prop: 'value', statePath: 'name', direction: 'two-way' },
      ],
    },
  ],
  state: {
    variables: [
      { path: 'name', type: 'string', default: '' },
    ],
  },
  actions: [],
};

// Render in React
function App() {
  return <renderer.UIRoot root={ui} />;
}
```

### Vue Renderer

```typescript
import { createVueRenderer, compileToVue } from '@allternit/ix/vue';

// Generate Vue component definition
const vueComponent = compileToVue(uiDefinition);

// Use with Vue compiler or runtime
const { template, setup } = vueComponent;
```

### Visual Editor

```typescript
import { ui } from '@allternit/ix';

// Build UI programmatically
const myUI = ui.create()
  .state('count', 'number', 0)
  .add(
    ui.components.layout.Box({ padding: 4 })
      .child(ui.components.typography.Heading('Counter', 1))
      .child(
        ui.components.input.Button('Increment')
          .on('onClick', 'increment')
      )
  )
  .build();
```

### JSON-Render Compatibility

```typescript
import { jsonRenderAdapter, llmToIX } from '@allternit/ix';

// From json-render to Allternit-IX
const jsonRenderSchema = {
  version: '1.0.0',
  root: {
    type: 'Fragment',
    children: [
      { type: 'Text', props: { children: 'Hello' } },
    ],
  },
};

const allternitUi = jsonRenderAdapter.fromJsonRender(jsonRenderSchema);

// Auto-detect and convert
const result = llmToIX.convert(jsonRenderSchema);
```

### IX Capsule Runtime

```typescript
import { 
  createIXCapsule, 
  globalCapsuleRegistry,
  createRateLimitGate,
  createPropSanitizationGate,
} from '@allternit/ix';

// Create capsule with security policies
const capsule = globalCapsuleRegistry.create({
  id: 'my-capsule',
  ui: allternitUi,
  stateStore: createStateStore(),
  policyGates: [
    createRateLimitGate({ maxActionsPerMinute: 60 }),
    createPropSanitizationGate({ allowHTML: false }),
  ],
});

// Subscribe to events
capsule.subscribe((event) => {
  console.log('Capsule event:', event);
});

// Apply state patches
capsule.applyPatch([
  { op: 'replace', path: '/count', value: 10 },
]);
```

## Architecture

```
Allternit-IX
├── types/           # TypeScript interfaces
├── schema/          # UI IR schema definitions
├── catalog/         # Component registry with semver
├── state/           # Store + JSON Patch engine (RFC 6902)
├── adapters/        # Format converters (json-render)
├── react/           # React renderer
├── vue/             # Vue renderer
├── runtime/         # Capsule runtime + policy gates
├── sdk/             # API client + React hooks
└── pipeline/        # LLM-to-IX conversion
```

> Note: the `collab/` (Yjs/CRDT + Visual Editor) and `svelte/` subtrees were removed
> 2026-09-18 — they were never implemented against real dependencies (yjs, y-webrtc,
> y-protocols, lib0, svelte were never declared) and could not build. See PR #TBD.

## Component Catalog

Built-in components:

| Component | Category | Props |
|-----------|----------|-------|
| Box | layout | padding, margin, width, height, display, gap |
| Stack | layout | direction, spacing, align, justify |
| Text | typography | variant, size, weight, color, align |
| Heading | typography | level, color |
| Button | input | variant, size, disabled, onClick |
| Input | input | value, placeholder, type, onChange |
| TextArea | input | value, rows, onChange |
| Select | input | value, options, onChange |
| Card | display | padding, elevation, borderRadius |
| List | display | items, renderItem, emptyText |
| Table | display | data, columns, onRowClick |

## API Reference

### State Management

```typescript
import { createStateStore, applyPatch } from '@allternit/ix';

const store = createStateStore({
  initial: { count: 0 },
  persist: true,
  scopeId: 'my-app',
});

// Subscribe to changes
const unsubscribe = store.subscribe('count', (value) => {
  console.log('Count:', value);
});

// Apply JSON Patch
const patch = [
  { op: 'replace', path: '/count', value: 5 },
];
applyPatchToStore(store, patch);
```

### Policy Gates

```typescript
import {
  createRateLimitGate,
  createActionAllowlistGate,
  createStateValidationGate,
  composePolicyGates,
} from '@allternit/ix';

const policies = composePolicyGates(
  createRateLimitGate({ maxActionsPerMinute: 60 }),
  createActionAllowlistGate(['submit', 'cancel']),
  createStateValidationGate({
    email: {
      type: 'string',
      required: true,
      validate: (v) => v.includes('@') || 'Invalid email',
    },
  })
);
```

### LLM-to-IX Pipeline

```typescript
import { llmToIX } from '@allternit/ix';

// From JSX-like string
const result = llmToIX.fromJSX(`
  <Card>
    <Heading level={2}>Sign Up</Heading>
    <Input $value={email} placeholder="Email" />
    <Button onClick="submit">Submit</Button>
  </Card>
`);

// Auto-detect format
const detected = llmToIX.convert(unknownInput);
console.log(detected.confidence); // 0.0 - 1.0
```

## Implementation Status

### Week 1 ✅
- UI IR Schema (UIRoot, UIComponent, UIState, UIAction)
- Component Catalog (15 built-ins, semver support)
- State Store (scoped state, persistence, bindings)
- JSON Patch Engine (RFC 6902, full implementation)
- json-render Adapter (bidirectional conversion)

### Weeks 2-3 ✅
- React Renderer (complete with all built-ins)
- IX Capsule Runtime (sandboxed execution)
- Policy Gates (7 gate types + composition)
- LLM-to-IX Pipeline (multi-format conversion)
- Rust API Integration (Axum routes)
- TypeScript SDK (API client + React hooks)

### Option 4: Enhancements ✅
- Vue Renderer (component generation)

> Removed 2026-09-18 (never built — missing yjs/y-webrtc/y-protocols/lib0/svelte deps):
> Svelte Renderer, Yjs/CRDT Integration, Visual Editor, Awareness API.

## License

MIT
