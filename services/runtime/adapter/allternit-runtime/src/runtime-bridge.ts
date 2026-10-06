/** The slice of a session's orchestration state this bridge reads and sets. */
export interface OrchestrationContext {
  sessionId: string;
  agentId: string;
  mode?: string;
}

// Stub for the optional native policy bridge until bindings are compiled.
class NativeBridge {
  evaluate_policy(_input: string): string {
    return JSON.stringify({ decision: 'allow', reason: 'stub' });
  }
}

export class RuntimeBridge {
  private native: NativeBridge | null = null;

  constructor() {
    try {
      this.native = new NativeBridge();
    } catch (e) {
      console.warn('[RuntimeBridge] Native bridge not found, falling back to JS implementation');
    }
  }

  async processCommand(input: string, context: OrchestrationContext) {
    // Handling a:// and @ triggers
    if (input.startsWith('a://browser')) {
      context.mode = 'vision';
      console.log('[RuntimeBridge] Switching to VISION mode');
    } else if (input.includes('capture:network')) {
      context.mode = 'network';
      console.log('[RuntimeBridge] Switching to NETWORK mode');
    }

    if (this.native) {
      const policyResponse = await this.native.evaluate_policy(JSON.stringify({
        intent: input,
        context: context.sessionId
      }));
      const decision = JSON.parse(policyResponse);
      if (decision.decision !== 'allow') {
        throw new Error('Policy Violation: ' + decision.reason);
      }
    }

    return context;
  }
}
