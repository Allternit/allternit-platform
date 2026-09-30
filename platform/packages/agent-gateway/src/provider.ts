/* eslint-disable @typescript-eslint/no-explicit-any */
import { unsupported, type AaiProvider, type AaiResult } from "./types.js";

/** Base class: every op answers UNSUPPORTED until a subclass overrides it. */
export abstract class BaseAaiProvider implements AaiProvider {
  abstract readonly adapterId: string;
  list(): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.list")); }
  get(_a: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.get")); }
  capabilities(_a: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.capabilities")); }
  identity(_a: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.identity")); }
  contextOpen(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.context.open")); }
  contextMessage(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.context.message")); }
  contextSteer(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.context.steer")); }
  contextCancel(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.context.cancel")); }
  contextClose(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.context.close")); }
  events(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.events")); }
  tasks(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.tasks")); }
  memory(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.memory")); }
  computer(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.computer")); }
  artifacts(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.artifacts")); }
  approvals(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.approvals")); }
  snapshot(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.snapshot")); }
  sync(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.sync")); }
  health(_i: any): Promise<AaiResult<any>> { return Promise.resolve(unsupported("agent.health")); }
}
