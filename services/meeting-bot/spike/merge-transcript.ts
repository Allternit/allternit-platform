// Merge STT segments with the speaker timeline.
//
//   node dist/merge-transcript.js <out-dir>
//
// Inputs (written by capture.sh, transcribe.sh and join.ts):
//   audio/capture-start-ms   epoch ms when capture began
//   audio/segments.csv       ffmpeg segment list: file,start_s,end_s (relative to capture start)
//   audio/stt/seg-*.json     voice-service /v1/stt responses ({segments:[{start_time,end_time,text,confidence}]})
//   speakers.jsonl           {t, name} every 250 ms for each speaking participant
//
// Output: transcript.jsonl rows
//   {t_start, t_end, text, speaker, speakers_seen, confidence, file}
// speaker = the active speaker nearest the segment midpoint (within 1 s), the
// simple rule the spike calls for; speakers_seen = everyone marked speaking
// during the segment, which shows how often the midpoint rule is ambiguous.
// Also writes transcript.txt for reading.

import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

interface SttSegment {
  start_time: number;
  end_time: number;
  text: string;
  confidence?: number;
}
interface SpeakerSample {
  t: number;
  name: string;
}

const MIDPOINT_WINDOW_MS = 1000;

const out = process.argv[2];
if (!out) {
  console.error("usage: merge-transcript <out-dir>");
  process.exit(2);
}
const aud = join(out, "audio");

const startFile = join(aud, "capture-start-ms");
const captureStart = existsSync(startFile) ? Number(readFileSync(startFile, "utf8").trim()) : 0;
if (!captureStart) console.warn("[merge] no capture-start-ms: times are relative and speakers cannot be matched");

// file -> offset seconds from capture start
const offsets = new Map<string, number>();
const csv = join(aud, "segments.csv");
if (existsSync(csv)) {
  for (const line of readFileSync(csv, "utf8").split("\n")) {
    const [file, start] = line.split(",");
    if (file && start !== undefined && !Number.isNaN(Number(start))) offsets.set(basename(file, ".wav"), Number(start));
  }
}

const speakers: SpeakerSample[] = [];
const spk = join(out, "speakers.jsonl");
if (existsSync(spk)) {
  for (const line of readFileSync(spk, "utf8").split("\n")) {
    if (!line.trim()) continue;
    try {
      const r = JSON.parse(line) as SpeakerSample;
      if (typeof r.t === "number" && r.name) speakers.push(r);
    } catch {
      /* skip a torn last line */
    }
  }
  speakers.sort((a, b) => a.t - b.t);
}

function speakerAt(mid: number): string | null {
  let best: SpeakerSample | null = null;
  let bestD = Infinity;
  // Linear scan is fine for a 30-minute spike (~7k samples per speaker).
  for (const s of speakers) {
    const d = Math.abs(s.t - mid);
    if (d < bestD) {
      bestD = d;
      best = s;
    }
    if (s.t > mid + MIDPOINT_WINDOW_MS) break;
  }
  return best && bestD <= MIDPOINT_WINDOW_MS ? best.name : null;
}

function speakersBetween(a: number, b: number): string[] {
  return [...new Set(speakers.filter((s) => s.t >= a && s.t <= b).map((s) => s.name))];
}

const sttDir = join(aud, "stt");
const files = existsSync(sttDir) ? readdirSync(sttDir).filter((f) => f.endsWith(".json")).sort() : [];
const rows: Array<Record<string, unknown>> = [];
let fallbackOffset = 0;
for (const f of files) {
  const seg = basename(f, ".json");
  const res = JSON.parse(readFileSync(join(sttDir, f), "utf8")) as { segments?: SttSegment[] };
  const off = offsets.get(seg) ?? fallbackOffset;
  let lastEnd = 0;
  for (const s of res.segments ?? []) {
    if (!s.text?.trim()) continue;
    const tStart = Math.round(captureStart + (off + s.start_time) * 1000);
    const tEnd = Math.round(captureStart + (off + s.end_time) * 1000);
    rows.push({
      t_start: tStart,
      t_end: tEnd,
      text: s.text.trim(),
      speaker: speakerAt((tStart + tEnd) / 2),
      speakers_seen: speakersBetween(tStart, tEnd),
      confidence: s.confidence ?? null,
      file: `${seg}.wav`,
    });
    lastEnd = Math.max(lastEnd, s.end_time);
  }
  // Without segments.csv, assume back-to-back files of the last known length.
  if (!offsets.has(seg)) fallbackOffset = off + Math.max(lastEnd, 30);
}

writeFileSync(join(out, "transcript.jsonl"), rows.map((r) => JSON.stringify(r)).join("\n") + (rows.length ? "\n" : ""));
const clock = (t: number) => {
  const s = Math.max(0, Math.round((t - captureStart) / 1000));
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
};
writeFileSync(
  join(out, "transcript.txt"),
  rows.map((r) => `[${clock(r.t_start as number)}] ${(r.speaker as string | null) ?? "?"}: ${r.text as string}`).join("\n") + "\n",
);
const labelled = rows.filter((r) => r.speaker).length;
const ambiguous = rows.filter((r) => (r.speakers_seen as string[]).length > 1).length;
console.log(
  `[merge] ${rows.length} segments, ${labelled} with a speaker, ${ambiguous} with more than one speaker seen -> ${join(out, "transcript.jsonl")}`,
);
