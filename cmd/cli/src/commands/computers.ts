import { Command } from 'commander';
import { createReadStream } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import { ApiClient } from '../api-client.js';
import { pairCommand, serveCommand } from './computer-pair.js';

type GlobalOptions = { apiUrl: string; token?: string; json?: boolean };

function client(command: Command): ApiClient {
  const options = command.optsWithGlobals<GlobalOptions>();
  return new ApiClient({ apiUrl: options.apiUrl, token: options.token });
}

function output(command: Command, value: unknown): void {
  const json = command.optsWithGlobals<GlobalOptions>().json;
  process.stdout.write(`${JSON.stringify(value, null, json ? 0 : 2)}\n`);
}

async function run(command: Command, request: () => Promise<unknown>): Promise<void> {
  try {
    output(command, await request());
  } catch (error) {
    process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}

/// Authenticated raw fetch for non-JSON planes (file bytes, PNG screenshots).
/// `duplex` is accepted for streaming request bodies (not yet in @types/node 20).
async function raw(command: Command, method: string, path: string, init: RequestInit & { duplex?: 'half' | 'full' } = {}): Promise<Response> {
  const options = command.optsWithGlobals<GlobalOptions>();
  const response = await fetch(`${options.apiUrl.replace(/\/$/, '')}${path}`, {
    ...init,
    method,
    headers: {
      accept: '*/*',
      ...(options.token ? { authorization: `Bearer ${options.token}` } : {}),
      ...(init.headers ?? {}),
    },
  });
  if (!response.ok) {
    const payload = await response.json().catch(() => ({})) as { message?: string; error?: string };
    throw new Error(payload.message ?? payload.error ?? `${response.status} ${response.statusText}`);
  }
  return response;
}

function approvalQuery(approvalId?: string): string {
  return approvalId ? `?approval_id=${encodeURIComponent(approvalId)}` : '';
}

// ── Interactive PTY over WebSocket (ssh) ────────────────────────────────────
//
// Frame format, mirrored from `computer_pty_ws_handler` in
// cmd/allternit-api/src/computer_ws.rs:
//   client -> server:  Binary frame = raw terminal keystrokes (bytes).
//                      Text frame `{"cols":N,"rows":M}` = PTY resize.
//   server -> client:  Binary frame = raw PTY output (bytes).
//                      (Text frames are reserved; malformed text is ignored.)
// Connection: mint a short-lived token via
//   POST /api/v1/computers/:id/ws-token {purpose:"pty"}, then open
//   WS /ws/computers/:id/pty?token=... (http -> ws, https -> wss).
//
// v1 limitations (documented honestly):
//   - Raw stdin/stdout passthrough only: no local line editing, history,
//     tab-completion, or client-side keybindings. Everything goes to the
//     guest bash PTY.
//   - Resize is driven by SIGWINCH, so it works on POSIX terminals; Windows
//     consoles need a VT-capable terminal (e.g. Windows Terminal).
//   - The guest bridge serves one client at a time.

/// Build the pty WebSocket URL from the API base URL and minted token.
export function ptyWsUrl(apiUrl: string, id: string, token: string): string {
  const base = apiUrl.replace(/\/$/, '').replace(/^http/, 'ws');
  return `${base}/ws/computers/${encodeURIComponent(id)}/pty?token=${encodeURIComponent(token)}`;
}

function terminalSize(): { cols: number; rows: number } {
  return { cols: process.stdout.columns || 80, rows: process.stdout.rows || 24 };
}

async function sshSession(command: Command, id: string): Promise<void> {
  const options = command.optsWithGlobals<GlobalOptions>();
  const minted = await client(command).request(
    'POST',
    `/api/v1/computers/${encodeURIComponent(id)}/ws-token`,
    { purpose: 'pty' },
  ) as { token?: string };
  if (!minted?.token) {
    throw new Error('ws-token response did not include a token');
  }
  const ws = new WebSocket(ptyWsUrl(options.apiUrl, id, minted.token));
  ws.binaryType = 'arraybuffer';

  const interactive = Boolean(process.stdin.isTTY && process.stdout.isTTY);
  let raw = false;
  const onStdin = (data: Buffer) => ws.send(data);
  const onResize = () => {
    const { cols, rows } = terminalSize();
    ws.send(JSON.stringify({ cols, rows }));
  };
  const cleanup = () => {
    if (raw) process.stdin.setRawMode(false);
    process.stdin.pause();
    process.stdin.removeListener('data', onStdin);
    process.removeListener('SIGWINCH', onResize);
  };

  await new Promise<void>((resolve, reject) => {
    ws.addEventListener('open', () => {
      if (!interactive) return;
      process.stdin.setRawMode(true);
      raw = true;
      process.stdin.on('data', onStdin);
      process.stdin.resume();
      onResize();
      process.on('SIGWINCH', onResize);
    });
    ws.addEventListener('message', (event) => {
      if (typeof event.data === 'string') return;
      process.stdout.write(Buffer.from(event.data as ArrayBuffer));
    });
    ws.addEventListener('close', () => {
      cleanup();
      resolve();
    });
    ws.addEventListener('error', (event) => {
      cleanup();
      const message = event instanceof ErrorEvent ? event.message : 'connection failed';
      reject(new Error(`pty websocket error: ${message || 'connection failed'}`));
    });
  });
}

async function ssh(command: Command, id: string): Promise<void> {
  try {
    await sshSession(command, id);
  } catch (error) {
    process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}

// ── Command tree ─────────────────────────────────────────────────────────────

const SSH_HELP_LIMITATIONS =
  'v1: raw stdin/stdout passthrough (no local line editing/history; keys go straight to the guest PTY); ' +
  'resize via SIGWINCH (POSIX terminals; use a VT-capable terminal on Windows); one client per computer at a time.';

type LifecycleOptions = { approvalId?: string };

function lifecycleCommand(
  name: string,
  method: 'GET' | 'POST',
  suffix: string,
  description: string,
): Command {
  return new Command(name)
    .description(description)
    .argument('<id>', 'computer id')
    .option('--approval-id <id>', 'action-hash approval grant for gated actions')
    .action(function (this: Command, id: string, options: LifecycleOptions) {
      return run(this, () => client(this).request(
        method,
        `/api/v1/computers/${encodeURIComponent(id)}${suffix}${approvalQuery(options.approvalId)}`,
      ));
    });
}

export function createComputersCommand(): Command {
  const create = new Command('create')
    .description('Create a computer (cloud_desktop, local, or deferred kinds)')
    .requiredOption('--kind <kind>', 'computer kind: local|byo_vps|managed|byoc|cloud_desktop')
    .option('--owner-type <type>', 'owner type: user|org|bot|session')
    .option('--owner-id <id>', 'owner id (must match the authenticated owner)')
    .option('--cpu-cores <n>', 'vCPU cores', Number)
    .option('--memory-mb <n>', 'memory in MB', Number)
    .option('--disk-mb <n>', 'disk in MB', Number)
    .option('--resolution <WxH>', 'desktop resolution, e.g. 1920x1080')
    .option('--name <name>', 'display name')
    .option('--os <os>', 'guest OS, e.g. linux|windows')
    .option('--template-id <id>', 'template id (exactly one of --template-id/--template-ref)')
    .option('--template-ref <ref>', 'curated system/... template ref')
    .option('--bot-id <id>', 'bot owner id (required for cloud_desktop)')
    .option('--session-id <id>', 'session owner id')
    .option('--persistence <mode>', 'persistence: ephemeral|session|persistent')
    .option('--provider <provider>', 'substrate hint: incus (Linux/Windows) or tart (macOS)')
    .option('--approval-id <id>', 'action-hash approval grant for gated actions')
    .action(function (this: Command, options: {
      kind: string;
      ownerType?: string;
      ownerId?: string;
      cpuCores?: number;
      memoryMb?: number;
      diskMb?: number;
      resolution?: string;
      name?: string;
      os?: string;
      templateId?: string;
      templateRef?: string;
      botId?: string;
      sessionId?: string;
      persistence?: string;
      provider?: string;
      approvalId?: string;
    }) {
      return run(this, () => client(this).request(
        'POST',
        `/api/v1/computers${approvalQuery(options.approvalId)}`,
        {
          kind: options.kind,
          owner_type: options.ownerType,
          owner_id: options.ownerId,
          cpu_cores: options.cpuCores,
          memory_mb: options.memoryMb,
          disk_mb: options.diskMb,
          resolution: options.resolution,
          bot_id: options.botId,
          name: options.name,
          os: options.os,
          template_id: options.templateId,
          template_ref: options.templateRef,
          session_id: options.sessionId,
          persistence: options.persistence,
          provider: options.provider,
        },
      ));
    });

  const list = new Command('list')
    .description('List computers visible to the current user/org')
    .option('--bot-id <id>', 'filter by bot id')
    .option('--kind <kind>', 'filter by kind: local|byo_vps|managed|byoc|cloud_desktop')
    .option('--group-id <id>', 'filter by computer group id')
    .option('--include-roles', 'include non-user roles (e.g. golden template-build holders)')
    .action(function (this: Command, options: {
      botId?: string;
      kind?: string;
      groupId?: string;
      includeRoles?: boolean;
    }) {
      const params = new URLSearchParams();
      if (options.botId) params.set('bot_id', options.botId);
      if (options.kind) params.set('kind', options.kind);
      if (options.groupId) params.set('group_id', options.groupId);
      if (options.includeRoles) params.set('include_roles', '1');
      const query = params.size > 0 ? `?${params.toString()}` : '';
      return run(this, () => client(this).request('GET', `/api/v1/computers${query}`));
    });

  const resize = new Command('resize')
    .description('Resize a cloud_desktop computer (disk resize requires a stopped computer)')
    .argument('<id>', 'computer id')
    .option('--cpu-cores <n>', 'vCPU cores', Number)
    .option('--memory-mb <n>', 'memory in MB', Number)
    .option('--disk-mb <n>', 'disk in MB', Number)
    .option('--approval-id <id>', 'action-hash approval grant for gated actions')
    .action(function (this: Command, id: string, options: {
      cpuCores?: number;
      memoryMb?: number;
      diskMb?: number;
      approvalId?: string;
    }) {
      return run(this, () => client(this).request(
        'PATCH',
        `/api/v1/computers/${encodeURIComponent(id)}/resize${approvalQuery(options.approvalId)}`,
        {
          cpu_cores: options.cpuCores,
          memory_mb: options.memoryMb,
          disk_mb: options.diskMb,
        },
      ));
    });

  const clone = new Command('clone')
    .description('Clone a cloud_desktop computer into a new running instance')
    .argument('<id>', 'computer id')
    .option('--name <name>', 'name for the clone')
    .option('--approval-id <id>', 'action-hash approval grant for gated actions')
    .action(function (this: Command, id: string, options: { name?: string; approvalId?: string }) {
      return run(this, () => client(this).request(
        'POST',
        `/api/v1/computers/${encodeURIComponent(id)}/clone${approvalQuery(options.approvalId)}`,
        options.name === undefined ? {} : { name: options.name },
      ));
    });

  const sshCommand = new Command('ssh')
    .description(`Interactive PTY over WebSocket (raw passthrough). ${SSH_HELP_LIMITATIONS}`)
    .argument('<id>', 'computer id')
    .action(function (this: Command, id: string) {
      return ssh(this, id);
    });

  const driveShell = new Command('shell')
    .description('Run a one-shot command in the guest and print its stdout/stderr')
    .argument('<id>', 'computer id')
    .argument('<command...>', 'command and arguments to run in the guest (use -- before flags)')
    .option('--approval-id <id>', 'action-hash approval grant for gated actions')
    .action(async function (this: Command, id: string, argv: string[], options: LifecycleOptions) {
      try {
        const response = await client(this).request(
          'POST',
          `/api/v1/computers/${encodeURIComponent(id)}/shell${approvalQuery(options.approvalId)}`,
          { command: argv },
        ) as { stdout?: string; stderr?: string; exit_code?: number };
        if (response.stdout) process.stdout.write(response.stdout);
        if (response.stderr) process.stderr.write(response.stderr);
        if (typeof response.exit_code === 'number' && response.exit_code !== 0) {
          process.exitCode = response.exit_code;
        }
      } catch (error) {
        process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
        process.exitCode = 1;
      }
    });

  const driveUpload = new Command('upload')
    .description('Upload a local file into the guest (raw octet-stream body)')
    .argument('<id>', 'computer id')
    .argument('<local-path>', 'file on this machine')
    .argument('<guest-path>', 'absolute destination path in the guest')
    .option('--approval-id <id>', 'action-hash approval grant for gated actions')
    .action(async function (this: Command, id: string, localPath: string, guestPath: string, options: LifecycleOptions) {
      try {
        await readFile(localPath); // fail fast on missing/unreadable local files
        await raw(this, 'POST', `/api/v1/computers/${encodeURIComponent(id)}/files/upload?path=${encodeURIComponent(guestPath)}${options.approvalId ? `&approval_id=${encodeURIComponent(options.approvalId)}` : ''}`, {
          headers: { 'content-type': 'application/octet-stream' },
          body: createReadStream(localPath) as unknown as BodyInit,
          duplex: 'half',
        });
        process.stdout.write(`${localPath} -> ${guestPath}\n`);
      } catch (error) {
        process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
        process.exitCode = 1;
      }
    });

  const driveDownload = new Command('download')
    .description('Download a file from the guest to a local path')
    .argument('<id>', 'computer id')
    .argument('<guest-path>', 'absolute path in the guest')
    .argument('<local-path>', 'destination file on this machine')
    .action(async function (this: Command, id: string, guestPath: string, localPath: string) {
      try {
        const response = await raw(this, 'GET', `/api/v1/computers/${encodeURIComponent(id)}/files/download?path=${encodeURIComponent(guestPath)}`);
        await writeFile(localPath, Buffer.from(await response.arrayBuffer()));
        process.stdout.write(`${guestPath} -> ${localPath}\n`);
      } catch (error) {
        process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
        process.exitCode = 1;
      }
    });

  const drive = new Command('drive')
    .description('Drive a computer: one-shot screenshot/click/type, shell, file upload/download')
    .addCommand(createScreenshotCommand(
      'screenshot',
      'Capture the guest screen with optional click/type one-shots (alias of `computers screenshot`)',
    ))
    .addCommand(driveShell)
    .addCommand(driveUpload)
    .addCommand(driveDownload);

type ScreenshotOptions = {
  click?: string;
  doubleClick?: string;
  rightClick?: string;
  type?: string;
  key?: string;
  approvalId?: string;
};

async function screenshotAction(
  command: Command,
  id: string,
  localPath: string | undefined,
  options: ScreenshotOptions,
): Promise<void> {
  try {
    const response = await raw(command, 'GET', `/api/v1/computers/${encodeURIComponent(id)}/screenshot`);
    const png = Buffer.from(await response.arrayBuffer());
    if (localPath) {
      await writeFile(localPath, png);
      process.stdout.write(`${png.byteLength} bytes -> ${localPath}\n`);
    } else {
      process.stdout.write(png);
    }
    const oneShots: Array<{ action: string; x?: number; y?: number; text?: string; key?: string }> = [];
    const parseXY = (value: string): { x: number; y: number } => {
      const [x, y] = value.split(',').map((part) => Number(part.trim()));
      if (!Number.isFinite(x) || !Number.isFinite(y)) {
        throw new Error(`invalid coordinate "${value}" (expected x,y)`);
      }
      return { x, y };
    };
    if (options.click) oneShots.push({ action: 'click', ...parseXY(options.click) });
    if (options.doubleClick) oneShots.push({ action: 'doubleclick', ...parseXY(options.doubleClick) });
    if (options.rightClick) oneShots.push({ action: 'rightclick', ...parseXY(options.rightClick) });
    if (options.type) oneShots.push({ action: 'type', text: options.type });
    if (options.key) oneShots.push({ action: 'key', key: options.key });
    for (const oneShot of oneShots) {
      const isKeyboard = oneShot.action === 'type' || oneShot.action === 'key';
      await client(command).request(
        'POST',
        `/api/v1/computers/${encodeURIComponent(id)}/${isKeyboard ? 'keyboard' : 'mouse'}${approvalQuery(options.approvalId)}`,
        oneShot,
      );
    }
  } catch (error) {
    process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}

function createScreenshotCommand(name: string, description: string): Command {
  return new Command(name)
    .description(description)
    .argument('<id>', 'computer id')
    .argument('[local-path]', 'destination file (omit to write PNG bytes to stdout)')
    .option('--click <x,y>', 'after capture, left-click at x,y (one-shot drive)')
    .option('--double-click <x,y>', 'after capture, double-click at x,y')
    .option('--right-click <x,y>', 'after capture, right-click at x,y')
    .option('--type <text>', 'after capture, type text into the focused widget')
    .option('--key <key>', 'after capture, press one keysym (e.g. Return, Control_L)')
    .option('--approval-id <id>', 'action-hash approval grant for the gated one-shot input')
    .action(function (this: Command, id: string, localPath: string | undefined, options: ScreenshotOptions) {
      return screenshotAction(this, id, localPath, options);
    });
}

  return new Command('computers')
    .alias('computer')
    .description('Manage Allternit computers (cloud desktops, local VMs, your paired machines)')
    .addCommand(pairCommand())
    .addCommand(serveCommand())
    .addCommand(create)
    .addCommand(list)
    .addCommand(lifecycleCommand('get', 'GET', '', 'Get a computer by id'))
    .addCommand(lifecycleCommand('start', 'POST', '/start', 'Start a stopped computer'))
    .addCommand(lifecycleCommand('stop', 'POST', '/stop', 'Stop a running computer'))
    .addCommand(lifecycleCommand('restart', 'POST', '/restart', 'Restart a computer'))
    .addCommand(lifecycleCommand('delete', 'POST', '/delete', 'Delete a computer (irreversible)'))
    .addCommand(resize)
    .addCommand(clone)
    .addCommand(sshCommand)
    .addCommand(drive)
    .addCommand(createScreenshotCommand(
      'screenshot',
      'Capture the guest screen (PNG bytes) to a file or stdout, with optional click/type one-shots',
    ));
}

export const computersCommand = createComputersCommand();
