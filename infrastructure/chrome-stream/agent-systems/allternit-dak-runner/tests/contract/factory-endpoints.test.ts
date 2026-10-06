/**
 * Factory HTTP Endpoint Contract Tests
 * 
 * Tests to verify the Factory API endpoints (allternit-api, in process under
 * /api/factory) match DAK adapter expectations.
 * Ensures endpoint paths, request/response schemas are correct.
 */

import axios from 'axios';

const FACTORY_BASE_URL = process.env.FACTORY_BASE_URL || 'http://127.0.0.1:3000/api/factory';

describe('Factory HTTP Endpoints', () => {
  const axiosInstance = axios.create({
    baseURL: FACTORY_BASE_URL,
    timeout: 5000,
    headers: {
      'Content-Type': 'application/json',
    },
  });

  // ============================================================================
  // Health Check
  // ============================================================================

  describe('Health', () => {
    it('GET /ledger/health should return 200', async () => {
      const response = await axiosInstance.get('/ledger/health');
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('status');
      expect(response.data.status).toBe('ok');
    });
  });

  // ============================================================================
  // Work Discovery
  // ============================================================================

  describe('Work Discovery', () => {
    it('GET /v1/work/discover should return 200', async () => {
      const response = await axiosInstance.get('/v1/work/discover');
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('work_requests');
      expect(Array.isArray(response.data.work_requests)).toBe(true);
    });

    it('GET /v1/work/discover?role=builder should filter by role', async () => {
      const response = await axiosInstance.get('/v1/work/discover', {
        params: { role: 'builder' },
      });
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('work_requests');
    });

    it('GET /v1/work/discover?priority=1 should filter by priority', async () => {
      const response = await axiosInstance.get('/v1/work/discover', {
        params: { priority: 1 },
      });
      expect(response.status).toBe(200);
    });
  });

  // ============================================================================
  // Lease Management
  // ============================================================================

  describe('Lease Management', () => {
    let testLeaseId: string;

    it('POST /v1/leases should create a lease', async () => {
      const response = await axiosInstance.post('/v1/leases', {
        wih_id: 'wih_test',
        agent_id: 'test-agent',
        paths: ['/src/**'],
        ttl_seconds: 300,
      });
      expect(response.status).toBe(201);
      expect(response.data).toHaveProperty('lease_id');
      expect(response.data).toHaveProperty('granted');
      testLeaseId = response.data.lease_id;
    });

    it('GET /v1/leases/:id should return lease details', async () => {
      if (!testLeaseId) {
        // Create a lease first
        const createResponse = await axiosInstance.post('/v1/leases', {
          wih_id: 'wih_test',
          agent_id: 'test-agent',
          paths: ['/src/**'],
          ttl_seconds: 300,
        });
        testLeaseId = createResponse.data.lease_id;
      }

      const response = await axiosInstance.get(`/v1/leases/${testLeaseId}`);
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('lease');
      expect(response.data.lease).toHaveProperty('lease_id', testLeaseId);
    });

    it('GET /v1/leases should list leases', async () => {
      const response = await axiosInstance.get('/v1/leases');
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('leases');
      expect(Array.isArray(response.data.leases)).toBe(true);
    });

    it('GET /v1/leases?holder=test-agent should filter by holder', async () => {
      const response = await axiosInstance.get('/v1/leases', {
        params: { holder: 'test-agent' },
      });
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('leases');
    });

    it('POST /v1/leases/:id/renew should renew lease', async () => {
      if (!testLeaseId) {
        // Create a lease first
        const createResponse = await axiosInstance.post('/v1/leases', {
          wih_id: 'wih_test',
          agent_id: 'test-agent',
          paths: ['/src/**'],
          ttl_seconds: 300,
        });
        testLeaseId = createResponse.data.lease_id;
      }

      const response = await axiosInstance.post(`/v1/leases/${testLeaseId}/renew`, {
        extend_seconds: 300,
      });
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('lease');
      expect(response.data.lease).toHaveProperty('lease_id', testLeaseId);
    });

    it('DELETE /v1/leases/:id should release lease', async () => {
      if (!testLeaseId) {
        // Create a lease first
        const createResponse = await axiosInstance.post('/v1/leases', {
          wih_id: 'wih_test',
          agent_id: 'test-agent',
          paths: ['/src/**'],
          ttl_seconds: 300,
        });
        testLeaseId = createResponse.data.lease_id;
      }

      const response = await axiosInstance.delete(`/v1/leases/${testLeaseId}`);
      expect(response.status).toBe(200);
    });

    it('GET /v1/leases/:id should return 404 for non-existent lease', async () => {
      try {
        await axiosInstance.get('/v1/leases/lease_nonexistent');
        fail('Expected 404');
      } catch (error: any) {
        expect(error.response.status).toBe(404);
      }
    });
  });

  // ============================================================================
  // Gate Checking
  // ============================================================================

  describe('Gate Checking', () => {
    it('POST /v1/gate/check should accept gate check request', async () => {
      const response = await axiosInstance.post('/v1/gate/check', {
        wih_id: 'wih_test',
        dag_id: 'dag_test',
        node_id: 'node_test',
        run_id: 'run_test',
        tool: {
          name: 'fs.read',
          args: { path: '/test.txt' },
          intended_paths: ['/test.txt'],
        },
        context: {
          context_pack_id: 'cp_test',
          policy_bundle_id: 'pb_test',
          lease_id: 'lease_test',
        },
      });
      expect([200, 201]).toContain(response.status);
      expect(response.data).toHaveProperty('decision');
    });
  });

  // ============================================================================
  // WIH Management
  // ============================================================================

  describe('WIH Management', () => {
    it('POST /v1/wihs should list WIHs', async () => {
      const response = await axiosInstance.post('/v1/wihs', {});
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('wihs');
    });

    it('POST /v1/wihs/pickup should accept pickup request', async () => {
      const response = await axiosInstance.post('/v1/wihs/pickup', {
        wih_id: 'wih_test',
        dag_id: 'dag_test',
        node_id: 'node_test',
        agent_id: 'test-agent',
      });
      expect([200, 201, 400]).toContain(response.status); // 400 if WIH doesn't exist
    });
  });

  // ============================================================================
  // Context Pack
  // ============================================================================

  describe('Context Pack', () => {
    it('POST /v1/context-pack/seal should accept seal request', async () => {
      const response = await axiosInstance.post('/v1/context-pack/seal', {
        wih_id: 'wih_test',
        dag_id: 'dag_test',
        node_id: 'node_test',
        inputs: {
          tier0_law: 'SYSTEM_LAW content',
          sot: 'SOT content',
          architecture: 'ARCH content',
          contracts: [],
          deltas: [],
        },
      });
      expect([200, 201]).toContain(response.status);
      expect(response.data).toHaveProperty('pack_id');
    });

    it('GET /v1/context-pack/:id should return 404 for non-existent pack', async () => {
      try {
        await axiosInstance.get('/v1/context-pack/cp_nonexistent');
        fail('Expected 404');
      } catch (error: any) {
        expect(error.response.status).toBe(404);
      }
    });
  });

  // ============================================================================
  // Ledger
  // ============================================================================

  describe('Ledger', () => {
    it('POST /v1/ledger/tail should return ledger events', async () => {
      const response = await axiosInstance.post('/v1/ledger/tail', {
        count: 10,
      });
      expect(response.status).toBe(200);
      expect(Array.isArray(response.data)).toBe(true);
    });

    it('POST /v1/ledger/trace should accept trace request', async () => {
      const response = await axiosInstance.post('/v1/ledger/trace', {
        node_id: 'node_test',
      });
      expect(response.status).toBe(200);
    });
  });

  // ============================================================================
  // Receipts
  // ============================================================================

  describe('Receipts', () => {
    it('GET /v1/receipts/query should accept query', async () => {
      const response = await axiosInstance.get('/v1/receipts/query', {
        params: {
          run_id: 'run_test',
          wih_id: 'wih_test',
          type: 'tool_call_post',
          limit: 10,
          offset: 0,
        },
      });
      expect(response.status).toBe(200);
      expect(response.data).toHaveProperty('receipts');
    });
  });

  // ============================================================================
  // Path Alignment Tests
  // ============================================================================

  describe('DAK Adapter Path Alignment', () => {
    it('Gate endpoint should be /v1/gate/check (singular, not plural)', async () => {
      // This verifies the fix for DAK adapter path correction
      const response = await axiosInstance.post('/v1/gate/check', {
        wih_id: 'wih_test',
        dag_id: 'dag_test',
        node_id: 'node_test',
        run_id: 'run_test',
        tool: {
          name: 'fs.read',
          args: {},
          intended_paths: [],
        },
        context: {},
      });
      expect([200, 201, 400]).toContain(response.status);
      
      // Verify plural form does NOT exist
      try {
        await axiosInstance.post('/v1/gates/check', {});
        fail('Expected 404 for /v1/gates/check (plural)');
      } catch (error: any) {
        expect(error.response.status).toBe(404);
      }
    });
  });
});

// ============================================================================
// Test Runner
// ============================================================================

// Run tests if executed directly
if (require.main === module) {
  (async () => {
    console.log('Running Factory Endpoint Contract Tests...');
    console.log(`Factory Base URL: ${FACTORY_BASE_URL}`);
    
    try {
      // Quick health check
      await axiosInstance.get('/ledger/health');
      console.log('✓ Factory API is running');
      
      // Run all tests
      const testSuite = new describe('Factory HTTP Endpoints', () => {});
      console.log('✓ All tests defined');
      console.log('\nRun with Jest for full test execution:');
      console.log('  npm test -- factory-endpoints.test.ts');
    } catch (error: any) {
      console.error('✗ Factory API is not running');
      console.error(`  Error: ${error.message}`);
      console.error('\nStart allternit-api first (it serves /api/factory in process):');
      console.error('  cargo run -p allternit-api');
      process.exit(1);
    }
  })();
}
