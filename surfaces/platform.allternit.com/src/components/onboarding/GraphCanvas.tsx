import React, { useLayoutEffect, useRef, useState } from "react";
import { GraphOrb } from "@/components/onboarding/art";
import { edgeFrom } from "@/lib/platform-setup";

export interface GraphNode {
  id: string;
  label: string;
  x: number;
  y: number;
  /** Set up (solid edge, green dot) vs available (dashed). */
  lit: boolean;
  focus?: boolean;
  status?: string;
  onPick?: () => void;
}

/** Scales a fixed art board to fit its box (both axes), centred. */
export function useFit(designW: number, designH: number) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [fit, setFit] = useState({ scale: 1, dx: 0, dy: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const w = el.clientWidth;
      const h = el.clientHeight || designH;
      const scale = Math.max(0.2, Math.min(w / designW, h / designH, 1.15));
      setFit({ scale, dx: (w - designW * scale) / 2, dy: (h - designH * scale) / 2 });
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [designW, designH]);
  return { ref, ...fit };
}

/**
 * The block-field graph: an A-block hub with edges out to nodes. Lit edges
 * flow and carry packets; dashed ones are "available". Used both while setup
 * builds (left stage) and for the final connected view.
 */
export function GraphCanvas({
  width,
  height,
  hubX,
  hubY,
  hubLabel,
  nodes,
  labels = [],
  packets = true,
  ariaLabel,
  children,
}: {
  width: number;
  height: number;
  hubX: number;
  hubY: number;
  hubLabel: string;
  nodes: GraphNode[];
  labels?: Array<{ t: string; x: number; y: number }>;
  packets?: boolean;
  ariaLabel: string;
  children?: React.ReactNode;
}) {
  const { ref, scale, dx, dy } = useFit(width, height);
  const lit = nodes.filter((n) => n.lit);
  return (
    <div ref={ref} className="ob-gfit" role="group" aria-label={ariaLabel}>
      <div
        className="ob-sgraph"
        style={{ width, height, transform: `translate(${dx}px, ${dy}px) scale(${scale})`, transformOrigin: "0 0" }}
      >
        {nodes.map((n, i) => {
          const e = edgeFrom(hubX, hubY, n.x, n.y);
          return (
            <div
              key={`e-${n.id}`}
              className={`ob-edge ${n.lit ? "on" : ""}`}
              style={{
                left: e.x,
                top: e.y,
                width: e.len,
                transform: `rotate(${e.angle.toFixed(1)}deg)`,
                animationDelay: `${(0.05 + i * 0.03).toFixed(2)}s`,
              }}
            />
          );
        })}
        {packets &&
          lit.map((n, i) => (
            <span
              key={`p-${n.id}`}
              className="ob-pkt"
              aria-hidden="true"
              style={
                {
                  left: hubX,
                  top: hubY,
                  ["--tx" as string]: `${n.x - hubX}px`,
                  ["--ty" as string]: `${n.y - hubY}px`,
                  animationDelay: `${(i * 0.4).toFixed(2)}s`,
                } as React.CSSProperties
              }
            />
          ))}
        {labels.map((g) => (
          <div key={g.t} className="ob-glab" style={{ left: g.x, top: g.y }}>
            {g.t}
          </div>
        ))}
        <div className="ob-hub" style={{ left: hubX, top: hubY }}>
          <span className="ob-shock" />
          <GraphOrb />
          <span className="ob-hubcap">{hubLabel}</span>
        </div>
        {nodes.map((n, i) => {
          const cls = `ob-node ${n.lit ? "conn" : ""} ${n.focus ? "focus" : ""}`;
          const style = { left: n.x, top: n.y, animationDelay: `${(0.05 + i * 0.05).toFixed(2)}s` };
          return n.onPick ? (
            <button
              key={n.id}
              type="button"
              className={cls}
              style={style}
              aria-label={`${n.label}, ${n.status ?? (n.lit ? "set up" : "available")}`}
              aria-pressed={n.focus ? true : false}
              onClick={n.onPick}
            >
              <i />
              {n.label}
            </button>
          ) : (
            <div key={n.id} className={cls} style={style}>
              <i />
              {n.label}
            </div>
          );
        })}
      </div>
      {children}
    </div>
  );
}
