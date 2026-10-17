import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  AOrb,
  BlockField,
  BlockWord,
  Burst,
  CheckIcon,
  ChevronIcon,
} from "@/components/onboarding/art";
import { usePrefersReducedMotion } from "@/components/onboarding/useReducedMotion";

/**
 * The sign-in stage: four developer scenes (API, Models, Compute, Runs) that
 * build themselves and loop. Ported 1:1 from the approved prototype
 * (Platform.dc.html). Scenes are illustrations of the console, not live data,
 * so nothing here claims a real request ran.
 *
 * Beat sheet: dwell 4.6 s per scene; next slide pushes in from +48 px over
 * 450 ms cubic-bezier(.32,.72,0,1) while the previous one leaves -36 px with a
 * blur over 320 ms ease-in; pieces pop in on their own --d delays. Manual
 * prev/next/tick/click restarts the dwell. Reduced motion: no autoplay and no
 * travel; prev/next still work.
 */

export const SCENES = [
  {
    name: "API",
    lock: "api" as const,
    lockLabel: "A://TERNIT API",
    title: "One API for local and cloud models.",
    body: "OpenAI-compatible. Point your SDK at api.allternit.com and keep your code.",
  },
  {
    name: "Models",
    lock: "models" as const,
    lockLabel: "A://TERNIT MODELS",
    title: "Every request goes to the right place.",
    body: "Your hardware, Allternit-managed compute or a cloud supplier, on one set of credits.",
  },
  {
    name: "Compute",
    lock: "compute" as const,
    lockLabel: "A://TERNIT COMPUTE",
    title: "Run where it makes sense.",
    body: "Pair your own machines or use Allternit-managed compute. Both show up in one place.",
  },
  {
    name: "Runs",
    lock: "runs" as const,
    lockLabel: "A://TERNIT RUNS",
    title: "Agents that ask before they act.",
    body: "Every run shows its steps. An approval holds the run until you say yes.",
  },
];

export const SCENE_DWELL_MS = 4600;
const STAGE_W = 694;
const STAGE_H = 420;

type Dir = "fw" | "bk";

function d(s: number | string): React.CSSProperties {
  return { ["--d" as string]: typeof s === "number" ? `${s}s` : s } as React.CSSProperties;
}

function ApiScene() {
  return (
    <div className="ob-sc">
      <div className="ob-halo" style={{ left: 420, top: -80, width: 340, height: 340, background: "radial-gradient(circle,rgba(217,119,87,.28),rgba(217,119,87,0) 66%)" }} />
      <div className="ob-halo" style={{ left: -80, top: 200, width: 300, height: 300, background: "radial-gradient(circle,rgba(47,111,224,.14),rgba(47,111,224,0) 66%)" }} />
      <div className="ob-term ob-a" style={{ left: 36, top: 22, width: 622, height: 196, ...d(0.12) }}>
        <div className="ob-term-h"><span /><span /><span /><em>allternit-cloud.sh</em></div>
        <div className="ob-term-b">
          <div className="ob-cl ob-l" style={d(0.35)}><span className="k">curl</span> https://api.allternit.com/v1/chat/completions \</div>
          <div className="ob-cl ob-l" style={d(0.57)}>  -H <span className="s">"Authorization: Bearer $ALLTERNIT_API_KEY"</span> \</div>
          <div className="ob-cl ob-l" style={d(0.79)}>  -H <span className="s">"Content-Type: application/json"</span> \</div>
          <div className="ob-cl ob-l" style={d(1.01)}>  -d <span className="s">'{"{"}"model": "llama-3.3-70b-turbo",</span></div>
          <div className="ob-cl ob-l" style={d(1.23)}><span className="s">       "messages": [{"{"}"role": "user", "content": "Hello"{"}"}]{"}"}'</span></div>
          <div className="ob-cl ob-l" style={d(1.55)}><span className="ob-prompt">$</span> <span className="ob-run ob-press" style={d(1.75)}>⏎ run</span></div>
        </div>
      </div>
      <div className="ob-resp ob-r" style={{ left: 110, top: 206, width: 470, height: 190, ...d(1.95) }}>
        <div className="ob-resp-h"><b>Response</b><span className="ob-200 ob-a" style={d(2.05)}><i />200 · OK</span></div>
        <div className="ob-resp-b">
          <div className="ob-cl ob-l" style={d(2.15)}>{"{"}</div>
          <div className="ob-cl ob-l" style={d(2.28)}>  <span className="k">"id"</span>: <span className="s">"chatcmpl-…"</span>,</div>
          <div className="ob-cl ob-l" style={d(2.41)}>  <span className="k">"model"</span>: <span className="s">"llama-3.3-70b-turbo"</span>,</div>
          <div className="ob-cl ob-l" style={d(2.54)}>  <span className="k">"choices"</span>: [{"{"} <span className="k">"message"</span>: {"{"}</div>
          <div className="ob-cl ob-l" style={d(2.67)}>    <span className="k">"role"</span>: <span className="s">"assistant"</span>,</div>
          <div className="ob-cl ob-l" style={d(2.8)}>    <span className="k">"content"</span>: <span className="s">"Hello! How can I help?"</span> {"}"} {"}"}]</div>
          <div className="ob-cl ob-l" style={d(2.93)}>{"}"}</div>
        </div>
      </div>
      <Burst left={530} top={222} delay={2.05} />
      <div className="ob-tiltcard ob-tilt" style={{ left: 470, top: 262, width: 206, ...d(3.0) }}>
        <small>OpenAI-compatible</small>
        <b>Swap the base URL</b>
        <code>https://api.allternit.com/v1</code>
      </div>
    </div>
  );
}

function ModelsScene() {
  return (
    <div className="ob-sc">
      <div className="ob-halo" style={{ left: -60, top: -60, width: 340, height: 340, background: "radial-gradient(circle,rgba(47,111,224,.20),rgba(47,111,224,0) 66%)" }} />
      <div className="ob-halo" style={{ left: 430, top: 180, width: 300, height: 300, background: "radial-gradient(circle,rgba(217,119,87,.18),rgba(217,119,87,0) 66%)" }} />
      <div className="ob-req ob-a" style={{ left: 172, top: 18, width: 350, ...d(0.1) }}>
        <span className="ob-m">POST</span>/v1/chat/completions<em>model: llama-3.3-70b-turbo</em>
      </div>
      <div className="ob-wire ob-f" style={{ left: 347, top: 118, width: 170, transform: "rotate(140deg)", ...d(0.5) }} />
      <div className="ob-wire ob-f" style={{ left: 347, top: 118, width: 46, transform: "rotate(90deg)", ...d(0.5) }} />
      <div className="ob-wire ob-f" style={{ left: 347, top: 118, width: 170, transform: "rotate(40deg)", ...d(0.5) }} />
      <div className="ob-router ob-a" style={{ left: 323, top: 72, ...d(0.3) }}><AOrb state="searching" /></div>
      <div className="ob-lane ob-a" style={{ left: 30, top: 150, ...d(0.55) }}>
        <b>Your hardware</b><small>Local models on your machines</small><span className="ob-lane-chip">Ready</span>
      </div>
      <div className="ob-lane ob-a ob-lane-lit" style={{ left: 249, top: 150, ...d(0.64) }}>
        <b>Allternit-managed compute</b><small>Managed by Allternit</small><span className="ob-lane-chip">Routed</span>
      </div>
      <div className="ob-lane ob-a" style={{ left: 468, top: 150, ...d(0.73) }}>
        <b>Cloud supplier</b><small>Together AI · Groq · Fireworks</small><span className="ob-lane-chip">Ready</span>
      </div>
      <div className="ob-pkt2" style={{ left: 347, top: 118 }} />
      <Burst left={347} top={192} delay={1.75} />
      <div className="ob-table ob-r" style={{ left: 30, top: 248, width: 634, ...d(0.8) }}>
        <div className="ob-search">Search models, families, or providers...</div>
        <div className="ob-trow h"><span>Model</span><span>Family</span><span>Provider</span><span>Upstream</span></div>
        <div className="ob-trow ob-l" style={d(0.95)}><span className="m">kimi-k3</span><span>Kimi</span><span>Allternit Cloud</span><span>Kimi</span></div>
        <div className="ob-trow ob-l on" style={d(1.05)}><span className="m">llama-3.3-70b-turbo</span><span>Llama</span><span>Allternit</span><span>Together AI</span></div>
        <div className="ob-trow ob-l" style={d(1.15)}><span className="m">deepseek-chat</span><span>DeepSeek</span><span>Allternit</span><span>DeepSeek</span></div>
        <div className="ob-trow ob-l" style={d(1.25)}><span className="m">Local model</span><span>Your pick</span><span>Your hardware</span><span>—</span></div>
      </div>
    </div>
  );
}

function ComputeCard({
  left,
  mono,
  color,
  title,
  sub,
  delay,
  cpu,
  mem,
}: {
  left: number;
  mono: string;
  color: string;
  title: string;
  sub: string;
  delay: number;
  cpu: number;
  mem: number;
}) {
  return (
    <div className="ob-cc ob-r" style={{ left, top: 26, ...d(delay) }}>
      <div className="ob-cc-h"><span className="ob-cc-m" style={{ background: color }}>{mono}</span><b>{title}</b></div>
      <small>{sub}</small>
      <div className="ob-meter"><span>CPU</span><i><b className="ob-g" style={{ ...d(delay + 0.75), width: `${cpu}%` }} /></i></div>
      <div className="ob-meter"><span>Memory</span><i><b className="ob-g" style={{ ...d(delay + 0.85), width: `${mem}%` }} /></i></div>
      <div className="ob-stat">
        <span className="ob-stat-a ob-out" style={d(delay + 0.95)}>Connecting…</span>
        <span className="ob-stat-b ob-a" style={d(delay + 0.95)}><i />Online</span>
      </div>
    </div>
  );
}

function ComputeScene() {
  return (
    <div className="ob-sc">
      <div className="ob-halo" style={{ left: 180, top: 120, width: 380, height: 320, background: "radial-gradient(ellipse,rgba(34,160,107,.20),rgba(34,160,107,0) 66%)" }} />
      <div className="ob-halo" style={{ left: 470, top: -70, width: 280, height: 280, background: "radial-gradient(circle,rgba(217,119,87,.16),rgba(217,119,87,0) 66%)" }} />
      <ComputeCard left={30} mono="H" color="#2F6FE0" title="Your hardware" sub="Paired device · local" delay={0.25} cpu={38} mem={52} />
      <div className="ob-ring" style={{ left: 130, top: 180, ...d(1.2) }} />
      <ComputeCard left={247} mono="M" color="#D97757" title="Allternit-managed compute" sub="Managed by Allternit" delay={0.4} cpu={52} mem={43} />
      <div className="ob-ring" style={{ left: 347, top: 180, ...d(1.55) }} />
      <ComputeCard left={464} mono="C" color="#22A06B" title="Cloud supplier" sub="Together AI · Groq · Fireworks" delay={0.55} cpu={66} mem={34} />
      <div className="ob-ring" style={{ left: 564, top: 180, ...d(1.9) }} />
      <div className="ob-pair ob-r" style={{ left: 30, top: 254, width: 420, ...d(0.7) }}>
        <span className="ob-prompt">$</span> allternit pair <span className="s">&lt;code&gt;</span>
        <span className="ob-paired ob-a" style={d(1.45)}><CheckIcon />Paired</span>
      </div>
      <Burst left={400} top={276} delay={1.45} ring={false} />
      <div className="ob-toast ob-r" style={{ left: 300, top: 330, ...d(2.6) }}>
        <AOrb state="idle" />
        <b>3 places ready</b>
        <span>Workloads go where you allow them to run.</span>
      </div>
    </div>
  );
}

function RunsScene() {
  const steps: Array<[string, "done" | "wait" | "todo", string]> = [
    ["Queued", "done", "✓"],
    ["Read workspace", "done", "✓"],
    ["Call model · kimi-k3", "done", "✓"],
    ["Draft report", "done", "✓"],
    ["Approval needed", "wait", "II"],
    ["Send to #launch", "todo", "·"],
  ];
  return (
    <div className="ob-sc">
      <div className="ob-halo" style={{ left: 400, top: 60, width: 340, height: 340, background: "radial-gradient(circle,rgba(124,92,240,.18),rgba(124,92,240,0) 66%)" }} />
      <div className="ob-halo" style={{ left: -60, top: -60, width: 300, height: 300, background: "radial-gradient(circle,rgba(217,119,87,.18),rgba(217,119,87,0) 66%)" }} />
      <div className="ob-runh ob-a" style={{ left: 30, top: 20, width: 634, ...d(0.1) }}>
        <b>launch-report</b>
        <code>allternit run --workspace ./project</code>
        <span className="ob-mode">cloud</span>
        <span className="ob-rs">
          <span className="ob-rs-a ob-out" style={d(2.75)}><i />Running</span>
          <span className="ob-rs-b ob-a" style={d(2.75)}><i />Completed</span>
        </span>
      </div>
      <div className="ob-prog ob-f" style={{ left: 30, top: 86, width: 634, ...d(0.3) }}><b className="ob-g" style={d(0.4)} /></div>
      <div className="ob-steps" style={{ left: 30, top: 104, width: 360 }}>
        {steps.map(([label, kind, icon], i) => (
          <div key={label} className={`ob-step ob-l ${kind}`} style={d((0.45 + i * 0.18).toFixed(2) + "s")}>
            <i>{icon}</i><span>{label}</span>
          </div>
        ))}
      </div>
      <div className="ob-appr ob-tilt" style={{ left: 420, top: 112, width: 244, ...d(1.45) }}>
        <div className="ob-appr-h"><AOrb state="thinking" /><b>Approval needed</b></div>
        <p>Send the report to <b>#launch</b>?</p>
        <div className="ob-appr-b"><span className="ob-deny">Deny</span><span className="ob-ok ob-press" style={d(2.3)}>Approve</span></div>
        <span className="ob-approved ob-a" style={d(2.45)}><CheckIcon />Approved</span>
      </div>
      <Burst left={608} top={230} delay={2.3} />
      <div className="ob-done ob-r" style={{ left: 30, top: 350, ...d(3.0) }}>
        <CheckIcon /><b>Run completed</b><span>6 steps · 1 approval</span>
      </div>
    </div>
  );
}

const SCENE_COMPONENTS = [ApiScene, ModelsScene, ComputeScene, RunsScene];

/** Scales a fixed-size art board to its container width. */
function useFitScale(designWidth: number) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [scale, setScale] = useState(1);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setScale(Math.min(1, el.clientWidth / designWidth) || 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [designWidth]);
  return { ref, scale };
}

export function ConsoleShowcase() {
  const reduced = usePrefersReducedMotion();
  const [view, setView] = useState<{ slide: number; prev: number; dir: Dir }>({ slide: 0, prev: -1, dir: "fw" });
  // Bumped on every manual move so the dwell timer (and tick fill) restarts.
  const [epoch, setEpoch] = useState(0);
  const { ref, scale } = useFitScale(STAGE_W);
  const { slide, prev, dir } = view;

  const go = useCallback((to: number, nextDir: Dir, manual: boolean) => {
    setView((cur) => {
      const target = (to + SCENES.length) % SCENES.length;
      return target === cur.slide ? cur : { slide: target, prev: cur.slide, dir: nextDir };
    });
    if (manual) setEpoch((e) => e + 1);
  }, []);

  useEffect(() => {
    if (reduced) return;
    const t = window.setInterval(() => {
      if (typeof document !== "undefined" && document.hidden) return;
      setView((cur) => ({ slide: (cur.slide + 1) % SCENES.length, prev: cur.slide, dir: "fw" }));
    }, SCENE_DWELL_MS);
    return () => window.clearInterval(t);
  }, [reduced, epoch]);

  const cls = (i: number) => (i === slide ? `cur ${dir}` : i === prev ? `out ${dir}` : "");
  const next = () => go(slide + 1, "fw", true);
  const back = () => go(slide - 1, "bk", true);
  const scene = SCENES[slide];

  return (
    <div className="ob-show" data-slide={slide}>
      <div className="ob-lhead">
        <div className="ob-locks" role="img" aria-label={scene.lockLabel}>
          {SCENES.map((s, i) => (
            <div key={s.name} className={`ob-sl ${cls(i)}`}>
              <div className="ob-lock"><BlockWord word={s.lock} /></div>
            </div>
          ))}
        </div>
        <div className="ob-lnav">
          <span className="ob-count"><b>0{slide + 1}</b> / 04</span>
          <button type="button" className="ob-ib" aria-label="Previous preview" onClick={back}><ChevronIcon dir="left" /></button>
          <button type="button" className="ob-ib" aria-label="Next preview" onClick={next}><ChevronIcon dir="right" /></button>
        </div>
      </div>
      <div ref={ref} className="ob-lfit" style={{ height: STAGE_H * scale }}>
        <div className="ob-lstage" style={{ transform: `scale(${scale})` }}>
          <div className="ob-lfield"><BlockField id="ob-field-show" /></div>
          <div className="ob-plinth" />
          {SCENE_COMPONENTS.map((Scene, i) => (
            <div key={i} className={`ob-sl ${cls(i)}`} aria-hidden={i !== slide}>
              {(i === slide || i === prev) && <Scene />}
            </div>
          ))}
          <button
            type="button"
            className="ob-next"
            aria-label={`Show the next preview: ${SCENES[(slide + 1) % SCENES.length].name}`}
            onClick={next}
          />
        </div>
      </div>
      <div className="ob-ticks" role="group" aria-label="Previews">
        {SCENES.map((s, i) => (
          <button
            key={`${s.name}-${i === slide ? epoch : "idle"}`}
            type="button"
            className={`ob-tk ${i === slide ? "on" : ""}`}
            aria-label={`Preview ${i + 1} of 4: ${s.name}`}
            aria-current={i === slide ? "true" : undefined}
            onClick={() => i !== slide && go(i, i > slide ? "fw" : "bk", true)}
          >
            <i />
          </button>
        ))}
      </div>
      <div className="ob-caps" aria-live="polite">
        {SCENES.map((s, i) => (
          <div key={s.name} className={`ob-sl ${cls(i)}`} aria-hidden={i !== slide}>
            <div className="ob-cap">
              <h2 className="ob-ink">{s.title}</h2>
              <p>{s.body}</p>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
