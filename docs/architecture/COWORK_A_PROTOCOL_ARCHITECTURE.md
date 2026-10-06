# Cowork, Al, Gizzi, and Bots

## Purpose

This document defines the product/runtime relationship between Cowork, the A:// identity, Gizzi, and user-created bots.

It exists to prevent four common mistakes:

1. treating Cowork as the agent identity;
2. treating Al as a mandatory security root;
3. treating Gizzi Code as the entire product;
4. treating every bot as a transient chat session.

## Canonical product model

```text
User / Team
    |
    v
Al (A://)
Persistent Coworker / default orchestrator principal
    |
    +--> Gizzi
    |    technical / code / terminal worker
    |
    +--> User-created Bots
         independently permissioned specialized workers

Cowork = control room across the graph
```

This is the user-facing default, not a hardcoded security tree.

## 1. A:// / Al

The branded name is **A://**. It is pronounced **Al** when referring to the persistent user-facing Coworker.

Al is a durable principal with continuity across sessions and surfaces.

Al should be able to:

- receive user intent;
- maintain appropriate workspace/user continuity;
- plan and decompose work;
- create or delegate runs;
- choose workers by role/capability/policy;
- monitor canonical run state;
- request user approval;
- summarize results;
- expose ongoing work across desktop/mobile/channel surfaces.

Al should **not** become:

- the executor identity for every action;
- the owner of every credential;
- a universal memory bucket shared by all bots;
- a single mandatory queue for all peer communication;
- the only principal allowed to orchestrate.

## 2. Orchestration is a role

Internally, `orchestrator` is a role a principal may hold.

Al ships as the default orchestrator because that is the coherent user experience.

But the runtime must allow:

- a bot to orchestrate its own subdomain;
- flat peer-to-peer messaging where policy permits;
- an air-gapped worker that does not route through Al;
- multiple orchestration domains in one workspace;
- direct API-triggered work.

This preserves least privilege and avoids the confused-deputy problem.

## 3. Gizzi

Gizzi is the technical worker identity associated with Gizzi Code and the main terminal/developer surface.

Primary responsibilities:

- repository work;
- terminal/shell execution;
- code generation/editing;
- git operations;
- builds and tests;
- local files;
- development environments;
- debugging;
- VM/developer-runtime tasks.

Gizzi is below Al in the default UX, but Gizzi remains an independently attributable principal.

Example delegation:

```text
User: "Fix the failing build."
  -> Al understands/owns the user interaction
  -> Al delegates technical run to Gizzi
  -> Gizzi executes under its own principal/lease/policy
  -> Cowork shows the run
  -> Al reports the outcome
```

The ledger must attribute shell/code actions to Gizzi, not merely to Al.

## 4. User-created bots

A bot is a durable worker principal configured by a user or workspace.

A bot may have its own:

- name and identity;
- roles;
- instructions/persona;
- model/router policy;
- memory scope;
- connectors;
- credentials via brokered sessions;
- capabilities;
- approval policy;
- schedules/triggers;
- compute placement policy;
- budget/concurrency limits;
- channels;
- audit history.

Examples:

```text
Research Bot
Booking Bot
Bookkeeper Bot
Support Bot
Sales Bot
Browser Bot
Travel Bot
```

Bot identity must survive a model swap. Identity is not a model name or process ID.

## 5. Cowork

Cowork is the human control room for A:// activity.

Cowork is not Al and should never degrade into only "a chat with Al."

Cowork should make canonical work state visible:

- transcript/conversation;
- plan;
- active runs;
- jobs/DAG state;
- worker/principal assignment;
- lease/execution state where relevant;
- approvals;
- files touched;
- artifacts;
- connectors used;
- schedules/triggers;
- delegations/handoffs;
- event timeline;
- errors/recovery;
- audit attribution.

The transcript is one view of the work, not the source of truth for work state.

## 6. Reach surfaces versus control surfaces

Al should be reachable from multiple surfaces while remaining the same principal identity.

Possible reach surfaces:

```text
Desktop
Mobile
Slack / team chat
Email
Terminal
API
Voice
Allternit OS surfaces
```

Cowork is the richer control/inspection surface.

This distinction matters:

- reach surface: ask, steer, receive updates;
- control room: inspect execution, approvals, workers, artifacts, audit and recovery.

## 7. A:// as protocol versus Al as persona

Keep the distinction explicit.

**Protocol:**

```text
a://
```

Defines coordination semantics, identity/addressing, execution ownership, attribution, policy and results.

**Persona/principal:**

```text
Al
canonical address: a://workspace/{workspace}/principal/al
```

Al speaks A://. Al is not the protocol wearing a face.

Implementation documentation should avoid phrases such as:

- "A:// decided"
- "A:// owns all runs"
- "A:// executed the command"

Use the actual actor:

- Al delegated;
- policy denied;
- Gizzi executed;
- Fabric Transport leased;
- Research Bot completed.

## 8. Default delegation examples

### Technical coding task

```text
User
 -> Al
 -> Gizzi
 -> shell/git/files/build tools
 -> result
 -> Al reports
```

### Research task

```text
User
 -> Al
 -> Research Bot
 -> Browser Bot (optional delegated subtask)
 -> artifacts
 -> Al reports
```

### Payment-sensitive task

```text
User
 -> Al
 -> Bookkeeper Bot
 -> protected connector action
 -> approval required
 -> user grants/denies
 -> Bookkeeper Bot acts under its own lease
```

The Research Bot must not gain Bookkeeper Bot permissions because both were delegated by Al.

## 9. Memory boundaries

Memory should be explicitly scoped.

Potential scopes:

```text
user
workspace
principal
project
run
shared team memory
```

Do not assume Al's memory is automatically readable by every bot.

A bot should receive only memory relevant to its authorized scope.

## 10. Connector boundaries

Bots/principals should not normally receive reusable raw connector secrets.

Preferred architecture:

```text
Principal
 -> requests connector capability
 -> connector broker evaluates policy
 -> scoped attributed session
 -> external system
```

This makes revocation, attribution and least privilege possible.

## 11. Schedules, triggers, and persistence

Persistent bots differ from ordinary session agents because they can be addressable outside an open chat session.

Long-term bot triggers include:

- manual Cowork request;
- schedule/cron;
- webhook/event;
- connector event;
- file/system event;
- API call;
- Factory peer message;
- condition/watch trigger.

All trigger sources should converge into the same canonical Intent/Run pipeline rather than inventing separate execution systems.

## 12. Compute independence

A principal identity is independent of where its job runs.

The same bot may execute on:

```text
local host
isolated VM
another owned machine
BYO worker
Allternit cloud
```

Compute placement must not rewrite the executor identity.

This separation is foundational to Allternit's local/BYO/cloud positioning.

## 13. Model independence

Likewise, principal identity is independent of model selection.

```text
Al
 -> router
 -> model A for planning
 -> model B for hard task
 -> model C for specialized step
```

The user should still experience one durable Al identity.

Bots may have different model policies without becoming different principals on every model invocation.

## 14. Product language

Recommended user-facing language:

- **Ask A://** / **Ask Al**
- **Al is working**
- **Al assigned this to Gizzi**
- **Al assigned this to your Research Bot**
- **Approval needed**
- **Gizzi is running the build**
- **Research Bot completed the comparison**

Avoid exposing internal lease/transport terminology unless the user opens technical diagnostics.

## 15. Runtime truth hierarchy

When surfaces disagree, trust these layers in order:

```text
canonical persisted run/job state
 -> Fabric Transport lease ownership
 -> attributed event stream / ledger
 -> Cowork UI projection
 -> conversational narration
```

Model-generated text is never allowed to fabricate execution state.

## 16. Current code alignment

The present repo already contains important pieces of this model:

- Cowork web/desktop control surface;
- typed run/job state;
- principal records;
- Fabric Transport worker authentication/leases;
- approval bindings;
- event attribution fields;
- Factory peer messaging substrate;
- Gizzi Code/local execution path;
- bot-mode substrates;
- scheduler/runtime pieces.

The remaining product work is mostly integration and lifecycle convergence, not invention of an entirely new architecture.

## 17. Design test for every feature

Before adding a Cowork/Bot/Al/Gizzi feature, answer:

1. Which principal owns the identity?
2. Which role is it performing?
3. What is the canonical Intent/Run?
4. Which executor actually acts?
5. Which capabilities are required?
6. Which policy/approval applies?
7. Where does execution ownership live?
8. What is the attribution triple?
9. What result/artifact is committed?
10. How does Cowork observe it?

If those questions cannot be answered, the feature is likely creating another execution island.
