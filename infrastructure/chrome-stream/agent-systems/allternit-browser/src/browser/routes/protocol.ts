import type { Express, Request, Response } from 'express';
import { z } from 'zod';
import {
  ActionIntentSchema,
  ExecutionLeaseSchema,
} from '@allternit/computer-use-protocol';
import type { createBrowserRouteContext } from '../server-context.js';
import { BrowserRunController } from '../../protocol/run-controller.js';
import { LocalPlaywrightProvider } from '../../protocol/local-provider.js';
import { compileBrowserTrajectoryToSkill } from '../../protocol/skill-factory.js';
import { loadAcuRecordingToTrajectory } from '../../protocol/recording-to-trajectory.js';
import { SiteToolRegistry } from '../site-tools/registry.js';
import { loadGitHubSiteTools } from '../site-tools/adapters/github.js';
import { loadGmailSiteTools } from '../site-tools/adapters/gmail.js';
import { loadNotionSiteTools } from '../site-tools/adapters/notion.js';

function buildSiteToolRegistry(): SiteToolRegistry {
  const registry = new SiteToolRegistry();
  registry.registerAll(loadGitHubSiteTools());
  registry.registerAll(loadGmailSiteTools());
  registry.registerAll(loadNotionSiteTools());
  return registry;
}

const StartRunBodySchema = z.object({
  accountId: z.string().min(1).default('local'),
  conversationId: z.string().min(1).default('browser'),
  objective: z.string().min(1),
  provider: z.enum(['local-playwright', 'extension-tab', 'stagehand']).default('local-playwright'),
  startedBy: z.enum(['platform-web', 'desktop', 'gizzi', 'extension', 'api']).default('api'),
  sessionId: z.string().min(1).optional(),
  runId: z.string().min(1).optional(),
  deviceId: z.string().min(1).optional(),
  profile: z.string().min(1).optional(),
  targetId: z.string().min(1).optional(),
});

const ExecuteBodySchema = z.object({
  lease: z.unknown(),
  action: z.unknown(),
});

const CompleteBodySchema = z.object({
  reason: z.string().min(1).optional(),
});

const CompileSkillBodySchema = z.object({
  title: z.string().min(1).optional(),
  description: z.string().min(1).optional(),
  tags: z.array(z.string().min(1)).optional(),
});

const CompileFromRecordingBodySchema = CompileSkillBodySchema.extend({
  recordingId: z.string().min(1).optional(),
  path: z.string().min(1).optional(),
}).refine((body) => body.recordingId || body.path, {
  message: 'Provide either recordingId or path',
});

const localProvider = new LocalPlaywrightProvider();
const siteToolRegistry = buildSiteToolRegistry();
const controller = new BrowserRunController({
  providers: [localProvider],
  sourceSurface: 'api',
  siteTools: siteToolRegistry,
  resolveToolContext: (run) => {
    const binding = localProvider.getBinding(run.sessionId);
    if (!binding) throw new Error(`No local browser binding for session ${run.sessionId}`);
    return { cdpUrl: binding.cdpUrl, targetId: binding.targetId };
  },
});

export function registerBrowserProtocolRoutes(
  app: Express,
  ctx: ReturnType<typeof createBrowserRouteContext>,
): void {
  app.get('/v1/browser-runs/providers', (_req: Request, res: Response) => {
    res.json({ providers: controller.listProviders().map((provider) => provider.capabilities) });
  });

  app.post('/v1/browser-runs', async (req: Request, res: Response) => {
    await route(res, async () => {
      const body = StartRunBodySchema.parse(req.body ?? {});
      const profileCtx = ctx.resolveProfileContext(
        { query: { profile: body.profile }, body: { profile: body.profile } },
        res,
        ctx,
      );
      if (!profileCtx) return;

      const tab = await profileCtx.ensureTabAvailable(body.targetId);
      const started = controller.startRun(body);
      if (body.provider === 'local-playwright') {
        localProvider.bind({
          sessionId: started.session.sessionId,
          cdpUrl: profileCtx.profile.cdpUrl,
          targetId: tab.targetId,
        });
      }
      res.status(201).json({ ...started, tab });
    });
  });

  app.get('/v1/browser-runs/:runId', (req: Request, res: Response) => {
    routeSync(res, () => {
      const run = controller.getRun(req.params.runId);
      if (!run) return res.status(404).json({ error: 'Run not found' });
      res.json({
        run,
        lease: controller.getLease(run.runId),
        session: controller.getSession(run.sessionId),
      });
    });
  });

  app.get('/v1/browser-runs/:runId/events', (req: Request, res: Response) => {
    routeSync(res, () => {
      const after = typeof req.query.after === 'string' ? Number(req.query.after) : 0;
      res.json({ events: controller.eventsAfter(req.params.runId, Number.isFinite(after) ? after : 0) });
    });
  });

  app.post('/v1/browser-runs/:runId/observe', async (req: Request, res: Response) => {
    await route(res, async () => {
      const result = await controller.observe(req.params.runId);
      res.json(result);
    });
  });

  app.post('/v1/browser-runs/:runId/actions', async (req: Request, res: Response) => {
    await route(res, async () => {
      const body = ExecuteBodySchema.parse(req.body ?? {});
      const lease = ExecutionLeaseSchema.parse(body.lease);
      const action = ActionIntentSchema.parse(body.action);
      if (action.runId !== req.params.runId) {
        res.status(400).json({ error: 'Action runId does not match route runId' });
        return;
      }
      const result = await controller.execute({ lease, action });
      res.json(result);
    });
  });

  app.get('/v1/browser-runs/:runId/tools', (req: Request, res: Response) => {
    routeSync(res, () => {
      if (!controller.getRun(req.params.runId)) return res.status(404).json({ error: 'Run not found' });
      res.json(controller.availableTools(req.params.runId));
    });
  });

  app.post('/v1/browser-runs/:runId/tools/:toolName', async (req: Request, res: Response) => {
    await route(res, async () => {
      const body = z.object({
        lease: z.unknown(),
        args: z.record(z.string(), z.unknown()).default({}),
      }).parse(req.body ?? {});
      const lease = ExecutionLeaseSchema.parse(body.lease);
      if (lease.runId !== req.params.runId) {
        res.status(400).json({ error: 'Lease runId does not match route runId' });
        return;
      }
      const result = await controller.executeTool({
        lease,
        toolName: req.params.toolName,
        args: body.args,
      });
      res.json(result);
    });
  });

  app.post('/v1/browser-runs/:runId/video/start', async (req: Request, res: Response) => {
    await route(res, async () => {
      const body = z.object({
        recordingId: z.string().min(1).optional(),
        size: z.object({ width: z.number().int().positive(), height: z.number().int().positive() }).optional(),
      }).parse(req.body ?? {});
      const run = controller.getRun(req.params.runId);
      if (!run) {
        res.status(404).json({ error: 'Run not found' });
        return;
      }
      const binding = localProvider.getBinding(run.sessionId);
      if (!binding) {
        res.status(400).json({ error: `No local browser binding for session ${run.sessionId}` });
        return;
      }
      const started = await localProvider.startRecordedSession({
        sessionId: run.sessionId,
        cdpUrl: binding.cdpUrl,
        recordingId: body.recordingId ?? run.runId,
        size: body.size,
      });
      controller.recordArtifact(run.runId, {
        kind: 'video_recording_started',
        startedAtEpoch: started.startedAtEpoch,
        targetId: started.binding.targetId,
      });
      res.json(started);
    });
  });

  app.post('/v1/browser-runs/:runId/video/stop', async (req: Request, res: Response) => {
    await route(res, async () => {
      const run = controller.getRun(req.params.runId);
      if (!run) {
        res.status(404).json({ error: 'Run not found' });
        return;
      }
      const video = await localProvider.stopRecordedSession(run.sessionId);
      if (!video) {
        res.status(400).json({ error: `No video recording active for session ${run.sessionId}` });
        return;
      }
      controller.recordArtifact(run.runId, { kind: 'video', ...video });
      res.json({ video });
    });
  });

  app.post('/v1/browser-runs/:runId/complete', (req: Request, res: Response) => {
    routeSync(res, () => {
      const _body = CompleteBodySchema.parse(req.body ?? {});
      res.json(controller.completeRun(req.params.runId));
    });
  });

  app.post('/v1/browser-runs/:runId/cancel', (req: Request, res: Response) => {
    routeSync(res, () => {
      const body = CompleteBodySchema.parse(req.body ?? {});
      res.json(controller.cancelRun(req.params.runId, body.reason ?? 'cancelled by caller'));
    });
  });

  app.post('/v1/browser-runs/:runId/skill', (req: Request, res: Response) => {
    routeSync(res, () => {
      const body = CompileSkillBodySchema.parse(req.body ?? {});
      const trajectory = controller.toTrajectory(req.params.runId);
      res.json(compileBrowserTrajectoryToSkill(trajectory, body));
    });
  });

  app.post('/v1/browser-skills/from-recording', async (req: Request, res: Response) => {
    await route(res, async () => {
      const body = CompileFromRecordingBodySchema.parse(req.body ?? {});
      const trajectory = await loadAcuRecordingToTrajectory(body.recordingId ?? body.path!);
      const { title, description, tags } = body;
      const pkg = compileBrowserTrajectoryToSkill(trajectory, { title, description, tags });
      res.json({
        ...pkg,
        // Metadata only — the raw trajectory is intentionally omitted because
        // it contains the unredacted recording.
        trajectory: {
          trajectoryId: trajectory.trajectoryId,
          runId: trajectory.runId,
          provider: trajectory.provider,
          objective: trajectory.objective,
          committedSteps: trajectory.steps.filter((step) => step.status === 'committed').length,
        },
      });
    });
  });
}

async function route(res: Response, fn: () => Promise<void>): Promise<void> {
  try {
    await fn();
  } catch (error) {
    sendError(res, error);
  }
}

function routeSync(res: Response, fn: () => void): void {
  try {
    fn();
  } catch (error) {
    sendError(res, error);
  }
}

function sendError(res: Response, error: unknown): void {
  if (res.headersSent) return;
  res.status(400).json({ error: error instanceof Error ? error.message : String(error) });
}
