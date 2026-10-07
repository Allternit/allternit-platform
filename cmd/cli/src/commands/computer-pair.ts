/**
 * `allternit computers pair <code>` and `allternit computers serve`: make
 * this machine one of your remote computers (ACI P4). It joins your private
 * Allternit mesh with `mesh-node --forward 5900`, so your other devices can
 * see and take over its screen through its own VNC server (macOS Screen
 * Sharing, or wayvnc/x11vnc on Linux). Nothing is exposed outside the mesh.
 *
 * pair  — checks VNC and mesh-node first (the code is single use), redeems
 *         the code with the cloud, saves the credentials (0600), installs a
 *         user service that runs `serve`, and waits for the first report.
 * serve — the service: keeps mesh-node running and reports the mesh address
 *         and whether VNC is up, every minute.
 */
import { Command } from 'commander';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import { connect } from 'node:net';
import { homedir, hostname, platform } from 'node:os';
import path from 'node:path';

export const VNC_PORT = 5900;
/**
 * The Allternit Factory engine's peer port: other computers' engines run bots
 * here through it (`allternit-factory serve --peer-port 3019`). Forwarded over
 * the mesh beside VNC; the engine only accepts calls with a valid peer ticket.
 */
export const FACTORY_PEER_PORT = 3019;
const REPORT_EVERY_MS = 60_000;

export interface PairedConfig {
  computerId: string;
  secret: string;
  cloudUrl: string;
  name: string;
  controlUrl: string;
  /** Single-use: only needed until the first join; mesh state keeps the identity. */
  authKey?: string;
  meshNode: string;
  /**
   * Local port of this machine's VNC server when it isn't 5900 (e.g. a
   * server whose 5900 belongs to something else). The mesh side is always
   * 5900, which is where your devices look for it.
   */
  vncPort?: number;
}

function localVncPort(config: Pick<PairedConfig, 'vncPort'>): number {
  return config.vncPort ?? VNC_PORT;
}

export function stateDir(): string {
  return path.join(homedir(), '.allternit', 'computer');
}

export function configPath(): string {
  return path.join(stateDir(), 'paired.json');
}

export function cloudUrl(): string {
  return (process.env.ALLTERNIT_CLOUD_URL ?? 'https://api.allternit.com').replace(/\/$/, '');
}

/** Resolves when something accepts connections on 127.0.0.1:port. */
export function portOpen(port: number, host = '127.0.0.1', timeoutMs = 1500): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = connect({ port, host });
    const done = (open: boolean) => {
      socket.destroy();
      resolve(open);
    };
    socket.setTimeout(timeoutMs, () => done(false));
    socket.once('connect', () => done(true));
    socket.once('error', () => done(false));
  });
}

export function vncHelp(os = platform()): string {
  if (os === 'darwin') {
    return 'Turn on Screen Sharing: System Settings → General → Sharing → Screen Sharing.';
  }
  if (os === 'linux') {
    return 'Start a VNC server on port 5900, e.g. `wayvnc 127.0.0.1 5900` (Wayland) or `x11vnc -localhost -rfbport 5900 -forever` (X11).';
  }
  return 'Start a VNC server on port 5900.';
}

/** mesh-node from --mesh-node, ALLTERNIT_MESH_NODE_BIN, PATH, or Allternit Desktop. */
export function findMeshNode(explicit?: string, env: NodeJS.ProcessEnv = process.env, exists: (p: string) => boolean = existsSync): string | null {
  const candidates = [
    explicit,
    env.ALLTERNIT_MESH_NODE_BIN,
    ...(env.PATH ?? '').split(path.delimiter).filter(Boolean).map((dir) => path.join(dir, 'mesh-node')),
    '/Applications/Allternit Desktop.app/Contents/Resources/bin/mesh-node',
  ].filter((p): p is string => Boolean(p));
  return candidates.find((p) => exists(p)) ?? null;
}

export function meshHostname(name: string): string {
  const slug = name.toLowerCase().replace(/[^a-z0-9-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40);
  return slug || 'allternit-computer';
}

async function saveConfig(config: PairedConfig): Promise<void> {
  await mkdir(stateDir(), { recursive: true, mode: 0o700 });
  await writeFile(configPath(), JSON.stringify(config, null, 2), { mode: 0o600 });
  await chmod(configPath(), 0o600);
}

async function loadConfig(): Promise<PairedConfig> {
  return JSON.parse(await readFile(configPath(), 'utf8')) as PairedConfig;
}

async function report(config: PairedConfig, meshIp: string, name?: string): Promise<void> {
  const res = await fetch(`${config.cloudUrl}/api/v1/computers/paired/${encodeURIComponent(config.computerId)}/report`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-allternit-computer-secret': config.secret },
    body: JSON.stringify({ meshIp, vncReady: await portOpen(localVncPort(config)), ...(name ? { name } : {}) }),
  });
  if (!res.ok) throw new Error(`report failed (${res.status})`);
}

/** The user service that keeps this machine on the mesh. */
export function serviceFiles(node: string, cli: string, os = platform()): { path: string; content: string; start: string[][] } | null {
  const logs = path.join(stateDir(), 'serve.log');
  if (os === 'darwin') {
    const plist = path.join(homedir(), 'Library', 'LaunchAgents', 'com.allternit.computer.plist');
    return {
      path: plist,
      content: `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.allternit.computer</string>
  <key>ProgramArguments</key>
  <array><string>${node}</string><string>${cli}</string><string>computers</string><string>serve</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${logs}</string>
  <key>StandardErrorPath</key><string>${logs}</string>
</dict>
</plist>
`,
      start: [['launchctl', 'unload', plist], ['launchctl', 'load', '-w', plist]],
    };
  }
  if (os === 'linux') {
    const unit = path.join(homedir(), '.config', 'systemd', 'user', 'allternit-computer.service');
    return {
      path: unit,
      content: `[Unit]
Description=Allternit remote computer (mesh + reports)
After=network-online.target

[Service]
ExecStart=${node} ${cli} computers serve
Restart=always
RestartSec=5
StandardOutput=append:${logs}
StandardError=append:${logs}

[Install]
WantedBy=default.target
`,
      start: [['systemctl', '--user', 'daemon-reload'], ['systemctl', '--user', 'enable', '--now', 'allternit-computer.service']],
    };
  }
  return null;
}

function runQuiet(args: string[]): Promise<number> {
  return new Promise((resolve) => {
    const child = spawn(args[0], args.slice(1), { stdio: 'ignore' });
    child.on('exit', (code) => resolve(code ?? 1));
    child.on('error', () => resolve(1));
  });
}

async function pairAction(code: string, options: { name?: string; meshNode?: string; service?: boolean; vncPort?: string }): Promise<void> {
  const fail = (message: string) => {
    process.stderr.write(`allternit: ${message}\n`);
    process.exitCode = 1;
  };
  const vncPort = options.vncPort ? Number(options.vncPort) : VNC_PORT;
  if (!Number.isInteger(vncPort) || vncPort < 1 || vncPort > 65535) {
    return fail(`--vnc-port must be a port number. Your code is still unused.`);
  }
  if (!(await portOpen(vncPort))) {
    return fail(`no VNC server on this computer (port ${vncPort}). ${vncHelp()} Then run this again; your code is still unused.`);
  }
  const meshNode = findMeshNode(options.meshNode);
  if (!meshNode) {
    return fail('mesh-node was not found. Install Allternit Desktop, or set ALLTERNIT_MESH_NODE_BIN / pass --mesh-node <path>. Your code is still unused.');
  }
  const name = options.name?.trim() || hostname();
  const res = await fetch(`${cloudUrl()}/api/v1/computers/pair`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ code, name, os: platform() === 'darwin' ? 'macos' : platform() }),
  });
  const body = await res.json().catch(() => ({})) as Record<string, string>;
  if (!res.ok) return fail(body.message ?? body.error ?? `pairing failed (${res.status})`);
  const config: PairedConfig = {
    computerId: body.computerId,
    secret: body.secret,
    controlUrl: body.controlUrl,
    authKey: body.authKey,
    cloudUrl: cloudUrl(),
    name,
    meshNode,
    ...(vncPort !== VNC_PORT ? { vncPort } : {}),
  };
  await saveConfig(config);
  process.stdout.write(`Paired "${name}". Joining your Allternit mesh…\n`);

  const service = options.service === false ? null : serviceFiles(process.execPath, process.argv[1]);
  if (!service) {
    process.stdout.write('Run `allternit computers serve` to keep this computer on the mesh (no service manager set up).\n');
    return;
  }
  await mkdir(path.dirname(service.path), { recursive: true });
  await writeFile(service.path, service.content);
  for (const cmd of service.start) await runQuiet(cmd);
  process.stdout.write(`Installed ${service.path}. It keeps this computer on the mesh and starts at login.\n`);
  process.stdout.write('It shows up in Computers on your other devices within a minute.\n');
}

/**
 * mesh-node arguments: listen on the mesh at 5900, forward to the local VNC
 * port, and (with `peer`) forward the Factory engine's peer port too.
 */
export function meshNodeArgs(config: PairedConfig, options: { peer?: boolean } = {}): string[] {
  return [
    '--hostname', meshHostname(config.name),
    '--control-url', config.controlUrl,
    '--data-dir', path.join(stateDir(), 'mesh'),
    '--forward', String(localVncPort(config)),
    '--listen', String(VNC_PORT),
    ...((options.peer ?? true) ? ['--also', String(FACTORY_PEER_PORT)] : []),
    ...(config.authKey ? ['--auth-key', config.authKey] : []),
  ];
}

/** An older mesh-node (before `--also`) refuses the flag at startup. */
export function meshNodeLacksAlso(stderr: string): boolean {
  return /flag provided but not defined: -also/.test(stderr);
}

class MeshNodeTooOld extends Error {}

/** Start mesh-node and resolve with the mesh IP it reports. */
function startMeshNode(config: PairedConfig, peer: boolean): Promise<{ ip: string; stop: () => void; exited: Promise<number> }> {
  const args = meshNodeArgs(config, { peer });
  const child = spawn(config.meshNode, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const exited = new Promise<number>((resolve) => child.on('exit', (code) => resolve(code ?? 1)));
  return new Promise((resolve, reject) => {
    let buffer = '';
    let errors = '';
    child.stdout.on('data', (chunk: Buffer) => {
      buffer += chunk.toString();
      const match = /MESH_READY ip=(\S+)/.exec(buffer);
      if (match) resolve({ ip: match[1], stop: () => child.kill('SIGTERM'), exited });
    });
    child.stderr.on('data', (chunk: Buffer) => {
      errors += chunk.toString();
      if (!(peer && meshNodeLacksAlso(errors))) process.stderr.write(chunk);
    });
    void exited.then((code) =>
      reject(peer && meshNodeLacksAlso(errors) ? new MeshNodeTooOld('mesh-node is too old to forward the Factory peer port') : new Error(`mesh-node exited (${code}) before joining`)),
    );
  });
}

async function serveAction(): Promise<void> {
  const config = await loadConfig();
  let peer = true;
  for (;;) {
    try {
      const node = await startMeshNode(config, peer);
      if (config.authKey) {
        // Joined: the mesh state now holds the identity; the key was single use.
        delete config.authKey;
        await saveConfig(config);
      }
      process.stdout.write(`On the mesh at ${node.ip}\n`);
      let alive = true;
      void node.exited.then(() => { alive = false; });
      while (alive) {
        await report(config, node.ip).catch((error) => process.stderr.write(`${String(error)}\n`));
        await new Promise((resolve) => setTimeout(resolve, REPORT_EVERY_MS));
      }
    } catch (error) {
      if (error instanceof MeshNodeTooOld) {
        // VNC still works; Factory peer calls wait for a newer mesh-node
        // (it ships with Allternit Desktop).
        process.stderr.write('mesh-node is too old to forward the Factory peer port; update Allternit Desktop to run bots here from other computers. VNC is unaffected.\n');
        peer = false;
        continue;
      }
      process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    }
    await new Promise((resolve) => setTimeout(resolve, 5000));
  }
}

export function pairCommand(): Command {
  return new Command('pair')
    .description('Make this machine one of your remote computers (joins your private Allternit mesh)')
    .argument('<code>', 'pairing code from Computers → Pair a remote computer')
    .option('--name <name>', 'how this computer is listed (default: host name)')
    .option('--mesh-node <path>', 'path to the mesh-node binary')
    .option('--no-service', "don't install a login service; run `allternit computers serve` yourself")
    .option('--vnc-port <port>', 'local port of this machine\'s VNC server when it isn\'t 5900 (your devices still reach it at 5900 on the mesh)')
    .action((code: string, options: { name?: string; meshNode?: string; service?: boolean; vncPort?: string }) => pairAction(code, options));
}

export function serveCommand(): Command {
  return new Command('serve')
    .description('Keep this paired computer on the mesh and report its status (run by the login service)')
    .action(() => serveAction());
}
