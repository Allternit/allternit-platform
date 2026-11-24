# AllternitOS Living Roadmap

**Status:** Active architecture program  
**Owner:** Allternit  
**Created:** August 2, 2026  
**Last reviewed:** August 2, 2026  
**Review cadence:** Update at every architecture decision, milestone exit, or material evidence change  
**Canonical companion:** `docs/Future_Blueprints/BLUEPRINT-ALLTERNIT_AGENTIC_OS_V2.md`  
**Native OS companion:** `docs/Future_Blueprints/BLUEPRINT-ALLTERNIT_NATIVE_OS.md`  
**OS benchmark:** `docs/Core_System/00-Strategy/CANONICAL_OS_BENCHMARK_AND_AGENTIC_MAPPING.md`  
**Gap register:** `docs/Core_System/00-Strategy/ALLTERNIT_OS_GAP_AND_TRACEABILITY_REGISTER.md`  
**Upstream sourcing:** `docs/Core_System/00-Strategy/ALLTERNIT_OS_UPSTREAM_FORK_AND_BORROW_REGISTER.md`  
**Research companion:** `docs/Audits_and_Research/RESEARCH-AGENTIC-OS-CONVERGENCE-2026.md`  
**Visual artifact:** `docs/demos/allternit-os-ecosystem-map.html`

## Living-document protocol

This document is the operational index for the AllternitOS program. It records
what is decided, what remains uncertain, what existing code maps into the OS, and
what must happen next. It is intentionally shorter than the full specification
and more actionable than the research report.

Update this file when:

- an architecture decision is accepted or superseded;
- a legacy component is adopted, adapted, replaced, or retired;
- a milestone begins or exits;
- a conformance test changes status;
- a model, runtime, or isolation backend is selected;
- research materially changes a recommendation;
- a new first-class program or package type is approved.

Every update must change `Last reviewed`, update the relevant status table, and
append a dated entry to the change log. Claims of completion must link to code,
tests, receipts, or release evidence. Aspirations stay marked `proposed`.

## 1. Product decision

Allternit Platform is not discarded and replaced by a new OS application. The
platform is reorganized into the operating system.

After almost a year of development, the repository already contains substantial
parts of an agentic OS:

- agent and model runtimes;
- gizzi-code's harness, daemon, sessions, tools, permissions, memory, automation,
  durable goals, background tasks, and terminal interface;
- policy, receipts, purpose binding, audit, and evaluation components;
- workflows, DAGs, Rails, orchestration, parallel execution, and Cowork;
- provider and local-model adapters;
- browser and computer-use systems;
- plugin, skill, artifact, and registry systems;
- desktop, web, iOS, terminal, extension, and Office surfaces;
- local, remote, cloud, VM, and device execution infrastructure.

The failure was primarily one of topology and product framing: the old
AllternitOS was implemented as a web view above the platform instead of making the
platform itself the shared OS beneath every surface.

The v2 task is therefore **constitutional refactoring**, not a greenfield rewrite:

1. Identify the authoritative implementation of each OS responsibility.
2. Put stable contracts above existing implementations.
3. Remove duplicate or surface-local authority.
4. Package major capabilities as independently runnable programs.
5. Connect programs through shared workloads, artifacts, events, identities, and
   capability leases.
6. Retire the old React OS shell without discarding useful platform features.

## 2. Position in the system

```text
People, organizations, schedules, devices, and external events
                              │
┌─────────────────────────────▼────────────────────────────────┐
│                         SURFACES                             │
│ Desktop │ Web │ iOS │ gizzi-code │ Office │ Extensions │ API│
└─────────────────────────────┬────────────────────────────────┘
                              │ Allternit OS Protocol
┌─────────────────────────────▼────────────────────────────────┐
│                    ALLTERNIT PLATFORM = OS                  │
│ Workloads │ Identity │ Policy │ Agents │ Models │ Packages  │
│ Scheduler │ Memory │ Artifacts │ Approvals │ Receipts       │
│ Experience Compiler │ Recovery │ Device and runtime routing │
└──────────────┬──────────────────────────────┬────────────────┘
               │                              │
┌──────────────▼──────────────┐ ┌────────────▼────────────────┐
│ First-party OS programs    │ │ Execution fabric            │
│ Code, Cowork, Browser,     │ │ WASM, process, container,   │
│ Design, Research, etc.     │ │ microVM, VM, remote, device │
└─────────────────────────────┘ └─────────────────────────────┘
```

AllternitOS has two distributions behind one contract. Hosted runs above macOS,
Linux, Windows, iOS, or cloud infrastructure. Native boots directly through UEFI
into an immutable Linux system and an Allternit-owned compositor, shell, services,
applications, recovery, and update lifecycle. Hosted ships first; Native is
designed in parallel as the complete machine experience. Linux supplies the
initial hardware kernel while Allternit owns the operating system.

## 3. Decisions locked

| ID | Decision | Status |
|---|---|---|
| D-001 | Allternit Platform becomes the system-wide agentic OS. | accepted |
| D-002 | Desktop, Web, iOS, gizzi-code, Office, and extensions are OS clients/surfaces. | accepted |
| D-003 | Initial distribution is a native app plus persistent daemon. | accepted |
| D-004 | Native AllternitOS is a first-class bootable product/design track; it is not a dependency for the first Hosted release. | accepted |
| D-005 | Stable language-neutral contracts precede directory moves. | accepted |
| D-006 | Workloads, not chats or windows, are the durable unit of work. | accepted |
| D-007 | Agents, models, tools, skills, workflows, policies, and UI contributions are package resources. | accepted |
| D-008 | Models propose; policy authorizes; executors act; receipts attest. | accepted |
| D-009 | GGUF/llama.cpp is the first portable model lane; MLX is the first optimized Apple lane. | accepted |
| D-010 | Full-duplex voice is an OS service governed by a Turn Manager. | accepted |
| D-011 | Computer use prefers API, DOM, and accessibility before vision/pixels. | accepted |
| D-012 | Unattended computer use defaults to an isolated agent desktop. | accepted |
| D-013 | Agent-generated code targets WASM first. | accepted |
| D-014 | Successful turns create evidence, not automatically trusted skills. | accepted |
| D-015 | Teach Mode and the Experience Compiler generate skills, workflows, tools, and harnesses through staged promotion. | accepted |
| D-016 | Emergency stop, trust anchors, updater, and rollback live outside mutable agent logic. | accepted |
| D-017 | OS evidence is structurally distinct from model prose in every surface. | accepted |
| D-018 | QEMU is a research/conformance backend; native virtualization is preferred for consumer execution. | accepted |
| D-019 | Hosted and Native distributions implement one OS contract and package/workload model, never product forks. | accepted |
| D-020 | Native uses Linux initially but owns boot policy, immutable image, shell/compositor, packages, security, updates, recovery, and applications. | accepted |
| D-021 | Native targets a signed VM image first, one qualified x86-64 workstation second, and ARM64/appliance hardware afterward. | accepted |

## 4. Architecture decisions still required

| ADR | Question | Recommended default | Status |
|---|---|---|---|
| ADR-001 | Canonical source root | `os/` | draft required |
| ADR-002 | Contract schema and binding generation | Protobuf or JSON Schema with compatibility tests | investigation |
| ADR-003 | Durable event journal | Embedded transactional store first | investigation |
| ADR-004 | Local daemon implementation | Select from existing Rust/TypeScript runtimes by evidence | audit required |
| ADR-005 | WASM runtime | Adopt existing `domains/kernel/core/wasm-runtime` if qualified | audit required |
| ADR-006 | Package signing | Sigstore-compatible identities plus offline signatures | investigation |
| ADR-007 | macOS isolation backend | Apple Virtualization framework | recommended |
| ADR-008 | Linux/cloud isolation backend | Firecracker/Kata by deployment shape | recommended |
| ADR-009 | Windows isolation backend | Hyper-V/WSL2-backed executor | investigation |
| ADR-010 | Bootstrap model | Select from measured hardware-tier candidates | benchmark required |
| ADR-011 | Full-duplex voice model | Cascaded baseline, optional Moshi-class pack | benchmark required |
| ADR-012 | First reference workload | Evidence-backed research and report workflow | recommended |

## 5. Existing code becomes OS modules

The new root must not duplicate these systems. Each row requires a code-level
disposition audit.

| OS responsibility | Existing foundation | Intended disposition |
|---|---|---|
| Agent harness and terminal runtime | `cmd/gizzi-code` | Adopt as a principal execution/runtime implementation behind OS contracts |
| Persistent daemon and jobs | gizzi-code daemon, jobs, durable goals, background-task migrations | Adopt/adapt into workload runtime |
| Sessions and traces | gizzi-code sessions, session trace, continuity | Adapt into Workload/Event contracts |
| Permissions | gizzi-code permission system plus governance policy crates | Reconcile; select one non-bypassable capability path |
| Agent memory | gizzi-code brain/memory plus platform memory services | Reconcile into governed Memory objects |
| Tools and skills | gizzi-code tools/skills/plugins plus Platform plugin catalog | Package behind one registry and capability model |
| Workflows | Rails, DAG, `@allternit/workflow-engine`, Cowork engine | Reconcile behind Workload/Step scheduler contracts |
| Orchestration | `@allternit/orchestrator`, parallel-run, swarm systems | Adapt as scheduling/topology services, not separate kernels |
| Models/providers | provider adapters, local-model router, gizzi models/provider commands | Adopt behind Model manifests and Model Manager |
| Browser use | ACI/browser capsule, browser tools, computer-use protocol | Adapt into Browser Executor program/service |
| Desktop use | computer-use engine and native bridges | Adapt into capability-gated Host Accessibility Executor |
| Artifacts | artifact UI, registry, document and presentation systems | Adopt behind Artifact contracts and provenance |
| Governance | policy engines, system law, receipts, audit, purpose binding | Select canonical implementations and make non-bypassable |
| Packages | plugin SDK, plugin catalog, artifact registry, skill installer | Extend into transactional OS package manager |
| VM execution | Apple VF, Firecracker, executor infrastructure, VM managers | Adopt through one Executor contract |
| Remote devices | pairing, cloud relay, mobile instance store | Adopt behind Device/Runtime identity and relay contracts |
| Voice | web STT service, iOS voice mode, gizzi voice | Reconcile behind Turn Manager and shared event semantics |
| UI generation | OpenUI, A2UI, form surfaces, visual state | Package as declarative surface-extension runtime |

No module earns `adopt` solely because it exists. It must satisfy contract,
security, durability, ownership, and conformance requirements.

## 6. First-party OS programs

A program is a package or package family that can run independently through the
OS protocol while interoperating with other programs through workloads, artifacts,
events, and capabilities. A program may have different projections on Desktop,
Web, iOS, or TUI without duplicating its core runtime.

### 6.1 Operator

**Existing roots:** Chat, native agents, gizzi assistant/runtime, Agent Hub.  
**Role:** System home, intent intake, workload creation, approvals, explanations,
and handoff to specialist programs.  
**Standalone:** Headless or conversational agent.  
**Interconnection:** Creates and monitors every workload type.

### 6.2 Gizzi Code Studio

**Existing roots:** `cmd/gizzi-code`, Code views, terminal/PTY, VS Code SDK.  
**Role:** Coding, repositories, environments, tests, diffs, reviews, and software
delivery.  
**Standalone:** Full terminal-native or desktop coding agent.  
**Interconnection:** Produces code artifacts, packages, generated tools, and
harnesses consumed by other programs.

### 6.3 Cowork

**Existing roots:** Cowork views/controller/engine, projects, durable goals.  
**Role:** Long-running project work, multi-agent teams, tasks, files, and human
collaboration.  
**Standalone:** Project workspace service and UI.  
**Interconnection:** Owns grouped workloads and delegates to Code, Research,
Browser, Documents, and Automator.

### 6.4 Browser / ACI

**Existing roots:** ACI, browser capsules, browser tools, extension bridge.  
**Role:** Isolated browser sessions, research, form work, citations, downloads,
and verified web actions.  
**Standalone:** Browser agent/executor with API.  
**Interconnection:** Supplies evidence and controlled web capabilities to every
agent and workflow.

### 6.5 Computer

**Existing roots:** computer-use engine/protocol, desktop native bridges.  
**Role:** Host accessibility and isolated agent-desktop execution.  
**Standalone:** Executor/service rather than a decorative desktop.  
**Interconnection:** Offers capability-scoped application control to workflows.

### 6.6 Research and Knowledge

**Existing roots:** Research view, research documents, citations, memory,
knowledge services.  
**Role:** Evidence collection, synthesis, citations, knowledge graphs, and reports.  
**Standalone:** Research agent and artifact workspace.  
**Interconnection:** Uses Browser and Documents; produces governed knowledge and
artifacts.

### 6.7 Documents and Office

**Existing roots:** Documents view, Word/Excel/PowerPoint plugins, Office add-in,
form surfaces, data grid and presentation components worth extracting from legacy
AllternitOS.  
**Role:** Create, edit, transform, verify, and export business artifacts.  
**Standalone:** Office program suite or headless artifact service.  
**Interconnection:** Receives research/data and returns versioned artifacts.

### 6.8 Design / UI Forge

**Existing roots:** Design Mode, UI Forge, canvas, visual verification, OpenUI,
A2UI, tldraw.  
**Role:** Design interfaces, generated mini-apps, visual artifacts, and surface
extensions.  
**Standalone:** Visual design/generation environment.  
**Interconnection:** Compiles declarative UI packages and verifies visual output.

### 6.9 Workflow / Rails

**Existing roots:** Rails, DAG views, workflow engine, automation tables,
intelligent scheduling.  
**Role:** Deterministic orchestration, schedules, triggers, retries, checkpoints,
and reusable procedures.  
**Standalone:** Headless workflow runtime and visual builder.  
**Interconnection:** Compiles successful agent paths into cheaper deterministic
workflows.

### 6.10 Swarm / Teams

**Existing roots:** Swarm ADE, Cowork teams, orchestrator, parallel-run.  
**Role:** Multi-agent topology, delegation, consensus, simulation, and evaluation.  
**Standalone:** Agent-team runtime and topology workbench.  
**Interconnection:** Schedules specialist programs as workload participants.

### 6.11 Skill Studio / Teach Mode

**Existing roots:** Platform skills, skill installer, gizzi skills, plugin SDK,
future Experience Compiler.  
**Role:** Record demonstrations, distill verified episodes, author skills,
generate harnesses, replay, evaluate, and promote.  
**Standalone:** Skill authoring and registry client.  
**Interconnection:** Learns from every program without granting itself authority.

### 6.12 Model Lab

**Existing roots:** Model Management, local models, provider adapters, Playground.  
**Role:** Install models, benchmark hardware, route workloads, inspect licenses,
manage residency, and evaluate quality/cost/privacy.  
**Standalone:** Local model server and administration UI.  
**Interconnection:** Supplies intelligence resources to every workload.

### 6.13 Runtime and Fleet

**Existing roots:** runtime views, device pairing, cloud relay, infrastructure,
BYOC, VPS, environment management.  
**Role:** Local/remote runtimes, executors, devices, health, budgets, upgrades,
and recovery.  
**Standalone:** Headless runtime/fleet controller.  
**Interconnection:** Places workloads on qualified execution targets.

### 6.14 Native System and A://Labs

**Existing roots:** Labs, Playground, evaluation, VM, runtime, and research systems.  
**Role:** Bootable AllternitOS plus its lab for generated drivers, compiler
experiments, hardware qualification, alternative kernels, and new interactions.  
**Standalone:** Signed VM/workstation and recovery images; disposable lab images.  
**Interconnection:** Native consumes the same packages and workloads as Hosted;
Labs promotes only conformance-qualified outputs into stable OS channels.

## 7. Interconnection contract

Programs do not import one another’s private state. They communicate through:

- `Workload` and `Step` for durable execution;
- `Artifact` for versioned outputs;
- `Event` for authenticated state changes;
- `Capability` and `Lease` for authority;
- `Agent` and `Model` references for intelligence;
- `Package` for distribution and dependencies;
- `Approval` for protected transitions;
- `Receipt` for evidence;
- `Checkpoint` for recovery;
- `Memory` references for governed context.

Example:

```text
Operator creates market-research Workload
  → Research requests Browser capability
  → Browser exports cited Evidence Artifact
  → Analyst creates Data Artifact
  → Documents composes Report Artifact
  → Design creates Presentation Artifact
  → Approval permits external publication
  → Experience Compiler proposes reusable Workflow package
```

Each program can be absent. The scheduler reports an unmet package/capability
dependency instead of silently substituting unrelated UI code.

## 8. Immediate execution outline

### Stage 0 — Ratify and index

- Accept this living roadmap as the program index.
- Create ADR-001 through ADR-012.
- Create a machine-readable module disposition registry.
- Freeze feature development in the legacy React AllternitOS.

**Exit:** architecture authority and decision process are explicit.

### Stage 1 — Repository disposition audit

For every candidate module, record:

- owner and current callers;
- runtime language and process boundary;
- data and authority owned;
- duplicate implementations;
- security and durability properties;
- tests and production evidence;
- target OS contract;
- `adopt`, `adapt`, `replace`, `retire`, or `research` decision.

**Exit:** no proposed OS subsystem lacks an implementation decision.

### Stage 2 — Legacy AllternitOS retirement

- Re-home `ProgramErrorBoundary` used by `NativeAgentView`.
- Re-home or replace `OrchestratorProgram` used by OpenUI.
- Remove navigation, policy, type, and view-registry exposure.
- Preserve auth, pairing, and plugin uses of `allternit://`.
- Remove legacy source after inbound-reference proof.
- Regenerate desktop assets only in the normal release workflow.

**Exit:** the web program shell is gone without breaking unrelated features.

### Stage 3 — Contract spine

- Define the v1 kernel objects.
- Generate or validate Rust, TypeScript, and Swift bindings.
- Establish compatibility and migration rules.
- Build a local event stream and protocol discovery endpoint.

**Exit:** Desktop and gizzi-code can observe the same synthetic workload through
the same semantic protocol.

### Stage 4 — First vertical workload

- Package Operator and Researcher.
- Admit a durable research workload.
- Route through policy and capability leases.
- Run an isolated browser.
- Produce artifacts and receipts.
- Gate export through approval.
- Recover after daemon restart.
- Observe/control from Desktop and gizzi-code; approve/monitor from iOS/Web.

**Exit:** one useful cross-surface workload meets conformance requirements.

### Stage 5 — Model distribution

- Hardware probe and model manifests.
- GGUF/llama.cpp portable lane.
- MLX Apple lane.
- Verified downloads, installation, update, removal, and storage accounting.
- Select bootstrap and recommended model packs through Allternit benchmarks.

**Exit:** offline local routing and useful local agent execution work on qualified
hardware.

### Stage 6 — Voice and computer use

- Shared Turn Manager.
- Cascaded full-duplex baseline with barge-in.
- Optional native speech-to-speech pack.
- Browser, host accessibility, and isolated desktop executors.
- Correct interruption, approval, and spoken-output receipts.

**Exit:** a voice-started workload can safely use browser/desktop capabilities and
be interrupted without losing authoritative state.

### Stage 7 — Experience Compiler

- Verified episode store.
- Manual Save as Skill.
- Teach Mode modeled after and extending Microsoft Skill Recorder.
- Secret redaction, semantic reconstruction, parameter extraction.
- Harness generation, sandbox replay, shadow use, and promotion.
- Compile deterministic paths into workflows or WASM tools.

**Exit:** repeated verified work becomes measurably faster, cheaper, and more
reliable without silent self-modification.

### Stage 8 — Generative systems and ecosystem

- Tool, Harness, and Driver Foundries.
- Generated WASM capability packages.
- QEMU driver lab and disposable hardware simulations.
- Signed registry and third-party SDK.
- Organization policies and fleet management.
- Immutable Linux appliance evaluation.

**Exit:** external developers and agents can create, qualify, install, update, and
remove packages without private repository knowledge.

## 9. Canonical operating-system benchmark

The program is now benchmarked against the complete anatomy of mature operating
systems rather than other products that use the “agent OS” label. The exhaustive
48-responsibility matrix, traditional-object translation, legacy-OS comparison,
ownership map, claim gates, and test families live in
`CANONICAL_OS_BENCHMARK_AND_AGENTIC_MAPPING.md`.

### 9.1 Kernel object map

| Mature OS | Agentic OS | Allternit authority |
|---|---|---|
| Machine/device | Runtime, device, model, browser, app, robot | Runtime and Device Managers |
| User/service identity | Person, org, agent, package, device principal | Identity service |
| Process/job | Durable workload/workload group | Workload runtime |
| Thread | Step, branch, subagent task | Scheduler |
| Executable/library | Agent/program package and resource | Package Manager |
| System call | Typed capability request | Policy + Executor gateway |
| Access token/handle | Purpose-bound capability lease | Capability service |
| File/mount | Artifact and governed connector namespace | Artifact service |
| Virtual memory/cache | Context segment and retrieval cache | Context/Memory Manager |
| Pipe/socket/RPC | Typed event, stream, request, and handoff | OS protocol and event journal |
| Driver | Tool/executor adapter | Driver Manager |
| Window | Surface projection of shared state | Experience Manager |
| Audit record | Receipt and correlated event | Receipt/Journal service |
| Checkpoint/hibernate | Workload checkpoint | Recovery service |
| Package repository | Signed agent/model/tool/skill registry | Package Registry |
| Safe mode/recovery OS | External minimal recovery controller | Supervisor/Recovery runtime |

### 9.2 Layer closure map

| Real OS domain | Agentic requirement | Existing Allternit mapping | Closure required |
|---|---|---|---|
| Trust, secure boot, recovery | Signed bootstrap and recovery outside mutable agents | updater, governance, runtime infrastructure | measured start, safe mode, independent rollback |
| Kernel objects and syscalls | Stable objects and non-bypassable capability calls | sessions, goals, permissions, receipts | canonical schemas and one authority path |
| Scheduler, process, memory | Durable workloads, structured concurrency, context/resource pressure | Rails, workflow, Cowork, gizzi jobs/memory | unified admission, budgets, cancellation, context tiers |
| Drivers, I/O, Plug and Play | Qualified tools/devices with discovery, cancellation, and receipts | browser, computer use, Office, provider adapters | driver ABI, attestation, hot removal, conformance |
| Filesystem and storage | Governed artifact namespace, mounts, transactions, backup | artifacts, workspaces, documents, registry | VFS semantics, provenance, quota, restore, deletion |
| Identity, security, isolation | Principals, leases, secrets, sandbox ladder | auth, pairing, policy crates, VM systems | reconcile policy; bind every executor; tenant isolation |
| Services, config, packages | Supervision and transactional resource lifecycle | daemon/jobs, plugins, skills, local models | one package format, solver, health gates, rollback |
| UI, input, voice, attention | Surface-neutral state and interruptible multimodality | Desktop/Web/iOS/TUI/Office, voice, A2UI | shared Turn/Experience/Attention managers |
| Audit, failure, updates | Evidence journal, containment, checkpoint, migration | traces, receipts, audit, updater | correlated truth, chaos tests, atomic migration |
| Virtualization, fleet, admin | Risk placement and distributed control | Apple VF, Firecracker, relay, devices, gizzi-code | executor contract, attestation, remote stop, conformance CLI |

### 9.3 Product-claim gates

- **Agentic OS:** shared durable workload state; one policy boundary; transactional
  packages; bounded/cancellable work; qualified isolation; evidence taxonomy;
  restart recovery; model-independent emergency stop; public conformance.
- **Primary computing environment:** signed native distributions; isolated agent
  desktops; offline local model; full-duplex voice; fleet, backup, registry, and
  administrator tooling.
- **Native AllternitOS:** immutable Linux image; verified boot; encrypted storage;
  A/B updates; recovery image; hardware qualification; accessible primary shell.

Native is not a dependency for the first Hosted claim gate, but it is designed and
built in parallel. Linux/KVM can remain the hardware kernel while Allternit owns
the agentic kernel and the complete machine experience.

## 10. Reference systems and what they contribute

| System | Useful convergence | Allternit extension |
|---|---|---|
| Fable OS | Runtime compiler, generated drivers/apps, persistent capabilities, kernel evidence | Preserve generativity but add isolation, packages, policy, independent evaluation, and external rollback |
| AIOS | Agent kernel/SDK separation, scheduling, memory, tool and model resource management | Add stronger authority, package transactions, receipts, recovery, and multi-surface semantics |
| Microsoft UFO²/UFO³ | Hybrid GUI/API control, dynamic DAGs, capability-based multi-device placement, secure agent protocol | Add constitutional kernel objects, transactional packages, durable authority, local data lifecycle, and recovery |
| Microsoft Skill Recorder | Learn workflows from screen, window, browser, clipboard, and narration demonstrations | Add local redaction, accessibility/DOM capture, harnesses, replay, capabilities, and promotion lifecycle |
| iii AgentOS | Workers/functions/triggers, tenancy, goals, budgets, scheduling, governance, and self-improving functions | Add signed supply chain, host/device integration, artifact lifecycle, isolation qualification, and recovery |
| CoWork OS | Local-first workbench joining tasks, memory, skills, approvals, channels, devices, automations, and artifacts | Prove non-bypassable authority, transactional lifecycle, isolation, recovery, and cross-surface conformance |
| Omarchy | Installable, opinionated Linux distribution with coherent compositor/shell, CLI, themes, applications, hardware workflows, updates, migrations, and snapshot recovery | Borrow product coherence and daily interaction; replace unsandboxed extensions and rolling in-place authority with capability isolation, atomic images, evidence, and model-independent recovery |
| OpenDAN / OpenSwarm | Personal local AI aggregation and decentralized multi-agent coordination experiments | Integrate useful patterns as programs/protocols, not competing kernels |
| Hermes Agent | Agent-managed procedural skills and progressive loading | Require verified outcomes, versioning, shadow evaluation, and compile deterministic steps |
| Open Interpreter | Native app distribution, local execution, desktop work, sandbox approvals | Move durable authority into a shared daemon and package ecosystem |
| ElizaOS | Plugin ecosystem, connectors, model abstraction, agent UI | Separate extension ecosystem from non-bypassable kernel authority |
| Browser Use / Playwright | Practical browser automation and semantic control | Wrap behind isolated profiles, capabilities, receipts, and risk gates |
| OSWorld | Real cross-application benchmark and reproducible desktop environments | Build Allternit-specific long-horizon conformance and recovery tasks |
| Moshi | Open full-duplex speech model and low-latency conversational architecture | Govern through Turn Manager, approvals, model packages, and shared workload state |
| llama.cpp / MLX | Portable local inference and Apple-optimized execution | Add manifests, hardware policy, lifecycle, provenance, and workload-aware routing |
| Firecracker / Kata / gVisor | Risk-tiered workload isolation patterns | Hide backends behind one executor contract selected by workload risk |

Primary links are maintained in the companion research report and visual artifact.

## 11. Success measures

AllternitOS is succeeding when:

- a workload is not owned by the surface that created it;
- closing a UI does not terminate approved background work;
- the same workload is inspectable from multiple surfaces;
- every protected action has an authority chain and receipt;
- first-party programs can run independently without forking the kernel;
- installing a program does not require rebuilding a surface;
- local models work without cloud connectivity on qualified hardware;
- risky computer use runs in a recoverable isolated environment;
- learned workflows reduce median model calls, elapsed time, and cost;
- generated capabilities cannot bypass their declared leases;
- failed packages and runtime upgrades roll back automatically;
- model claims cannot appear as OS evidence;
- third-party developers can pass conformance without repository-private help.

## 12. Current program status

| Workstream | State | Next evidence |
|---|---|---|
| Architecture thesis | documented | ADR ratification |
| Industry research | documented | periodic refresh |
| Canonical OS benchmark | documented | assign owners and turn rows into conformance IDs |
| Gap and traceability register | initial canonical register | ratify owners, targets and repository evidence |
| Upstream fork/borrow register | mapped for all 48 responsibilities | pin revisions, verify licenses and begin 30-day composition |
| Existing module mapping | initial | code-level disposition registry |
| Legacy OS retirement | planned | dependency-safe removal change |
| Contract spine | not started | v1 schemas and bindings |
| Local daemon selection | not started | implementation audit |
| First workload | not started | executable vertical slice |
| Model distribution | researched | hardware/model benchmark |
| Full-duplex voice | researched | Turn Manager prototype |
| Computer use isolation | researched | executor qualification |
| Experience Compiler | specified | verified episode prototype |
| Generative systems | specified | WASM tool foundry prototype |
| Native OS distribution | canonical design | N0 signed QEMU image architecture and bill of materials |

## 13. Next action

Execute Gate A in the Gap and Traceability Register before deleting or moving
large systems: create the repository disposition and authority graphs, ratify the
canonical object/state contracts, select the journal and daemon, and prove the
capability/evidence boundary. Then build the single cross-surface vertical
workload defined by Gate B.

## Change log

- **2026-08-02:** Created the living roadmap. Locked the position that Allternit
  Platform becomes the OS; mapped Platform and gizzi-code capabilities into
  system modules and fourteen independently runnable/interconnected programs;
  recorded staged execution plan, reference-system mapping, and update protocol.
- **2026-08-02:** Added a canonical mature-OS benchmark covering the full stack
  from trust and boot through kernel objects, drivers, storage, security, UI,
  updates, recovery, fleet, and administration. Mapped traditional objects to
  agentic objects and Allternit owners; added Linux/macOS/Windows/Android patterns,
  claim gates, and expanded agent-OS convergence references.
- **2026-08-02:** Promoted Native AllternitOS from research-only positioning to a
  first-class bootable distribution designed in parallel with Hosted. Locked one
  OS contract across both distributions and added the Native blueprint covering
  boot, immutable image, compositor/shell, packages, hardware, isolation, models,
  installer, recovery, build lanes, milestones, and acceptance gates.
- **2026-08-02:** Added Omarchy as the native product-experience reference. Its
  opinionated distribution, persistent shell, unified CLI, themes, hardware
  repair, migrations, update channels, and snapshot recovery inform Allternit's
  daily system design; its unsandboxed shell plugins and rolling in-place update
  model are explicitly not adopted for autonomous-agent trust.
- **2026-08-03:** Added the canonical Gap and Traceability Register. Recorded all
  required fields for 48 OS responsibilities; added Hosted/Native parity, twenty
  end-to-end journeys, fourteen state machines, explicit threats, control/data/
  evidence separation, model evolution, generated-system qualification,
  repository evidence, failure-first reviews, measurable acceptance categories,
  severity-ranked gaps, named accountable roles, and the Gate A-E start sequence.
- **2026-08-03:** Added the upstream Fork and Borrow Register and reframed the
  Developer Preview as a 30-day composition program. Mapped every one of the 48
  responsibilities to upstream components, existing Allternit foundations and
  required glue; selected Omarchy as the product fork and Skill Recorder as a
  selective fork; constrained AIOS, iii AgentOS and Fable OS to qualified
  borrow/research roles; added source-risk gates and day-by-day exit evidence.
