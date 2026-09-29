import { Command } from 'commander';
import { SubsClient, followTaskEvents, type SseEvent } from '../subs/client.js';
import { randomUUID } from 'node:crypto';
import { userInfo } from 'node:os';

type GlobalOptions = { json?: boolean };

interface TaskRecord {
  task_id: string;
  status: string;
  [key: string]: unknown;
}

const TERMINAL_KINDS = new Set(['done', 'error', 'needs_user']);

function printEvent(command: Command, ev: SseEvent, parsed: unknown): void {
  const json = command.optsWithGlobals<GlobalOptions>().json;
  if (json) {
    process.stdout.write(`${JSON.stringify({ kind: ev.event, payload: parsed })}\n`);
    return;
  }
  const p = (parsed ?? {}) as Record<string, unknown>;
  switch (ev.event) {
    case 'submitted':
      process.stdout.write(`submitted → ${p.provider_thread_id ?? '(no thread id)'}\n`);
      break;
    case 'progress':
      process.stdout.write(`progress: ${p.label}${typeof p.fraction === 'number' ? ` (${Math.round(p.fraction * 100)}%)` : ''}\n`);
      break;
    case 'progress.heartbeat':
      process.stdout.write(`… working (${Math.round(Number(p.elapsed_s ?? 0))}s)\n`);
      break;
    case 'reply': {
      const inner = (p.event ?? {}) as Record<string, unknown>;
      if (inner.type === 'reply.text.delta') process.stdout.write(String(inner.delta ?? ''));
      break;
    }
    case 'artifact.ready':
      process.stdout.write(`\nartifact ready: ${JSON.stringify(p.ref)}\n`);
      break;
    case 'needs_user':
      process.stdout.write(`NEEDS YOU: ${p.message ?? p.status_detail ?? ''}\n`);
      break;
    case 'done':
      process.stdout.write(`\ndone (${String(p.outcome ?? 'success')})\n`);
      break;
    case 'error':
      process.stdout.write(`\nerror: ${JSON.stringify(p.error ?? p)}\n`);
      break;
    default:
      break;
  }
}

export function createTaskCommand(): Command {
  const task = new Command('task').description('Run subscription fabric tasks');

  task.addCommand(
    new Command('run')
      .description('Submit a capability task to the subscription gateway')
      .argument('<capability>', 'capability id, e.g. chat.create')
      .requiredOption('--prompt <prompt>', 'task prompt')
      .option('--provider <provider>', 'provider id or auto', 'auto')
      .option('--wait', 'follow the event stream until a terminal state', false)
      .action(async function (this: Command, capability: string, options: { prompt: string; provider: string; wait?: boolean }) {
        const client = new SubsClient();
        try {
          // D16 — the person typing this command is the human act behind the task.
          const created = await client.requestOk<TaskRecord>('POST', '/v1/tasks', {
            initiated_by: { kind: 'human', user_id: userInfo().username, action_id: `cli-${randomUUID()}` },
            capability,
            prompt: options.prompt,
            ...(options.provider !== 'auto' ? { routing: { provider: options.provider } } : {}),
          });
          if (!options.wait) {
            output(this, created);
            return;
          }
          process.stderr.write(`task ${created.task_id} — following events…\n`);
          const { acked } = await followTaskEvents(
            client,
            created.task_id,
            (ev, parsed) => printEvent(this, ev, parsed),
            (ev, parsed) => {
              if (TERMINAL_KINDS.has(ev.event)) return true;
              if (ev.event === 'task.status') {
                const status = (parsed as { status?: string })?.status;
                return status === 'completed' || status === 'partial' || status === 'failed' || status === 'needs_user';
              }
              return false;
            },
          );
          process.stderr.write(`(acked ${acked} events)\n`);
          output(this, await client.requestOk<TaskRecord>('GET', `/v1/tasks/${created.task_id}`));
        } catch (error) {
          process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
          process.exitCode = 1;
        }
      }),
  );

  return task;
}

function output(command: Command, value: unknown): void {
  const json = command.optsWithGlobals<GlobalOptions>().json;
  process.stdout.write(`${JSON.stringify(value, null, json ? 2 : 2)}\n`);
}

export const taskCommand = createTaskCommand();
