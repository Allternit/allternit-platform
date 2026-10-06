/**
 * Allternit Integrity Service (Kernel Port)
 * 
 * Implements G0501 (Immutable Receipts) and G0502 (Safety Interception)
 * natively in the Kernel.
 */

import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
// Real Policy Engine import (Localized from DAK)
import { createPolicyEngine } from './dak/policy-engine.js';

export interface OperatorReceipt {
  receipt_id: string;
  created_at: string;
  run_id: string;
  tool_id: string;
  input_hashes: string[];
  output_hashes: string[];
  artifact_manifest: any[];
  execution: {
    exit_code: number;
    duration_ms: number;
  };
}

export class IntegrityService {
  private static RECEIPTS_DIR = '.allternit/receipts';
  private static policyEngine = createPolicyEngine();

  /**
   * G0501: Generate Immutable Receipt
   */
  static async generateReceipt(
    sessionId: string, 
    action: any, 
    result: any
  ): Promise<OperatorReceipt> {
    const receiptId = `rcpt_${crypto.randomBytes(6).toString('hex')}`;
    const now = new Date().toISOString();

    const inputHash = crypto.createHash('sha256')
      .update(JSON.stringify(action)).digest('hex');
    const outputHash = crypto.createHash('sha256')
      .update(JSON.stringify(result)).digest('hex');

    const receipt: OperatorReceipt = {
      receipt_id: receiptId,
      created_at: now,
      run_id: sessionId,
      tool_id: action.action_type || action.tool || 'operator_action',
      input_hashes: [inputHash],
      output_hashes: [outputHash],
      artifact_manifest: [],
      execution: {
        exit_code: result.success ? 0 : 1,
        duration_ms: result.duration || result.metadata?.duration || 0
      }
    };

    // 1. Persist locally
    await fs.mkdir(this.RECEIPTS_DIR, { recursive: true });
    await fs.writeFile(
      path.join(this.RECEIPTS_DIR, `${receiptId}.json`),
      JSON.stringify(receipt, null, 2)
    );

    // 2. Sync to the Factory (The Nerves bridge)
    await this.syncToFactory(receipt);

    return receipt;
  }

  /**
   * G0502: Safety Interception
   * REAL: Evaluates action against the central PolicyEngine.
   */
  static async evaluateSafety(sessionId: string, action: any): Promise<boolean> {
    console.log(`[Integrity] Evaluating safety for action: ${action.action_type}`);
    
    const decision = await this.policyEngine.evaluate({
      tool: action.action_type,
      args: action.action_inputs,
      role: 'operator',
      context: { network_policy: 'none' } as any
    });

    return decision.type === 'ALLOW';
  }

  /**
   * REAL: Syncs receipt to the Factory control plane (allternit-api)
   */
  private static async syncToFactory(receipt: OperatorReceipt): Promise<void> {
    const factoryApiUrl = process.env.ALLTERNIT_FACTORY_API_URL || 'http://127.0.0.1:3000';
    try {
      const resp = await fetch(`${factoryApiUrl}/api/v1/receipts`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(receipt)
      });
      if (!resp.ok) console.warn('[Integrity] Failed to sync receipt to the Factory:', resp.statusText);
    } catch (err) {
      console.warn('[Integrity] Factory sync network error:', err);
    }
  }
}
