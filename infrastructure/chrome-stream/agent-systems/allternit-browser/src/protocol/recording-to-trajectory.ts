import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join, resolve, sep } from 'node:path';
import {
  BrowserTrajectorySchema,
  COMPUTER_USE_PROTOCOL_VERSION,
  type ActionKind,
  type BrowserTrajectory,
} from '@allternit/computer-use-protocol';

/**
 * Adapter from the ACU engine's JSONL recording format
 * (domains/computer-use/core/core/action_recorder.py) to the
 * BrowserTrajectory shape consumed by compileBrowserTrajectoryToSkill.
 *
 * Recording format (one JSON object per line):
 *   line 0  — RecordingManifest  { _type: "manifest", recording_id, task,
 *                                 session_id, run_id, vision_provider,
 *                                 adapter_id, started_at, ... }
 *   line 1+ — RecordedFrame      { step, timestamp, action_type,
 *                                 action_target, action_params, reasoning,
 *                                 reflection, action_succeeded, ... }
 */

export interface AcuRecordingManifest {
  recording_id?: string;
  task?: string;
  session_id?: string;
  run_id?: string;
  vision_provider?: string;
  adapter_id?: string;
  started_at?: string;
  status?: string;
}

export interface AcuRecordedFrame {
  step?: number;
  timestamp?: string;
  action_type?: string;
  action_target?: string;
  action_params?: Record<string, unknown>;
  reasoning?: string;
  reflection?: string;
  action_succeeded?: boolean;
}

export interface AcuRecordingToTrajectoryOptions {
  /** Provider override; otherwise inferred from manifest.vision_provider. */
  provider?: BrowserTrajectory['provider'];
}

const DEFAULT_RECORDINGS_DIR = join(homedir(), '.allternit', 'recordings');

/**
 * ACU action_type → protocol ActionKind. ACU's vocabulary is broader
 * (desktop + browser); unmappable actions fall back to 'extract' with the
 * original type preserved in input.__acuActionType so no data is lost.
 */
const ACU_ACTION_KIND_MAP: Record<string, ActionKind> = {
  goto: 'navigate',
  navigate: 'navigate',
  click: 'click',
  mouse_click: 'click',
  double_click: 'click',
  right_click: 'click',
  type: 'type',
  type_text: 'type',
  press: 'press',
  key_press: 'press',
  press_key: 'press',
  hotkey: 'press',
  scroll: 'scroll',
  hover: 'hover',
  wait: 'wait',
  sleep: 'wait',
  select: 'select',
  select_option: 'select',
  screenshot: 'screenshot',
  capture_region: 'screenshot',
  extract: 'extract',
  eval: 'extract',
  observe: 'extract',
  dialog_accept: 'dialog.accept',
  dialog_dismiss: 'dialog.dismiss',
  download: 'download',
  file_upload: 'file.upload',
  upload_file: 'file.upload',
  tab_open: 'tab.open',
  new_tab: 'tab.open',
  tab_close: 'tab.close',
  close_tab: 'tab.close',
  tab_focus: 'tab.focus',
  switch_tab: 'tab.focus',
};

export function mapAcuActionKind(actionType: string): ActionKind {
  const mapped = ACU_ACTION_KIND_MAP[actionType.trim().toLowerCase()];
  if (mapped) return mapped;
  return 'extract';
}

function mapAcuProvider(visionProvider: string | undefined): BrowserTrajectory['provider'] {
  const value = (visionProvider ?? '').toLowerCase();
  if (value.includes('playwright')) return 'local-playwright';
  if (value.includes('extension')) return 'extension-tab';
  if (value.includes('stagehand')) return 'stagehand';
  return 'local-playwright';
}

/**
 * Normalize a Python/JS ISO timestamp to the Z-suffixed form Zod's
 * z.string().datetime() requires (e.g. "2026-09-08T12:00:00+00:00" → "...Z").
 * Falls back to the provided fallback (already normalized) when unparseable.
 */
export function normalizeIsoTimestamp(value: string | undefined, fallback: string): string {
  if (value) {
    const millis = Date.parse(value);
    if (Number.isFinite(millis)) return new Date(millis).toISOString();
  }
  return fallback;
}

/**
 * Parse ACU recording JSONL text into a BrowserTrajectory. Frames whose
 * action_succeeded is false become 'failed' steps; everything else is
 * 'committed' (compileBrowserTrajectoryToSkill only compiles committed
 * steps, so failed steps are naturally excluded from the skill package).
 */
export function acuRecordingJsonlToTrajectory(
  jsonl: string,
  options: AcuRecordingToTrajectoryOptions = {},
): BrowserTrajectory {
  const lines = jsonl.split('\n').map((line) => line.trim()).filter(Boolean);
  if (lines.length === 0) {
    throw new Error('ACU recording is empty (no JSONL lines)');
  }

  let manifest: AcuRecordingManifest = {};
  const frames: AcuRecordedFrame[] = [];
  for (const [index, line] of lines.entries()) {
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(line) as Record<string, unknown>;
    } catch {
      throw new Error(`ACU recording line ${index + 1} is not valid JSON`);
    }
    if (index === 0 && parsed._type === 'manifest') {
      manifest = parsed as unknown as AcuRecordingManifest;
    } else if (typeof parsed.action_type === 'string' || typeof parsed.step === 'number') {
      frames.push(parsed as unknown as AcuRecordedFrame);
    }
  }

  const recordingId = manifest.recording_id ?? 'unknown';
  const runId = manifest.run_id || `${recordingId}_run`;
  const sessionId = manifest.session_id || `${recordingId}_session`;
  const createdAt = normalizeIsoTimestamp(manifest.started_at, new Date(0).toISOString());

  return BrowserTrajectorySchema.parse({
    schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
    trajectoryId: `trajectory_${recordingId}`,
    runId,
    sessionId,
    objective: manifest.task || `Recording ${recordingId}`,
    createdAt,
    provider: options.provider ?? mapAcuProvider(manifest.vision_provider),
    steps: frames.map((frame, index) => {
      const actionType = frame.action_type ?? '';
      const kind = mapAcuActionKind(actionType);
      const params = frame.action_params ?? {};
      const input = kind === 'extract' && actionType && ACU_ACTION_KIND_MAP[actionType.trim().toLowerCase()] === undefined
        ? { ...params, __acuActionType: actionType }
        : params;
      const reason = [frame.reasoning, frame.reflection]
        .filter((part): part is string => typeof part === 'string' && part.trim().length > 0)
        .join(' — ') || `Step ${frame.step ?? index + 1}: ${actionType}`;
      return {
        stepId: `step_${frame.step ?? index + 1}`,
        status: frame.action_succeeded === false ? 'failed' : 'committed',
        action: {
          schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
          actionId: `action_${recordingId}_${frame.step ?? index + 1}`,
          runId,
          sessionId,
          kind,
          reason,
          targetDescription: frame.action_target || undefined,
          input,
        },
      };
    }),
  });
}

/**
 * Resolve a recording id or path to a JSONL file inside the recordings dir.
 * Rejects paths that escape the recordings dir.
 */
export function resolveRecordingPath(
  recordingIdOrPath: string,
  recordingsDir: string = DEFAULT_RECORDINGS_DIR,
): string {
  const candidate = recordingIdOrPath.endsWith('.jsonl')
    ? recordingIdOrPath
    : `${recordingIdOrPath}.jsonl`;
  const full = resolve(recordingsDir, candidate);
  const root = resolve(recordingsDir);
  if (full !== root && !full.startsWith(root + sep)) {
    throw new Error(`Recording path escapes the recordings directory: ${recordingIdOrPath}`);
  }
  return full;
}

/** Load an ACU recording from disk and convert it to a BrowserTrajectory. */
export async function loadAcuRecordingToTrajectory(
  recordingIdOrPath: string,
  options: AcuRecordingToTrajectoryOptions & { recordingsDir?: string } = {},
): Promise<BrowserTrajectory> {
  const { recordingsDir, ...trajectoryOptions } = options;
  const path = resolveRecordingPath(recordingIdOrPath, recordingsDir);
  const jsonl = await readFile(path, 'utf8');
  return acuRecordingJsonlToTrajectory(jsonl, trajectoryOptions);
}
