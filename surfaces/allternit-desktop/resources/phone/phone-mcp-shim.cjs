#!/usr/bin/env node
// MCP stdio shim for Allternit phone tools. Bots reach the phone through the
// native MCP host; this process just forwards to the Desktop's loopback phone
// gateway (url + token in ~/.allternit/phone-gateway.json, written by Desktop).
'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');
const readline = require('readline');

const GATEWAY_FILE = process.env.ALLTERNIT_PHONE_GATEWAY_FILE || path.join(os.homedir(), '.allternit', 'phone-gateway.json');

function gateway() {
  try {
    return JSON.parse(fs.readFileSync(GATEWAY_FILE, 'utf8'));
  } catch {
    return null;
  }
}

async function forward(pathname, init) {
  const gw = gateway();
  if (!gw) throw new Error('Allternit Desktop is not running, so the phone is unreachable.');
  const res = await fetch(gw.url + pathname, {
    ...init,
    headers: { 'content-type': 'application/json', 'x-allternit-phone-token': gw.token },
  });
  return res.json();
}

function send(msg) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...msg }) + '\n');
}

async function handle(msg) {
  if (msg.id === undefined) return; // notifications
  try {
    if (msg.method === 'initialize') {
      return send({ id: msg.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: 'allternit-phone', version: '1.0.0' } } });
    }
    if (msg.method === 'tools/list') {
      const { tools } = await forward('/phone/tools', { method: 'GET' });
      return send({ id: msg.id, result: { tools } });
    }
    if (msg.method === 'tools/call') {
      const result = await forward('/phone/call', { method: 'POST', body: JSON.stringify({ name: msg.params.name, arguments: msg.params.arguments || {} }) });
      if (result.ok && result.mimeType === 'image/png') {
        return send({ id: msg.id, result: { content: [{ type: 'image', data: result.dataBase64, mimeType: 'image/png' }] } });
      }
      return send({ id: msg.id, result: { isError: !result.ok, content: [{ type: 'text', text: JSON.stringify(result) }] } });
    }
    send({ id: msg.id, error: { code: -32601, message: 'Method not found' } });
  } catch (error) {
    send({ id: msg.id, error: { code: -32000, message: error.message } });
  }
}

readline.createInterface({ input: process.stdin }).on('line', (line) => {
  try {
    handle(JSON.parse(line));
  } catch {
    /* ignore garbage */
  }
});
