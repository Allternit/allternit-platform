# Allternit Documentation — Master Index

**Last Updated:** September 18, 2026 (refreshed after the docs/ consolidation — loose depth-1 program docs filed into `programs/<program>/` and `learnings/` (S7); root `reports/`, `research/`, and `spec/` folded into `docs/` (S6))
**Status:** Unified, Bridge-Mapped, and Surgically Sorted

> **Taxonomy:** stable capitalized dir families (`Core_System/`, `Operations/`, `Future_Blueprints/`) and stable lower-case dirs (`architecture/`, `audit/`, `design/`, `marketing/`, `pipeline/`, `projects/`, `public/`, `research/`, `specs/`, `reports/`, `upstream/`, `archive/`, …) do not move. Loose program docs live under `programs/<program>/`; one-off orphans live under `learnings/`. See [README.md](./README.md) for the full rules.

---

## 🏗️ 1. Core System: The "Bridge" Strategy
*Connecting the current codebase to the target vision.*

### 📂 [01-Reality](./Core_System/01-Reality/) (Active Code Architecture)
*What is actually running in the monorepo today.*
- **System Map:** [ARCH-Global-System-Map.md](./Core_System/01-Reality/ARCH-Global-System-Map.md)
- **State Engines:**
  - [SPEC-Reality-Rails-Control-Plane.md](./Core_System/01-Reality/SPEC-Reality-Rails-Control-Plane.md)
  - [SPEC-Reality-Data-Fabric.md](./Core_System/01-Reality/SPEC-Reality-Data-Fabric.md)
  - [SPEC-Reality-Memory-Ars-Contexta.md](./Core_System/01-Reality/SPEC-Reality-Memory-Ars-Contexta.md)
  - [FrameworkRegistry.md](./Core_System/01-Reality/FrameworkRegistry.md)
- **Logic & Protocols:**
  - [SPEC-Reality-Workflow-Engine.md](./Core_System/01-Reality/SPEC-Reality-Workflow-Engine.md) (Declarative DAG Compilation & Tracking)
  - [SPEC-Reality-Skills-Tools.md](./Core_System/01-Reality/SPEC-Reality-Skills-Tools.md)
  - [SPEC-Reality-Security-Gate.md](./Core_System/01-Reality/SPEC-Reality-Security-Gate.md)
  - [Networking.md](./Core_System/01-Reality/Networking.md) | [GIZZI-Bridge.md](./Core_System/01-Reality/GIZZI-Bridge.md) | [ProtocolMap.md](./Core_System/01-Reality/ProtocolMap.md)
  - [RoutingPolicy.md](./Core_System/01-Reality/RoutingPolicy.md) | [SessionSemantics.md](./Core_System/01-Reality/SessionSemantics.md) | [review-protocol.md](./Core_System/01-Reality/review-protocol.md)
  - [ThreatModel.md](./Core_System/01-Reality/ThreatModel.md) | [ErrorSchema.md](./Core_System/01-Reality/ErrorSchema.md) | [Guarantees.md](./Core_System/01-Reality/Guarantees.md)
  - [CapabilityMatrix.md](./Core_System/01-Reality/CapabilityMatrix.md) | [Baseline.md](./Core_System/01-Reality/Baseline.md) | [BrowserActions.md](./Core_System/01-Reality/BrowserActions.md)
  - [KERNEL_CONTRACT_MAP.md](./Core_System/01-Reality/KERNEL_CONTRACT_MAP.md) | [KERNEL_UI_ADAPTERS.md](./Core_System/01-Reality/KERNEL_UI_ADAPTERS.md)
- **UI & Infrastructure:**
  - [SPEC-Reality-UI-Allternit-OS.md](./Core_System/01-Reality/SPEC-Reality-UI-Allternit-OS.md)
  - [SPEC-Reality-Infrastructure-Cloud.md](./Core_System/01-Reality/SPEC-Reality-Infrastructure-Cloud.md)
- **Guides:** [AGENT_QUICKSTART.md](./Core_System/01-Reality/AGENT_QUICKSTART.md) | [Cookbooks.md](./Core_System/01-Reality/Cookbooks.md)

### 📂 [02-Target](./Core_System/02-Target/) (Aspirational Specs)
*The foundational laws and "Good Ideas" we are working towards.*
- **The Laws:** [SYSTEM_LAW.md](./Core_System/02-Target/SYSTEM_LAW.md) | [UNIFIED_SOT.md](./Core_System/02-Target/UNIFIED_SOT.md) | [ARCHITECTURE.md](./Core_System/02-Target/ARCHITECTURE.md) | [SPEC-Subsystem-Kernel.md](./Core_System/02-Target/SPEC-Subsystem-Kernel.md) | [Invariants.md](./Core_System/02-Target/Invariants.md) | [Requirements.md](./Core_System/02-Target/Requirements.md) | [Vision.md](./Core_System/02-Target/Vision.md)
- **Agent Runner:** [SPEC-Target-Agent-Runner.md](./Core_System/02-Target/SPEC-Target-Agent-Runner.md) | [SPEC-Target-Bridge-Rails-Runner.md](./Core_System/02-Target/SPEC-Target-Bridge-Rails-Runner.md) | [SPEC-Target-Context-Pack-Seal.md](./Core_System/02-Target/SPEC-Target-Context-Pack-Seal.md) | [SPEC-Target-DAK-Runner.md](./Core_System/02-Target/SPEC-Target-DAK-Runner.md)
- **Subsystems:** [Messaging](./Core_System/02-Target/SPEC-Subsystem-Messaging.md) | [Workflow](./Core_System/02-Target/SPEC-Subsystem-Workflow-Engine.md) | [Memory](./Core_System/02-Target/SPEC-Subsystem-Memory-Fabric.md) | [Security & Governance](./Core_System/02-Target/SPEC-Subsystem-Security-Governance.md) | [Hooks](./Core_System/02-Target/SPEC-Subsystem-Hooks-System.md) | [Persona](./Core_System/02-Target/SPEC-Subsystem-Providers-Persona.md) | [Voice Swabble](./Core_System/02-Target/SPEC-Target-Voice-Swabble-Daemon.md) | [ALabs Vision](./Core_System/02-Target/SPEC-Target-ALabs-Vision.md) | [ALabs Arch](./Core_System/02-Target/SPEC-Target-ALabs-Architecture.md) | [Data Fabric](./Core_System/02-Target/ARCH-Component-Data-Fabric.md) | [DataFabric Spec](./Core_System/02-Target/DataFabric_Spec.md)
- **Modes:** [COWORK.md](./Core_System/02-Target/COWORK.md) | [CODE_MODE_REQUIREMENTS.md](./Core_System/02-Target/CODE_MODE_REQUIREMENTS.md) | [MODE_SPECIFIC_SESSIONS_SPEC.md](./Core_System/02-Target/MODE_SPECIFIC_SESSIONS_SPEC.md)
- **Advanced Architecture:** [DAG_MULTI_AGENT_COMMUNICATION.md](./Core_System/02-Target/DAG_MULTI_AGENT_COMMUNICATION.md) | [DAG_N2_TARGET_ARCHITECTURE_SPEC.md](./Core_System/02-Target/DAG_N2_TARGET_ARCHITECTURE_SPEC.md) | [multi-region.md](./Core_System/02-Target/multi-region.md)
- **Quality & Verification:** [AcceptanceTests.md](./Core_System/02-Target/AcceptanceTests.md) | [Conformance.md](./Core_System/02-Target/Conformance.md) | [CI_GATES_SPEC.md](./Core_System/02-Target/CI_GATES_SPEC.md) | [RCP-001_CODEBASE_GENERATOR.md](./Core_System/02-Target/RCP-001_CODEBASE_GENERATOR.md)
- **UI:** [UNIFIED_UI_IMPLEMENTATION.md](./Core_System/02-Target/UNIFIED_UI_IMPLEMENTATION.md)

### 📂 [03-Gaps](./Core_System/03-Gaps/) (Reconciliation Roadmap)
*Tracking the drift and technical debt.*
- [COMPREHENSIVE_GAP_ANALYSIS.md](./Core_System/03-Gaps/COMPREHENSIVE_GAP_ANALYSIS.md)
- [IMPLEMENTATION_STATUS.md](./Core_System/03-Gaps/IMPLEMENTATION_STATUS.md)
- [00-PLANNING_DELIVERABLES.md](./Core_System/03-Gaps/00-PLANNING_DELIVERABLES.md)
- [IMPLEMENTATION_PLAN_CORRECTED.md](./Core_System/03-Gaps/IMPLEMENTATION_PLAN_CORRECTED.md)
- [MigrationMap.md](./Core_System/03-Gaps/MigrationMap.md)
- [MODE_SESSIONS_MIGRATION_PLAN.md](./Core_System/03-Gaps/MODE_SESSIONS_MIGRATION_PLAN.md)
- [ORGANIZATION_SUMMARY.md](./Core_System/03-Gaps/ORGANIZATION_SUMMARY.md)

---

## 🚀 2. Future Blueprints
*Advanced research and visionary proposals (The "Good Ideas").*
- [BLUEPRINT-Allternit_Embodiment_RoboticsControlPlane_FullSpec.md](./Future_Blueprints/BLUEPRINT-Allternit_Embodiment_RoboticsControlPlane_FullSpec.md) | [BLUEPRINT-RLM_Integration_Plan.md](./Future_Blueprints/BLUEPRINT-RLM_Integration_Plan.md) | [BLUEPRINT-ALLTERNIT_BLUEPRINTS_ROADMAP.md](./Future_Blueprints/BLUEPRINT-ALLTERNIT_BLUEPRINTS_ROADMAP.md) | [BLUEPRINT-Intent-Compiler.md](./Future_Blueprints/BLUEPRINT-Intent-Compiler.md) | [BLUEPRINT-Vision-Always-On.md](./Future_Blueprints/BLUEPRINT-Vision-Always-On.md) | [BLUEPRINT-Vision-Code-Mode.md](./Future_Blueprints/BLUEPRINT-Vision-Code-Mode.md) | [BLUEPRINT-Ecosystem-Parity-Mobile.md](./Future_Blueprints/BLUEPRINT-Ecosystem-Parity-Mobile.md)

---

## 💼 3. Business, Product & Content
- **Business:** [ALLTERNIT_MANUFACTURING_STRATEGY_2026-04-07.md](./Business_Strategy/ALLTERNIT_MANUFACTURING_STRATEGY_2026-04-07.md) | [ALLTERNIT_BRAND_AUTHORITY.md](./Business_Strategy/ALLTERNIT_BRAND_AUTHORITY.md)
- **Product:** [ALABS-COURSE-CATALOG.md](./Product_and_Content/ALABS-COURSE-CATALOG.md)
- **Skills & Templates:** [Allternit_Identity_SKILL.md](./Product_and_Content/Skills_and_Templates/Allternit_Identity_SKILL.md) | [UNIT.template.md](./Product_and_Content/Skills_and_Templates/UNIT.template.md)

---

## 🛠️ 4. Operations
*Practical guides, deployment tracking, and setup instructions.*
- **Deployment:** [PRODUCTION_DEPLOYMENT_GUIDE.md](./Operations/PRODUCTION_DEPLOYMENT_GUIDE.md) | [ENTERPRISE_DEPLOYMENT_CHECKLIST.md](./Operations/ENTERPRISE_DEPLOYMENT_CHECKLIST.md) | [PORT_REGISTRY.md](./Operations/PORT_REGISTRY.md)
- **Computer Use:** [README.md](./Operations/Computer_Use/README.md) | [AGENT_NATIVE_USAGE_GUIDE.md](./Operations/Computer_Use/AGENT_NATIVE_USAGE_GUIDE.md) | [DEMONSTRATION_SUMMARY.md](./Operations/Computer_Use/DEMONSTRATION_SUMMARY.md)

---

## 📚 5. Programs (`programs/<program>/`)
*Phase-organized program docs: MAP (orchestrator analysis) → TASK (spec) → NOTES (sentinel-capped outcome). Filmed from the loose depth-1 caps-lock docs on 2026-09-18.*

| Program | Dir | Files | Key docs |
|---|---|---|---|
| Swarm builds | [swarm/](./programs/swarm/) | 68 | `SWARM_{A..E}_MAP.md`, `SWARM_*_PHASE*_NOTES.md` |
| Work engine (pre-Factory) | [rails/](./programs/rails/) | 24 | [RAILS_PRODUCT_UPDATE_SYSTEM_PROMPT.md](./programs/rails/RAILS_PRODUCT_UPDATE_SYSTEM_PROMPT.md), `RAILS_{GRAPH,MAIL,PARITY}_*` |
| Gizzi-code | [gizzi/](./programs/gizzi/) | 24 | [GIZZI_BOT_MODE_SPEC.md](./programs/gizzi/GIZZI_BOT_MODE_SPEC.md), `GIZZI_W2*`, `FABRIC_PWA_BOT_MODE_*`, `GC_*` |
| iOS | [ios/](./programs/ios/) | 15 | `IOS_BOT_PARITY_*`, `IOS_LOCAL_MODELS_MARKETPLACE_*` |
| Cloud agents | [cloud-agents/](./programs/cloud-agents/) | 8 | `CLOUD_AGENTS_PHASE_*` |
| AO engine / UHP | [ao/](./programs/ao/) | 11 | [UHP.md](./programs/ao/UHP.md), [UHP_VENDOR_INVENTORY.md](./programs/ao/UHP_VENDOR_INVENTORY.md), `AO_*_NOTES.md` |
| ACU shadow head | [acu/](./programs/acu/) | 3 | `ACU_SHADOW_HEAD_*` |
| Media plugins | [media-plugins/](./programs/media-plugins/) | 3 | `MEDIAPLUG1_NOTES.md`, [MEDIA_PLUGINS_MAP.md](./programs/media-plugins/MEDIA_PLUGINS_MAP.md) |

---

## 🧠 6. Learnings (`learnings/`)
*One-off and non-prefixed depth-1 orphans filed here on 2026-09-18 (214 files): audit tasks, triage notes, surface audits, agent-activity phases, setup guides, runbooks.*

Kept at depth 1 deliberately: [NATIVE_SESSIONS.md](./NATIVE_SESSIONS.md) and [AGENT_EMAIL_RAIL.md](./AGENT_EMAIL_RAIL.md) (both linked from the root `AGENTS.md`, which is path-frozen).

---

## 🗂️ 7. Stable Taxonomy Dirs
*Do not reorganize without an explicit decision (see [README.md](./README.md)).*

- **Specs & research:** [specs/](./specs/) (7) | [research/](./research/) (23) | [reports/](./reports/) (6) | [Audits_and_Research/](./Audits_and_Research/) (9) | [audit/](./audit/) (1) | [kimi-audit/](./kimi-audit/) (8) | [openai-audit/](./openai-audit/) (12) | [parity-reports/](./parity-reports/) (72) | [parity-reports-archive/](./parity-reports-archive/) (9) | [gap-analysis/](./gap-analysis/) (11)
- **Design & product:** [design/](./design/) (30) | [architecture/](./architecture/) (18) | [marketing/](./marketing/) (8) | [demos/](./demos/) (2) | [assets/](./assets/) (1)
- **Agent systems:** [a-protocol/](./a-protocol/) (2) | [agent-activity-design/](./agent-activity-design/) (5) | [agent-tasks/](./agent-tasks/) (46) | [bots/](./bots/) (2) | [desktop-cloud-mvp/](./desktop-cloud-mvp/) (48)
- **Pipeline & planning:** [pipeline/](./pipeline/) (44) | [plans/](./plans/) (1) | [development/](./development/) (1) | [infra/](./infra/) (1) | [jobs/](./jobs/) (1) | [learning/](./learning/) (67)
- **Upstream & public:** [upstream/](./upstream/) (10) | [public/](./public/) (119)
- **Projects & archive:** [projects/](./projects/) (allternit-cloud/ with MASTER_TRACKING.md + handoffs/, remote-control-gap-fix, porting-proven-patterns) | [archive/](./archive/) (708)

---

## Agent Gateway
- [Agent Gateway docs](./gateway/README.md) — AAI, adapters, look packs, channels, facades, operations

---

## 📦 8. Archive
- [Legacy Relics & Summaries](./Archive/)
- [Agent Examples & Workspaces](./Archive/Examples/)
