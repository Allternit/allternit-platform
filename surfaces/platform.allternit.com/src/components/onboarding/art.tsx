import React from "react";
import { BLOCK_WORDS } from "@/components/onboarding/blockWords";

/** A://TERNIT block-type word (console wordmark or a scene lock-up). */
export function BlockWord({
  word,
  height = 13,
  className,
}: {
  word: keyof typeof BLOCK_WORDS;
  height?: number;
  className?: string;
}) {
  const art = BLOCK_WORDS[word];
  const cells = art.cells.split(" ");
  const core = art.core ? art.core.split(" ") : [];
  const rect = (c: string, fill: string) => {
    const [x, y] = c.split(".").map(Number);
    return (
      <rect key={`${c}-${fill}`} x={x * 10 + 0.75} y={y * 10 + 0.75} width={8.5} height={8.5} rx={1.5} fill={fill} />
    );
  };
  return (
    <svg
      viewBox={`0 0 ${art.width} 50`}
      aria-hidden="true"
      shapeRendering="geometricPrecision"
      className={className}
      style={{ height, width: "auto", display: "block" }}
    >
      {core.map((c) => rect(c, "#D97757"))}
      {cells.map((c) => rect(c, "currentColor"))}
    </svg>
  );
}

const ORB_CELLS: Array<[number, number, "ra" | "rb"]> = [
  [0, 4, "rb"], [0, 3, "rb"], [0, 2, "rb"], [1, 1, "ra"], [2, 0, "rb"], [3, 1, "ra"],
  [4, 2, "rb"], [4, 3, "rb"], [4, 4, "rb"], [3, 2, "ra"], [1, 2, "ra"],
];

export type OrbState = "idle" | "thinking" | "searching" | "working" | "writing";

/** The A:// A-block orb; its state animates the blocks (thinking, searching, working, writing). */
export function AOrb({ state = "idle", className }: { state?: OrbState; className?: string }) {
  return (
    <svg viewBox="0 0 50 50" className={`ob-o ob-o-${state} ${className ?? ""}`} aria-hidden="true">
      <g fill="currentColor">
        {ORB_CELLS.map(([x, y, cls]) => (
          <rect key={`${x}-${y}`} x={x * 10 + 0.75} y={y * 10 + 0.75} width={8.5} height={8.5} rx={1.5} className={cls} />
        ))}
      </g>
      <rect x={20.75} y={20.75} width={8.5} height={8.5} rx={1.5} className="rc" fill="#D97757" />
    </svg>
  );
}

const GORB_CELLS: Array<[number, number, number]> = [
  [2, 0, 4], [1, 1, 3], [3, 1, 5], [0, 2, 2], [1, 2, 10], [3, 2, 9], [4, 2, 6], [0, 3, 1], [4, 3, 7], [0, 4, 0], [4, 4, 8],
];

/** The graph hub: the A-block core, working. */
export function GraphOrb({ label = "Allternit core" }: { label?: string }) {
  return (
    <svg className="ob-gorb" data-s="working" viewBox="0 0 50 50" role="img" aria-label={label}>
      <g fill="currentColor">
        {GORB_CELLS.map(([x, y, o]) => (
          <rect
            key={`${x}-${y}`}
            className="ob-gb"
            x={x * 10 + 0.75}
            y={y * 10 + 0.75}
            width={8.5}
            height={8.5}
            rx={1.5}
            style={{ ["--o" as string]: o } as React.CSSProperties}
          />
        ))}
      </g>
      <rect className="ob-gcore" x={20.75} y={20.75} width={8.5} height={8.5} rx={1.5} fill="#D97757" />
    </svg>
  );
}

/** Gizzi, the official mascot (same drawing as the approved prototype). */
export function Gizzi({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 96 96" aria-hidden="true" shapeRendering="geometricPrecision" className={className}>
      <ellipse cx="48" cy="86" rx="21" ry="4" fill="rgba(9, 11, 14, 0.14)" />
      <rect className="bc" x="44" y="8" width="8" height="5" rx="2" fill="#D97757" />
      <rect x="24" y="15" width="17" height="7" rx="3" fill="#D4B08C" />
      <rect x="55" y="15" width="17" height="7" rx="3" fill="#D4B08C" />
      <path d="M25 23H71L78 30V56L72 63V69L64 76H32L24 69V63L18 56V30L25 23Z" fill="#D4B08C" />
      <path d="M18 40H14V44H10V48H14V52H18V56H22V40H18Z" fill="#D4B08C" />
      <path d="M78 40H82V44H86V48H82V52H78V56H74V40H78Z" fill="#D4B08C" />
      <rect x="24" y="29" width="48" height="31" rx="9" fill="rgba(17, 19, 24, 0.16)" />
      <rect className="eye" x="33" y="36" width="8" height="10" rx="2" fill="#111318" />
      <rect className="eye" x="59" y="36" width="8" height="10" rx="2" fill="#111318" />
      <path
        d="M44.5 52L48 42L51.5 52H49.6L48.8 49.5H47.2L46.4 52H44.5ZM47.75 47.7H48.25L48 46.65L47.75 47.7Z"
        fill="#D97757"
      />
      <rect x="46" y="47.2" width="4" height="1.4" rx="0.7" fill="#D97757" />
      <text
        x="48"
        y="57.5"
        textAnchor="middle"
        fill="#D97757"
        fontSize="8.2"
        fontWeight="700"
        fontFamily="SF Mono, Menlo, monospace"
        letterSpacing="-0.4"
      >
        ://
      </text>
      <rect x="24" y="74" width="8" height="12" fill="#D4B08C" />
      <rect x="36" y="74" width="8" height="12" fill="#D4B08C" />
      <rect x="52" y="74" width="8" height="12" fill="#D4B08C" />
      <rect x="64" y="74" width="8" height="12" fill="#D4B08C" />
    </svg>
  );
}

export function CheckIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5 12l5 5 9-10" />
    </svg>
  );
}

export function ChevronIcon({ dir }: { dir: "left" | "right" }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d={dir === "left" ? "M15 6l-6 6 6 6" : "M9 6l6 6-6 6"} />
    </svg>
  );
}

/** The dotted block field behind stages and graphs. */
export function BlockField({ id }: { id: string }) {
  return (
    <svg aria-hidden="true">
      <defs>
        <pattern id={id} width="22" height="22" patternUnits="userSpaceOnUse">
          <rect x="7" y="7" width="8" height="8" rx="1.5" fill="currentColor" opacity=".06" />
        </pattern>
      </defs>
      <rect width="100%" height="100%" fill={`url(#${id})`} />
    </svg>
  );
}

const BURST: Array<[number, number, string, number]> = [
  [45, 9, "#D97757", 90], [50, 38, "var(--ink)", 127], [21, 41, "#22A06B", 164], [2, 63, "#2F6FE0", 201],
  [-19, 42, "#F2C14E", 238], [-48, 40, "#D97757", 275], [-45, 11, "var(--ink)", 112], [-62, -13, "#22A06B", 149],
  [-37, -28, "#2F6FE0", 186], [-29, -56, "#F2C14E", 223], [-1, -46, "#D97757", 260], [26, -57, "var(--ink)", 97],
  [35, -30, "#22A06B", 134], [61, -16, "#2F6FE0", 171],
];

/** A ring + block burst that fires once when its scene is current (decorative). */
export function Burst({ left, top, delay, ring = true }: { left: number; top: number; delay: number; ring?: boolean }) {
  const d = `${delay}s`;
  return (
    <>
      {ring && <div className="ob-ring" style={{ left, top, ["--d" as string]: d } as React.CSSProperties} />}
      <div className="ob-bst" style={{ left, top, ["--d" as string]: d } as React.CSSProperties} aria-hidden="true">
        {BURST.map(([dx, dy, c, rt], i) => (
          <i
            key={i}
            style={
              {
                ["--dx" as string]: `${dx}px`,
                ["--dy" as string]: `${dy}px`,
                ["--c" as string]: c,
                ["--rt" as string]: `${rt}deg`,
              } as React.CSSProperties
            }
          />
        ))}
      </div>
    </>
  );
}

export function GitHubMark() {
  return (
    <svg width="17" height="17" viewBox="0 0 16 16" aria-hidden="true">
      <path
        fill="currentColor"
        d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"
      />
    </svg>
  );
}
