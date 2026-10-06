/**
 * Allternit Gateway
 * 
 * Routes all UI requests to appropriate backend services.
 * Architecture: UI → Gateway (8013) → API (3000) → Kernel (3004)
 */

import express from 'express';
import cors from 'cors';
import { createCanvasCommands } from './commands/canvas.js';
import { createBrowserCommands } from './commands/browser.js';

const app = express();
const PORT = process.env.GATEWAY_PORT || 8013;

// Middleware
app.use(cors());
app.use(express.json({ limit: '10mb' }));

// Request logging
app.use((req, res, next) => {
  console.log(`[Gateway] ${req.method} ${req.path}`);
  next();
});

// ═════════════════════════════════════════════════════════════════════════════
// Health Check
// ═════════════════════════════════════════════════════════════════════════════
app.get('/health', (req, res) => {
  res.json({ 
    status: 'ok', 
    service: 'gateway',
    version: '0.1.0',
    timestamp: new Date().toISOString()
  });
});

// ═════════════════════════════════════════════════════════════════════════════
// Gateway Tool Router
// ═════════════════════════════════════════════════════════════════════════════

// Initialize command handlers
const canvasCommands = createCanvasCommands({
  invoke: async (command, params) => {
    // Forward to kernel
    const kernelUrl = process.env.KERNEL_URL || 'http://127.0.0.1:3004';
    const response = await fetch(`${kernelUrl}/v1/commands/execute`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ command, params }),
    });
    
    if (!response.ok) {
      throw new Error(`Kernel error: ${await response.text()}`);
    }
    
    const data = await response.json();
    return data.result;
  },
});

const browserCommands = createBrowserCommands({
  getBrowserUrl: () => process.env.BROWSER_URL || 'http://127.0.0.1:9222',
  fetch: fetch as any,
});

// Tool execution endpoint
app.post('/api/v1/gateway/tool', async (req, res) => {
  const { tool, params } = req.body;
  
  if (!tool) {
    return res.status(400).json({ error: 'Tool name required' });
  }
  
  console.log(`[Gateway] Executing tool: ${tool}`);
  
  try {
    let result;
    
    // Route to appropriate command handler
    switch (tool) {
      // Canvas commands
      case 'canvas.present':
        result = await canvasCommands.present(params);
        break;
      case 'canvas.hide':
        result = await canvasCommands.hide();
        break;
      case 'canvas.navigate':
        result = await canvasCommands.navigate(params);
        break;
      case 'canvas.eval':
        result = await canvasCommands.eval(params);
        break;
      case 'canvas.snapshot':
        result = await canvasCommands.snapshot(params);
        break;
      case 'canvas.a2ui.push':
      case 'canvas.a2ui.pushJSONL':
        result = await canvasCommands.a2ui.push(params);
        break;
      case 'canvas.a2ui.reset':
        result = await canvasCommands.a2ui.reset();
        break;
        
      // Browser commands
      case 'browser.proxy':
        result = await browserCommands.proxy(params);
        break;
      case 'browser.status':
        result = await browserCommands.status(params);
        break;
      case 'browser.start':
        result = await browserCommands.start(params);
        break;
      case 'browser.stop':
        result = await browserCommands.stop(params);
        break;
        
      default:
        return res.status(404).json({ error: `Unknown tool: ${tool}` });
    }
    
    res.json({ payload: result });
  } catch (error) {
    console.error(`[Gateway] Tool error:`, error);
    res.status(500).json({ 
      error: error instanceof Error ? error.message : String(error)
    });
  }
});

// ═════════════════════════════════════════════════════════════════════════════
// A2UI Routes (Proxy to API)
// ═════════════════════════════════════════════════════════════════════════════

app.use('/api/v1/a2ui', async (req, res) => {
  const apiUrl = process.env.API_URL || 'http://127.0.0.1:3000';
  const targetUrl = `${apiUrl}/api/v1/a2ui${req.path}`;

  try {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
    };
    if (req.ip) {
      headers['X-Forwarded-For'] = req.ip;
    }

    const response = await fetch(targetUrl, {
      method: req.method,
      headers,
      body: req.method !== 'GET' ? JSON.stringify(req.body) : undefined,
    });
    
    const data = await response.json();
    res.status(response.status).json(data);
  } catch (error) {
    res.status(500).json({ 
      error: 'API proxy error',
      details: error instanceof Error ? error.message : String(error)
    });
  }
});

// ═════════════════════════════════════════════════════════════════════════════
// ═════════════════════════════════════════════════════════════════════════════
// Generic API Proxy (Catch-all for v1, the Factory, and Sessions)
// ═════════════════════════════════════════════════════════════════════════════

app.use(['/api/v1', '/api/factory', '/api/chat', '/session', '/v1'], async (req, res) => {
  // allternit-api serves all of these, /api/factory included (in process).
  const apiUrl = process.env.API_URL || 'http://127.0.0.1:3000';
  const targetUrl = `${apiUrl}${req.baseUrl}${req.path}${req.url.includes('?') ? '?' + req.url.split('?')[1] : ''}`;

  console.log(`[Gateway] Proxying to: ${targetUrl}`);

  try {
    const options: RequestInit = {
      method: req.method,
      headers: {
        'Content-Type': 'application/json',
        ...req.headers as any
      }
    };

    if (req.method !== 'GET' && req.method !== 'HEAD') {
      options.body = JSON.stringify(req.body);
    }

    const response = await fetch(targetUrl, options);
    
    // Support streaming for SSE paths (like /events or /chat)
    if (response.headers.get('content-type')?.includes('text/event-stream')) {
      res.setHeader('Content-Type', 'text/event-stream');
      res.setHeader('Cache-Control', 'no-cache');
      res.setHeader('Connection', 'keep-alive');
      const reader = response.body?.getReader();
      if (reader) {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          res.write(value);
        }
      }
      res.end();
      return;
    }

    const data = await response.json().catch(() => null);
    res.status(response.status).json(data);
  } catch (error) {
    console.error(`[Gateway] Proxy error for ${targetUrl}:`, error);
    res.status(502).json({ 
      error: 'Proxy Error',
      details: error instanceof Error ? error.message : String(error)
    });
  }
});

// Start Server
// ═════════════════════════════════════════════════════════════════════════════
app.listen(PORT, () => {
  console.log(`[Gateway] Running on http://127.0.0.1:${PORT}`);
  console.log(`[Gateway] Health: http://127.0.0.1:${PORT}/health`);
  console.log(`[Gateway] Tools: http://127.0.0.1:${PORT}/api/v1/gateway/tool`);
});
