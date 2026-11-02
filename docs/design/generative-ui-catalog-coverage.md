# Generative UI catalog coverage

Eoj requested the complete reference inventory plus an extensible Allternit catalog. The earlier twelve-card list was an initial subset.

Upstream snapshot: `CopilotKit/CopilotKit@d168487e7a753a1527256c7e4c79344377f3d092`. Its examples README lists 65 consolidated demos: 24 integrations, seven canvas apps and 34 showcases. The inventory below records those entries. Legacy v1/v2 examples and additional source directories are tracked separately in the upstream tree.

## Domain catalog coverage

These are UI pattern mappings. They do not assert that the complete external apps or their backend services have been ported.

| Inspected reference catalog | Allternit controlled templates |
| --- | --- |
| banking | `transaction-ledger`, `spending-breakdown`, `budget-usage`, `income-expenses` |
| commerce | `product-grid`, `trading-list` |
| logistics | `inventory-risk`, `shipment-tracker`, `exception-board`, `tradeoff-table` |
| people | `people-directory`, `team-breakdown` |
| exec | `executive-brief`, `initiative-tracker`, `trend-comparison` |
| airline | `itinerary`, `flight-options`, `boarding-pass`, `baggage-tracker`, `seat-map`, `loyalty-summary` |
| bookstore | `reading-list` |
| keel | `workflow-run`, `task-approval`, `source-list`, `decision-log` |
| myelin | `plan`, `knowledge-map` |
| ledgerline | Catalog inspected; intentionally empty. Its visuals are tool renders. |

## Other inspected showcase patterns

| Reference | Allternit controlled templates |
| --- | --- |
| generative-ui-playground | `weather`, `stock`, `task-approval` |
| grok-generative-ui | `sentiment-split`, `argument-map`, `social-posts` |
| strands-crm | `deal-pipeline`, `deal-brief`, `account-research`, `followup-review` |
| mcp-apps | `flight-options`, `hotel-options`, `task-board` |
| a2ui-pdf-analyst | `pdf-analysis` |
| antigravity-repo-doctor | `repo-review`, `build-report` |

## Consolidated upstream examples

Framework integrations supply transport and agent-runtime patterns. Canvas and application demos can combine several controlled templates with existing product surfaces. They are not each counted as one interchangeable card.

| Category | Example | Source |
| --- | --- | --- |
| Integrations | langgraph-python | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/langgraph-python/) |
| Integrations | langgraph-js | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/langgraph-js/) |
| Integrations | langgraph-fastapi | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/langgraph-fastapi/) |
| Integrations | mastra | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/mastra/) |
| Integrations | crewai-flows | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/crewai-flows/) |
| Integrations | llamaindex | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/llamaindex/) |
| Integrations | pydantic-ai | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/pydantic-ai/) |
| Integrations | ms-agent-framework-python | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/ms-agent-framework-python/) |
| Integrations | ms-agent-framework-dotnet | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/ms-agent-framework-dotnet/) |
| Integrations | strands-python | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/strands-python/) |
| Integrations | strands-typescript | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/strands-typescript/) |
| Integrations | mcp-apps | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/mcp-apps/) |
| Integrations | adk | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/adk/) |
| Integrations | adk-angular | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/adk-angular/) |
| Integrations | antigravity | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/antigravity/) |
| Integrations | agentcore | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/agentcore/) |
| Integrations | agent-spec | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/agent-spec/) |
| Integrations | a2a-a2ui | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/a2a-a2ui/) |
| Integrations | agno | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/agno/) |
| Integrations | crewai-crews | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/crewai-crews/) |
| Integrations | a2a-middleware | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/a2a-middleware/) |
| Integrations | claude-sdk-python | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/claude-sdk-python/) |
| Integrations | claude-sdk-typescript | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/claude-sdk-typescript/) |
| Integrations | claude-managed-agents | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/integrations/claude-managed-agents/) |
| Canvas | langgraph-python | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/canvas/langgraph-python/) |
| Canvas | llamaindex | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/canvas/llamaindex/) |
| Canvas | llamaindex-composio | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/canvas/llamaindex-composio/) |
| Canvas | pydantic-ai | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/canvas/pydantic-ai/) |
| Canvas | mastra | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/canvas/mastra/) |
| Canvas | gemini | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/canvas/gemini/) |
| Canvas | mastra-pm | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/canvas/mastra-pm/) |
| Showcases | a2ui-pdf-analyst | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/a2ui-pdf-analyst/) |
| Showcases | arcade-tools | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/arcade-tools/) |
| Showcases | claude-managed-agents | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/claude-managed-agents/) |
| Showcases | daytona-runcode | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/daytona-runcode/) |
| Showcases | reskinnable-demo | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/reskinnable-demo/) |
| Showcases | presentation | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/presentation/) |
| Showcases | deep-agents-finance-erp | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/deep-agents-finance-erp/) |
| Showcases | deep-agents-job-search | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/deep-agents-job-search/) |
| Showcases | generative-ui | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/generative-ui/) |
| Showcases | grok-generative-ui | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/grok-generative-ui/) |
| Showcases | research-canvas | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/research-canvas/) |
| Showcases | mcp-demo | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/mcp-demo/) |
| Showcases | multi-page | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/multi-page/) |
| Showcases | open-mcp-client | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/open-mcp-client/) |
| Showcases | oracle-agent-memory | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/oracle-agent-memory/) |
| Showcases | orca | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/orca/) |
| Showcases | pydantic-ai-todos | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/pydantic-ai-todos/) |
| Showcases | adk-dashboard | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/adk-dashboard/) |
| Showcases | langgraph-js-support-agents | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/langgraph-js-support-agents/) |
| Showcases | multi-agent-canvas | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/multi-agent-canvas/) |
| Showcases | enterprise-brex | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/enterprise-brex/) |
| Showcases | spreadsheet | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/spreadsheet/) |
| Showcases | todo | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/todo/) |
| Showcases | strands-crm | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/strands-crm/) |
| Showcases | manufact-mcp-apps | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/manufact-mcp-apps/) |
| Showcases | antigravity-repo-doctor | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/antigravity-repo-doctor/) |
| Showcases | deep-agents | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/deep-agents/) |
| Showcases | generative-ui-playground | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/generative-ui-playground/) |
| Showcases | mcp-apps | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/mcp-apps/) |
| Showcases | strands-file-analyzer | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/strands-file-analyzer/) |
| Showcases | microsoft-kanban | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/microsoft-kanban/) |
| Showcases | scene-creator | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/scene-creator/) |
| Showcases | chatkit-studio | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/chatkit-studio/) |
| Showcases | a2a-travel | [Source](https://github.com/CopilotKit/CopilotKit/tree/d168487e7a753a1527256c7e4c79344377f3d092/examples/showcases/a2a-travel/) |

## Our own catalog

Allternit also registers operational views for agents, tool runs, builds, deployments, incidents, connector health, permissions, invoice and quote review, task boards, handoffs, cost, design reviews, shot lists, verification, knowledge relationships, file results and agendas. Each entry has a schema, trusted renderer and synthetic example. New entries flow into the gallery, model manual, generated server schema and shared regression corpus.

Catalog source: `allternit-ai/src/lib/openui/answer-catalog.ts`. Gallery and original templates: `answer-gallery.ts` and `answer-contract.ts`.

## Integration status and deterministic use

The 82-template registry is connected to the model manual, public card renderer, UI Forge, local artifact writes, cloud create/version writes, and Gizzi artifact tools. It is not a forced response format for every answer.

The separate automatic answer-outline picker selects from eight writing outlines using fixed keyword priorities. Only comparison, decision and plan currently carry explicit card suggestions. This picker does not route across all 82 catalog entries or validate that the model followed its outline. The model manual permits Markdown when a card is not needed, and Text only omits the card manual.

Given the same accepted envelope and user state, the application owns the layout and calculations. Runtime connector reads may return different data. Invalid card envelopes cannot execute raw layouts through the public renderer. This guarantee does not force an agent to emit a card or prevent it from proposing another artifact kind.

The November 1 expansion is source-only in session/genui-enforcement-20261101 worktrees. It has not been committed, merged or deployed. Shared-source wiring and automated example checks are not evidence of live desktop/web/PWA adoption. No production usage counts or signed-in cross-device results have been collected for this expansion.

To meet a requirement for deterministic catalog use, a future change must choose an allowed template policy before generation, validate the returned template against that policy, handle missing or mismatched output, record the chosen/rendered template, and verify the released paths on all three surfaces. Preserve explicit Text only and custom-app choices.

## November 1 visual refresh

The shared presentation now adds category identity and typography to all 82 templates. Seventeen entries have domain-specific React layouts fed by the already validated envelope. The remaining entries retain their fixed compiler compositions with improved shared metrics, tables, comparisons, checklists and section styles. Richer gallery fixtures remain synthetic. Source-only status and selection-enforcement limitations above still apply.
