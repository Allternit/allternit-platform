// cmd/allternit-cloud-api/render/motion/src/runner.mjs
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createCanvas, GlobalFonts, loadImage } from "@napi-rs/canvas";

// ../allternit-ai-wt-av2-gap-motion/src/lib/artifacts/motion/schema.ts
var MOTION_VERSION = 1;
var SCENE_TYPES = ["title", "counter", "chart", "logo", "list"];
var TRANSITION_TYPES = ["cut", "fade", "wipe", "slide"];
var FONT_FAMILIES = ["sans", "serif", "mono"];
var LIMITS = {
  maxScenes: 40,
  minDuration: 0.5,
  maxDuration: 60,
  maxTotalDuration: 180,
  maxText: 400,
  maxItems: 12,
  maxPoints: 24,
  minSize: 320,
  maxSize: 3840,
  minFps: 12,
  maxFps: 60
};
var DEFAULT_THEME = {
  bg: "#0b0b0f",
  fg: "#f5f5f7",
  accent: "#6d7cff",
  muted: "#8e8ea0",
  font: "sans"
};
var HEX = /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i;
function isHexColor(v) {
  return typeof v === "string" && HEX.test(v.trim());
}
function normalizeHex(v) {
  const s = v.trim().toLowerCase();
  return s.length === 4 ? `#${s[1]}${s[1]}${s[2]}${s[2]}${s[3]}${s[3]}` : s;
}
function clamp(n, lo, hi) {
  return Math.min(hi, Math.max(lo, n));
}
function str(v, fallback = "") {
  if (typeof v === "string") return v.slice(0, LIMITS.maxText);
  if (typeof v === "number" && Number.isFinite(v)) return String(v);
  return fallback;
}
function num(v, fallback) {
  const n = typeof v === "string" && v.trim() !== "" ? Number(v) : v;
  return typeof n === "number" && Number.isFinite(n) ? n : fallback;
}
function isSafeImageSrc(src) {
  return /^data:image\/(png|jpe?g|webp|gif|svg\+xml)[;,]/i.test(src) || /^https:\/\/[^\s]+$/i.test(src);
}
var rec = (v) => v && typeof v === "object" && !Array.isArray(v) ? v : {};
function strList(v, max) {
  if (!Array.isArray(v)) return [];
  return v.slice(0, max).map((x) => str(x));
}
function numList(v, max) {
  if (!Array.isArray(v)) return [];
  return v.slice(0, max).map((x) => num(x, 0));
}
function normalizeTheme(raw, warnings) {
  const o = rec(raw);
  const t = { ...DEFAULT_THEME };
  for (const k of ["bg", "fg", "accent", "muted"]) {
    if (o[k] === void 0) continue;
    if (isHexColor(o[k])) t[k] = normalizeHex(o[k]);
    else warnings.push(`Theme ${k} isn\u2019t a hex colour (#rrggbb); used the default.`);
  }
  if (o.font !== void 0) {
    if (FONT_FAMILIES.includes(o.font)) t.font = o.font;
    else warnings.push("Theme font must be sans, serif or mono; used sans.");
  }
  return t;
}
function normalizeTransition(raw, index, warnings) {
  if (index === 0) return { type: "cut", duration: 0 };
  const o = typeof raw === "string" ? { type: raw } : rec(raw);
  let type = "fade";
  if (o.type !== void 0) {
    if (TRANSITION_TYPES.includes(o.type)) type = o.type;
    else warnings.push(`Scene ${index + 1}: unknown transition \u201C${str(o.type)}\u201D; used fade.`);
  }
  const duration = type === "cut" ? 0 : clamp(num(o.duration, 0.5), 0.1, 2);
  return { type, duration };
}
function normalizeProps(type, raw) {
  const o = rec(raw);
  switch (type) {
    case "title":
      return {
        text: str(o.text ?? o.title ?? o.headline),
        subtitle: str(o.subtitle ?? o.sub),
        align: o.align === "center" ? "center" : "left"
      };
    case "counter":
      return {
        label: str(o.label),
        from: num(o.from, 0),
        to: num(o.to ?? o.value, 0),
        prefix: str(o.prefix),
        suffix: str(o.suffix),
        decimals: clamp(Math.round(num(o.decimals, 0)), 0, 4),
        caption: str(o.caption)
      };
    case "chart": {
      const labels = strList(o.labels, LIMITS.maxPoints);
      const values = numList(o.values, LIMITS.maxPoints);
      const n = Math.min(labels.length || values.length, values.length || labels.length);
      return {
        chart: o.chart === "line" || o.kind === "line" ? "line" : "bar",
        title: str(o.title),
        labels: Array.from({ length: n }, (_, i) => labels[i] ?? ""),
        values: values.slice(0, n),
        unit: str(o.unit)
      };
    }
    case "logo": {
      const src = str(o.src ?? o.image ?? o.url).trim();
      return { src: src && isSafeImageSrc(src) ? src : "", text: str(o.text ?? o.name), caption: str(o.caption) };
    }
    case "list":
      return { title: str(o.title), items: strList(o.items ?? o.bullets, LIMITS.maxItems) };
  }
}
function normalizeMotion(raw) {
  const root = rec(raw);
  const warnings = [];
  const rawScenes = Array.isArray(root.scenes) ? root.scenes : null;
  if (!rawScenes) return { ok: false, error: "This motion has no scenes list." };
  if (rawScenes.length === 0) return { ok: false, error: "This motion has no scenes yet." };
  if (root.version !== void 0 && root.version !== MOTION_VERSION) {
    if (typeof root.version === "number" && root.version > MOTION_VERSION) {
      return { ok: false, error: `This motion was made with a newer format (version ${root.version}). Update the app to play it.` };
    }
    warnings.push("Unknown version; read it as version 1.");
  }
  if (rawScenes.length > LIMITS.maxScenes) warnings.push(`Only the first ${LIMITS.maxScenes} scenes are used.`);
  const usedIds = /* @__PURE__ */ new Set();
  const scenes = rawScenes.slice(0, LIMITS.maxScenes).map((s, i) => {
    const o = rec(s);
    let type;
    if (SCENE_TYPES.includes(o.type)) type = o.type;
    else {
      type = inferType(o);
      if (o.type !== void 0) warnings.push(`Scene ${i + 1}: unknown type \u201C${str(o.type)}\u201D; read it as ${type}.`);
    }
    const props = normalizeProps(type, o.props ?? o);
    let title = str(o.title ?? o.name);
    if (!title) title = sceneLabel({ type, props }, i);
    let id = str(o.id).replace(/[^a-z0-9_-]/gi, "").slice(0, 40) || `s${i + 1}`;
    while (usedIds.has(id)) id = `${id}_${i + 1}`;
    usedIds.add(id);
    const duration = clamp(num(o.duration, 3), LIMITS.minDuration, LIMITS.maxDuration);
    if (o.duration !== void 0 && duration !== num(o.duration, duration)) warnings.push(`Scene ${i + 1}: duration limited to ${duration}s.`);
    const color = o.color !== void 0 && isHexColor(o.color) ? normalizeHex(o.color) : "";
    const bg = o.bg !== void 0 && isHexColor(o.bg) ? normalizeHex(o.bg) : "";
    return { id, type, title, duration, transition: normalizeTransition(o.transition, i, warnings), bg, color, props };
  });
  const kept = [];
  let total = 0;
  for (const s of scenes) {
    if (total + s.duration > LIMITS.maxTotalDuration && kept.length) {
      warnings.push(`The motion is limited to ${LIMITS.maxTotalDuration}s; later scenes were cut.`);
      break;
    }
    total += s.duration;
    kept.push(s);
  }
  const comp = {
    version: MOTION_VERSION,
    width: Math.round(clamp(num(root.width, 1920), LIMITS.minSize, LIMITS.maxSize)),
    height: Math.round(clamp(num(root.height, 1080), LIMITS.minSize, LIMITS.maxSize)),
    fps: Math.round(clamp(num(root.fps, 30), LIMITS.minFps, LIMITS.maxFps)),
    theme: normalizeTheme(root.theme, warnings),
    scenes: kept
  };
  return { ok: true, comp, warnings };
}
function inferType(o) {
  const p = rec(o.props ?? o);
  if (Array.isArray(p.items) || Array.isArray(p.bullets)) return "list";
  if (Array.isArray(p.values)) return "chart";
  if (p.to !== void 0 || p.value !== void 0) return "counter";
  if (p.src !== void 0 || p.image !== void 0) return "logo";
  return "title";
}
function parseMotion(body) {
  let raw;
  try {
    raw = JSON.parse(body);
  } catch {
    return { ok: false, error: "This motion isn\u2019t valid JSON." };
  }
  return normalizeMotion(raw);
}
function sceneLabel(scene, index) {
  const p = scene.props;
  const first = [p.text, p.title, p.label].find((v) => typeof v === "string" && v.trim());
  return first?.trim().slice(0, 40) || `Scene ${index + 1}`;
}
function totalDuration(comp) {
  return comp.scenes.reduce((a, s) => a + s.duration, 0);
}
function sceneAt(comp, t) {
  const total = totalDuration(comp);
  const tt = clamp(t, 0, Math.max(0, total - 1e-6));
  let start = 0;
  for (let i = 0; i < comp.scenes.length; i++) {
    const s = comp.scenes[i];
    if (tt < start + s.duration || i === comp.scenes.length - 1) return { index: i, scene: s, local: tt - start, start };
    start += s.duration;
  }
  const last = comp.scenes.length - 1;
  return { index: last, scene: comp.scenes[last], local: 0, start };
}

// ../allternit-ai-wt-av2-gap-motion/src/lib/artifacts/motion/render.ts
var cl = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
var P = (t, a, b) => b <= a ? t >= b ? 1 : 0 : cl((t - a) / (b - a));
var ease = {
  linear: (t) => t,
  outCubic: (t) => 1 - Math.pow(1 - t, 3),
  outExpo: (t) => t >= 1 ? 1 : 1 - Math.pow(2, -10 * t),
  ioCubic: (t) => t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2,
  outBack: (t) => {
    const c1 = 1.4;
    const c3 = c1 + 1;
    return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
  }
};
var FONT_STACKS = {
  sans: 'Inter, "SF Pro Display", system-ui, -apple-system, "Segoe UI", Roboto, sans-serif',
  serif: 'Georgia, "Iowa Old Style", "Times New Roman", serif',
  mono: 'ui-monospace, "SF Mono", Menlo, Consolas, monospace'
};
function inkFor(comp, scene) {
  const th = comp.theme;
  return {
    bg: scene.bg || th.bg,
    fg: scene.color || th.fg,
    accent: th.accent,
    muted: th.muted,
    font: FONT_STACKS[th.font],
    u: comp.height / 1080,
    w: comp.width,
    h: comp.height
  };
}
function setFont(ctx, ink, size, weight) {
  ctx.font = `${weight} ${size}px ${ink.font}`;
}
function wrap(ctx, text, maxW) {
  const lines = [];
  for (const para of text.split("\n")) {
    let line = "";
    for (const word of para.split(/\s+/).filter(Boolean)) {
      const next = line ? `${line} ${word}` : word;
      if (line && ctx.measureText(next).width > maxW) {
        lines.push(line);
        line = word;
      } else line = next;
    }
    lines.push(line);
  }
  return lines;
}
function fitSize(ctx, ink, text, weight, start, maxW, maxLines) {
  let size = start;
  for (; size > 28 * ink.u; size -= 4 * ink.u) {
    setFont(ctx, ink, size, weight);
    if (wrap(ctx, text, maxW).length <= maxLines) break;
  }
  return size;
}
function rise(ctx, text, x, baseline, size, p) {
  if (p <= 0 || !text) return;
  ctx.save();
  ctx.beginPath();
  ctx.rect(x - size, baseline - size * 1.05, ctx.measureText(text).width + size * 2, size * 1.35);
  ctx.clip();
  ctx.globalAlpha *= cl(p * 1.6);
  ctx.fillText(text, x, baseline + (1 - ease.outExpo(p)) * size * 0.95);
  ctx.restore();
}
function fill(ctx, ink) {
  ctx.fillStyle = ink.bg;
  ctx.fillRect(0, 0, ink.w, ink.h);
}
function drawTitle(ctx, ink, s, lt) {
  const { u, w, h } = ink;
  const center = s.props.align === "center";
  const padX = 150 * u;
  const maxW = w - padX * 2;
  const size = fitSize(ctx, ink, s.props.text, 800, 132 * u, maxW, 4);
  setFont(ctx, ink, size, 800);
  const lines = wrap(ctx, s.props.text, maxW);
  const lineH = size * 1.12;
  const blockH = lines.length * lineH;
  const top = h / 2 - blockH / 2 - (s.props.subtitle ? 36 * u : 0);
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = center ? "center" : "left";
  ctx.fillStyle = ink.fg;
  let wordIndex = 0;
  lines.forEach((line, li) => {
    const baseline = top + li * lineH + size * 0.85;
    const words = line.split(" ");
    const lineW = ctx.measureText(line).width;
    let x = center ? w / 2 - lineW / 2 : padX;
    ctx.textAlign = "left";
    for (const word of words) {
      const p = P(lt, 0.15 + wordIndex * 0.09, 0.15 + wordIndex * 0.09 + 0.7);
      rise(ctx, word, x, baseline, size, p);
      x += ctx.measureText(`${word} `).width;
      wordIndex += 1;
    }
  });
  const after = 0.15 + wordIndex * 0.09 + 0.45;
  const barP = ease.outExpo(P(lt, after - 0.2, after + 0.6));
  const barY = top + blockH + 18 * u;
  const barW = 140 * u * barP;
  ctx.fillStyle = ink.accent;
  ctx.fillRect(center ? w / 2 - barW / 2 : padX, barY, barW, 8 * u);
  if (s.props.subtitle) {
    const sub = fitSize(ctx, ink, s.props.subtitle, 500, 44 * u, maxW, 2);
    setFont(ctx, ink, sub, 500);
    ctx.fillStyle = ink.muted;
    const subLines = wrap(ctx, s.props.subtitle, maxW);
    subLines.forEach((line, i) => {
      const p = ease.outCubic(P(lt, after + 0.1 + i * 0.1, after + 0.7 + i * 0.1));
      ctx.save();
      ctx.globalAlpha *= p;
      ctx.textAlign = center ? "center" : "left";
      ctx.fillText(line, center ? w / 2 : padX, barY + 24 * u + sub * (i + 1) * 1.15 + (1 - p) * 14 * u);
      ctx.restore();
    });
  }
}
function formatNumber(n, decimals) {
  const fixed = Math.abs(n).toFixed(decimals);
  const [int, frac] = fixed.split(".");
  const grouped = int.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${n < 0 && Number(fixed) !== 0 ? "-" : ""}${grouped}${frac ? `.${frac}` : ""}`;
}
function counterValue(from, to, lt, duration) {
  const p = ease.outCubic(P(lt, 0.35, Math.max(0.9, Math.min(duration - 0.5, 2.4))));
  return from + (to - from) * p;
}
function drawCounter(ctx, ink, s, lt, duration) {
  const { u, w, h } = ink;
  const p = s.props;
  const text = `${p.prefix}${formatNumber(counterValue(p.from, p.to, lt, duration), p.decimals)}${p.suffix}`;
  const size = fitSize(ctx, ink, `${p.prefix}${formatNumber(Math.max(Math.abs(p.from), Math.abs(p.to)), p.decimals)}${p.suffix}`, 800, 280 * u, w - 300 * u, 1);
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = "center";
  if (p.label) {
    setFont(ctx, ink, 40 * u, 600);
    ctx.fillStyle = ink.muted;
    ctx.save();
    ctx.globalAlpha *= ease.outCubic(P(lt, 0, 0.5));
    ctx.fillText(p.label.toUpperCase(), w / 2, h / 2 - size * 0.62 - 20 * u + (1 - ease.outCubic(P(lt, 0, 0.5))) * 12 * u);
    ctx.restore();
  }
  setFont(ctx, ink, size, 800);
  ctx.fillStyle = ink.fg;
  ctx.save();
  ctx.globalAlpha *= cl(lt / 0.25);
  ctx.fillText(text, w / 2, h / 2 + size * 0.32);
  ctx.restore();
  const lineW = ease.outExpo(P(lt, 0.3, 1.1)) * Math.min(520 * u, w - 300 * u);
  ctx.fillStyle = ink.accent;
  ctx.fillRect(w / 2 - lineW / 2, h / 2 + size * 0.32 + 36 * u, lineW, 8 * u);
  if (p.caption) {
    setFont(ctx, ink, 38 * u, 500);
    ctx.fillStyle = ink.muted;
    ctx.save();
    ctx.globalAlpha *= ease.outCubic(P(lt, 1, 1.6));
    ctx.fillText(p.caption, w / 2, h / 2 + size * 0.32 + 110 * u);
    ctx.restore();
  }
}
function drawChart(ctx, ink, s, lt) {
  const { u, w, h } = ink;
  const { labels, values, chart, unit, title } = s.props;
  const left = 170 * u;
  const right = w - 170 * u;
  const top = 250 * u;
  const bottom = h - 200 * u;
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = "left";
  if (title) {
    setFont(ctx, ink, 64 * u, 800);
    ctx.fillStyle = ink.fg;
    rise(ctx, title, left, 150 * u, 64 * u, P(lt, 0.05, 0.7));
  }
  const n = values.length;
  if (!n) return;
  const lo = Math.min(0, ...values);
  const hi = Math.max(...values, lo + 1e-9);
  const y = (v) => bottom - (v - lo) / (hi - lo) * (bottom - top);
  ctx.strokeStyle = ink.muted;
  ctx.globalAlpha = 0.35;
  ctx.lineWidth = 2 * u;
  ctx.beginPath();
  ctx.moveTo(left, y(Math.max(0, lo)));
  ctx.lineTo(right, y(Math.max(0, lo)));
  ctx.stroke();
  ctx.globalAlpha = 1;
  const slot = (right - left) / n;
  const labelSize = Math.min(34 * u, slot * 0.28);
  const valueText = (v) => `${formatNumber(v, Number.isInteger(v) ? 0 : 1)}${unit}`;
  if (chart === "bar") {
    const bw = Math.min(slot * 0.62, 180 * u);
    values.forEach((v, i) => {
      const p = ease.outExpo(P(lt, 0.3 + i * 0.1, 0.3 + i * 0.1 + 0.9));
      const x = left + slot * i + (slot - bw) / 2;
      const y0 = y(Math.max(0, lo));
      const y1 = y0 + (y(v) - y0) * p;
      ctx.fillStyle = i === n - 1 ? ink.accent : mixHex(ink.accent, ink.bg, 0.55);
      ctx.beginPath();
      ctx.roundRect(x, Math.min(y0, y1), bw, Math.abs(y1 - y0), 10 * u);
      ctx.fill();
      setFont(ctx, ink, labelSize * 1.15, 700);
      ctx.fillStyle = ink.fg;
      ctx.textAlign = "center";
      ctx.save();
      ctx.globalAlpha *= ease.outCubic(P(lt, 0.9 + i * 0.1, 1.4 + i * 0.1));
      ctx.fillText(valueText(v), x + bw / 2, Math.min(y0, y1) - 16 * u);
      ctx.restore();
    });
  } else {
    const pts = values.map((v, i) => ({ x: left + slot * (i + 0.5), y: y(v) }));
    const draw = ease.ioCubic(P(lt, 0.3, 0.3 + 1.6));
    const segs = pts.length - 1;
    const reach = draw * segs;
    ctx.strokeStyle = ink.accent;
    ctx.lineWidth = 8 * u;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(pts[0].x, pts[0].y);
    for (let i = 1; i <= segs; i++) {
      const f = cl(reach - (i - 1));
      if (f <= 0) break;
      const a = pts[i - 1];
      const b = pts[i];
      ctx.lineTo(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f);
    }
    ctx.stroke();
    pts.forEach((pt, i) => {
      const p = ease.outBack(P(reach, i - 0.15, i + 0.25));
      if (p <= 0) return;
      ctx.fillStyle = ink.bg;
      ctx.strokeStyle = ink.accent;
      ctx.lineWidth = 6 * u;
      ctx.beginPath();
      ctx.arc(pt.x, pt.y, 13 * u * p, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      setFont(ctx, ink, labelSize * 1.15, 700);
      ctx.fillStyle = ink.fg;
      ctx.textAlign = "center";
      ctx.save();
      ctx.globalAlpha *= cl(p);
      ctx.fillText(valueText(values[i]), pt.x, pt.y - 28 * u);
      ctx.restore();
    });
  }
  setFont(ctx, ink, labelSize, 500);
  ctx.fillStyle = ink.muted;
  ctx.textAlign = "center";
  ctx.save();
  ctx.globalAlpha *= ease.outCubic(P(lt, 0.4, 1));
  labels.forEach((label, i) => ctx.fillText(clip(ctx, label, slot * 0.92), left + slot * (i + 0.5), bottom + 64 * u));
  ctx.restore();
}
function clip(ctx, text, maxW) {
  if (ctx.measureText(text).width <= maxW) return text;
  let t = text;
  while (t.length > 1 && ctx.measureText(`${t}\u2026`).width > maxW) t = t.slice(0, -1);
  return `${t}\u2026`;
}
function drawLogo(ctx, ink, s, lt, assets) {
  const { u, w, h } = ink;
  const img = s.props.src ? assets?.get(s.props.src) : void 0;
  const p = ease.outBack(P(lt, 0.1, 0.9));
  const a = cl(lt / 0.4);
  const cy = h / 2 - (s.props.caption ? 40 * u : 0);
  ctx.save();
  ctx.translate(w / 2, cy);
  ctx.scale(0.7 + 0.3 * p, 0.7 + 0.3 * p);
  ctx.globalAlpha *= a;
  if (img) {
    const maxW = Math.min(720 * u, w * 0.6);
    const maxH = 360 * u;
    const k = Math.min(maxW / img.width, maxH / img.height);
    ctx.drawImage(img.source, -img.width * k / 2, -img.height * k / 2, img.width * k, img.height * k);
  } else {
    const mark = 150 * u;
    setFont(ctx, ink, 120 * u, 800);
    ctx.textBaseline = "alphabetic";
    const tw = s.props.text ? ctx.measureText(s.props.text).width : 0;
    const gap = s.props.text ? 36 * u : 0;
    const total = mark + gap + tw;
    ctx.fillStyle = ink.accent;
    ctx.beginPath();
    ctx.roundRect(-total / 2, -mark / 2, mark, mark, mark * 0.24);
    ctx.fill();
    if (s.props.text) {
      ctx.fillStyle = ink.fg;
      ctx.textAlign = "left";
      ctx.fillText(s.props.text, -total / 2 + mark + gap, 120 * u * 0.33);
    }
  }
  ctx.restore();
  if (s.props.caption) {
    setFont(ctx, ink, 40 * u, 500);
    ctx.fillStyle = ink.muted;
    ctx.textAlign = "center";
    ctx.save();
    ctx.globalAlpha *= ease.outCubic(P(lt, 0.8, 1.4));
    ctx.fillText(s.props.caption, w / 2, cy + 280 * u + (1 - ease.outCubic(P(lt, 0.8, 1.4))) * 14 * u);
    ctx.restore();
  }
}
function drawList(ctx, ink, s, lt) {
  const { u, w, h } = ink;
  const left = 170 * u;
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = "left";
  const items = s.props.items;
  const head = s.props.title ? 160 * u : 0;
  const rowH = Math.min(120 * u, (h - 360 * u - head) / Math.max(1, items.length));
  const total = head + rowH * items.length;
  const top = h / 2 - total / 2;
  if (s.props.title) {
    setFont(ctx, ink, 72 * u, 800);
    ctx.fillStyle = ink.fg;
    rise(ctx, s.props.title, left, top + 70 * u, 72 * u, P(lt, 0.05, 0.75));
  }
  const size = Math.min(54 * u, rowH * 0.56);
  items.forEach((item, i) => {
    const p = ease.outExpo(P(lt, 0.4 + i * 0.25, 0.4 + i * 0.25 + 0.7));
    if (p <= 0) return;
    const baseline = top + head + rowH * i + rowH * 0.62;
    ctx.save();
    ctx.globalAlpha *= cl(p * 1.5);
    ctx.translate((1 - p) * -60 * u, 0);
    ctx.fillStyle = ink.accent;
    ctx.beginPath();
    ctx.arc(left + 14 * u, baseline - size * 0.32, 11 * u, 0, Math.PI * 2);
    ctx.fill();
    setFont(ctx, ink, size, 600);
    ctx.fillStyle = ink.fg;
    ctx.fillText(clip(ctx, item, w - left * 2 - 60 * u), left + 56 * u, baseline);
    ctx.restore();
  });
}
function drawScene(ctx, comp, scene, lt, assets) {
  const ink = inkFor(comp, scene);
  ctx.save();
  fill(ctx, ink);
  switch (scene.type) {
    case "title":
      drawTitle(ctx, ink, scene, lt);
      break;
    case "counter":
      drawCounter(ctx, ink, scene, lt, scene.duration);
      break;
    case "chart":
      drawChart(ctx, ink, scene, lt);
      break;
    case "logo":
      drawLogo(ctx, ink, scene, lt, assets);
      break;
    case "list":
      drawList(ctx, ink, scene, lt);
      break;
  }
  ctx.restore();
}
function renderFrame(ctx, comp, t, assets) {
  const { index, scene, local } = sceneAt(comp, t);
  const tr = scene.transition;
  const trDur = Math.min(tr.duration, scene.duration * 0.6);
  if (index === 0 || tr.type === "cut" || trDur <= 0 || local >= trDur) {
    drawScene(ctx, comp, scene, local, assets);
    return;
  }
  const prev = comp.scenes[index - 1];
  drawScene(ctx, comp, prev, prev.duration, assets);
  const p = ease.ioCubic(local / trDur);
  const { width: w, height: h } = comp;
  ctx.save();
  if (tr.type === "fade") {
    ctx.globalAlpha = p;
  } else if (tr.type === "slide") {
    ctx.translate(w * (1 - p), 0);
  } else {
    ctx.beginPath();
    ctx.rect(0, 0, w * p, h);
    ctx.clip();
  }
  drawScene(ctx, comp, scene, local, assets);
  ctx.restore();
  if (tr.type === "wipe" && p < 1) {
    const u = h / 1080;
    ctx.fillStyle = comp.theme.accent;
    ctx.fillRect(w * p - 6 * u, 0, 12 * u, h);
  }
}
function mixHex(a, b, t) {
  const pa = [1, 3, 5].map((i) => parseInt(a.slice(i, i + 2), 16));
  const pb = [1, 3, 5].map((i) => parseInt(b.slice(i, i + 2), 16));
  return `rgb(${pa.map((v, i) => Math.round(v + (pb[i] - v) * t)).join(",")})`;
}
function compositionImages(comp) {
  const out = /* @__PURE__ */ new Set();
  for (const s of comp.scenes) if (s.type === "logo" && s.props.src) out.add(s.props.src);
  return [...out];
}

// cmd/allternit-cloud-api/render/motion/src/runner.mjs
var FONTS = [
  ["Inter", "Inter_500Medium.ttf"],
  ["Inter", "Inter_600SemiBold.ttf"],
  ["Inter", "Inter_700Bold.ttf"],
  ["Inter", "Inter_800ExtraBold.ttf"],
  // The serif and mono stacks start with Georgia and ui-monospace/Menlo,
  // which a Linux host doesn't have; these stand in.
  ["Georgia", "SourceSerif4_500Medium.ttf"],
  ["Georgia", "SourceSerif4_700Bold.ttf"],
  ["Menlo", "JetBrainsMono_500Medium.ttf"],
  ["Menlo", "JetBrainsMono_700Bold.ttf"],
  ["ui-monospace", "JetBrainsMono_500Medium.ttf"],
  ["ui-monospace", "JetBrainsMono_700Bold.ttf"]
];
function registerFonts() {
  const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "fonts");
  for (const [family, file] of FONTS) {
    const p = path.join(dir, file);
    if (existsSync(p)) GlobalFonts.registerFromPath(p, family);
  }
}
function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    const k = argv[i];
    if (!k?.startsWith("--") || argv[i + 1] === void 0) throw new Error(`Bad arguments near ${k ?? "(end)"}`);
    out[k.slice(2)] = argv[i + 1];
  }
  for (const k of ["in", "out"]) if (!out[k]) throw new Error(`Missing --${k}`);
  return { input: out.in, output: out.out, ffmpeg: out.ffmpeg || "ffmpeg" };
}
async function loadAssets(comp) {
  const map = /* @__PURE__ */ new Map();
  await Promise.all(
    compositionImages(comp).map(async (src) => {
      if (!/^data:image\//i.test(src)) return;
      try {
        const img = await loadImage(src);
        map.set(src, { source: img, width: img.width || 512, height: img.height || 512 });
      } catch {
      }
    })
  );
  return map;
}
function startFfmpeg(bin, { width, height, fps }, output) {
  const proc = spawn(
    bin,
    [
      "-hide_banner",
      "-loglevel",
      "error",
      "-y",
      "-f",
      "rawvideo",
      "-pix_fmt",
      "rgba",
      "-s",
      `${width}x${height}`,
      "-framerate",
      String(fps),
      "-i",
      "pipe:0",
      "-an",
      "-c:v",
      "libx264",
      "-preset",
      "veryfast",
      "-crf",
      "18",
      "-pix_fmt",
      "yuv420p",
      "-r",
      String(fps),
      "-movflags",
      "+faststart",
      output
    ],
    { stdio: ["pipe", "ignore", "pipe"] }
  );
  let stderr = "";
  proc.stderr.on("data", (d) => {
    stderr = (stderr + d).slice(-2e3);
  });
  let failed = null;
  proc.stdin.on("error", (e) => {
    failed = e;
  });
  const exited = new Promise((resolve) => proc.on("close", (code) => resolve(code)));
  return { proc, exited, error: () => failed, stderr: () => stderr };
}
async function write(stream, buf, ff) {
  if (ff.error()) throw ff.error();
  if (stream.write(buf)) return;
  await new Promise((resolve, reject) => {
    const onDrain = () => {
      stream.off("error", onError);
      resolve();
    };
    const onError = (e) => {
      stream.off("drain", onDrain);
      reject(e);
    };
    stream.once("drain", onDrain);
    stream.once("error", onError);
  });
}
async function main() {
  const { input, output, ffmpeg } = args(process.argv.slice(2));
  const parsed = parseMotion(await readFile(input, "utf8"));
  if (!parsed.ok) throw new Error(`Invalid composition: ${parsed.error}`);
  const comp = parsed.comp;
  registerFonts();
  const assets = await loadAssets(comp);
  const size = { width: comp.width + comp.width % 2, height: comp.height + comp.height % 2, fps: comp.fps };
  const frames = Math.max(1, Math.round(totalDuration(comp) * comp.fps));
  const canvas = createCanvas(size.width, size.height);
  const ctx = canvas.getContext("2d");
  const ff = startFfmpeg(ffmpeg, size, output);
  try {
    for (let i = 0; i < frames; i += 1) {
      ctx.setTransform(size.width / comp.width, 0, 0, size.height / comp.height, 0, 0);
      renderFrame(ctx, comp, i / comp.fps, assets);
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      await write(ff.proc.stdin, canvas.data(), ff);
      if (i % 10 === 9 || i === frames - 1) console.log(`progress ${((i + 1) / frames).toFixed(4)}`);
    }
    ff.proc.stdin.end();
  } catch (e) {
    ff.proc.kill("SIGKILL");
    await ff.exited;
    throw new Error(`${e instanceof Error ? e.message : e}
${ff.stderr()}`);
  }
  const code = await ff.exited;
  if (code !== 0) throw new Error(`ffmpeg exited with ${code}
${ff.stderr()}`);
}
main().catch((e) => {
  console.error(e instanceof Error ? e.stack || e.message : String(e));
  process.exit(1);
});
