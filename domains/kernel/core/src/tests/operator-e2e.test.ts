/**
 * Operator E2E Integration Test
 * 
 * This test exercises the unified Kernel logic:
 * Orchestrator -> VisionParser -> IntegrityService -> DAK Provider -> Ralph Loop
 */

import { OperatorOrchestrator } from '../operator/orchestrator.js';
import { IntegrityService } from '../operator/integrity.js';
import fs from 'node:fs/promises';
import path from 'node:path';

async function runE2ETest() {
  console.log('🚀 Starting Operator E2E Integration Test...');

  // 1. Setup Test Data
  const sessionId = `test-session-${Date.now()}`;
  const orchestrator = new OperatorOrchestrator();

  // 2. Mock external dependencies that would require real API keys/services
  // We override the private methods for the test to avoid network calls
  (orchestrator as any).captureScreenshot = async () => "mock_b64_screenshot";
  (orchestrator as any).performVlmInference = async () => 
    "Thought: I see the icon.\nAction: click(start_box='(100,100)')";

  // Mock Rails sync to avoid 404s
  (IntegrityService as any).syncToFactory = async () => console.log('   [Mock] Sync to the Factory: OK');

  console.log(`\nTask: "Click the screen icon" (Session: ${sessionId})`);
  console.log('----------------------------------------------------');

  let actionCount = 0;
  let receiptGenerated = false;

  // 3. Execute the Stream
  try {
    for await (const event of orchestrator.streamTask({
      sessionId,
      intent: "click the screen icon",
      context: { platform: 'darwin' }
    })) {
      console.log(`[Event] type: ${event.type.padEnd(10)} | ${JSON.stringify(event).slice(0, 80)}...`);
      
      if (event.type === 'action') actionCount++;
      if (event.type === 'receipt') {
        receiptGenerated = true;
        // Verify receipt existence on disk
        const receiptPath = `.allternit/receipts/${event.receipt.receipt_id}.json`;
        try {
          await fs.access(receiptPath);
          console.log(`   ✅ Verified Receipt on disk: ${event.receipt.receipt_id}`);
        } catch {
          throw new Error(`Receipt file missing: ${receiptPath}`);
        }
      }
    }

    // 4. Final Assertions
    console.log('----------------------------------------------------');
    if (actionCount > 0 && receiptGenerated) {
      console.log('✅ E2E TEST PASSED: Logic flow is end-to-end wired.');
    } else {
      throw new Error(`Test failed: actions=${actionCount}, receipt=${receiptGenerated}`);
    }

  } catch (err: any) {
    console.error('❌ E2E TEST FAILED:', err.message);
    process.exit(1);
  }
}

runE2ETest();
