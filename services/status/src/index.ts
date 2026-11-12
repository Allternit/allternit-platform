// status.allternit.com Worker entry: cron probes + state machine, public JSON + page, HMAC ingest, admin API.
// Logic lives in app.ts; this module exports only the handler (workerd rejects other named exports).
import { type Env, handleFetch, runTick } from "./app";

export default {
  fetch(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    return handleFetch(req, env, ctx);
  },
  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    ctx.waitUntil(
      runTick(env, new Date(controller.scheduledTime)).then(
        (s) => {
          if (s.transitions.length || s.alerts || s.rollup) console.log(JSON.stringify({ tick: s }));
        },
        (e) => console.error(`tick failed: ${e instanceof Error ? e.stack : String(e)}`),
      ),
    );
  },
};
