// The in-page media override. Passed to Playwright's context.addInitScript(),
// so it runs in every frame before any page script. Playwright serialises the
// function with toString(): it must stay self-contained (no imports, no
// closures over module scope).
//
// Camera: a 1280x720 canvas streamed with canvas.captureStream(15). It shows
// our real bot: the allternit-ai /meeting-tile page (the Bot Mode stage),
// screencast from its own window by tile-feed.ts and handed to
// window.__gizziTileFrame(jpegBase64); the newest frame is drawn every tick.
// Until the first frame arrives the canvas is the graphite page colour.
// Microphone: a WebAudio MediaStreamDestination track, silent until
// window.__gizziSpeak(url) plays a file into it (the voice-reply path);
// window.__gizziOutLevel() reads that voice's level for the tile. No real
// device is ever opened, so no v4l2loopback or virtual mic is needed in the pod.

export interface GizziMediaOptions {
  name: string;
  state: string;
}

declare global {
  interface Window {
    __gizziSpeak?: (url: string) => Promise<{ durationSec: number }>;
    __gizziSetState?: (state: string) => void;
    __gizziTileFrame?: (jpegBase64: string) => Promise<boolean>;
    __gizziOutLevel?: () => number;
    __gizziMedia?: { installed: boolean; framesDrawn: number; tileFrames: number; state: string };
  }
}

export function installGizziMedia(opts: GizziMediaOptions): void {
  const md = navigator.mediaDevices;
  if (!md || window.__gizziMedia) return;

  const W = 1280;
  const H = 720;
  const FPS = 15;
  const CAM_ID = "gizzi-bot-camera";
  const MIC_ID = "gizzi-voice";
  const GROUP_ID = "gizzi";
  const EMPTY = "#1f1f1e"; // the graphite --bg-primary the tile draws on

  const state = { installed: true, framesDrawn: 0, tileFrames: 0, state: opts.state };
  window.__gizziMedia = state;

  // ---- camera: the latest tile frame ----------------------------------------
  let canvas: HTMLCanvasElement | null = null;
  let ctx2d: CanvasRenderingContext2D | null = null;
  let latest: ImageBitmap | null = null;

  function draw(): void {
    if (!ctx2d) return;
    if (latest) {
      ctx2d.drawImage(latest, 0, 0, W, H);
    } else {
      ctx2d.fillStyle = EMPTY;
      ctx2d.fillRect(0, 0, W, H);
    }
    state.framesDrawn++;
  }

  window.__gizziTileFrame = async (b64: string) => {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/jpeg" }));
    latest?.close();
    latest = bitmap;
    state.tileFrames++;
    draw();
    return true;
  };

  function ensureCanvas(): HTMLCanvasElement {
    if (canvas) return canvas;
    canvas = document.createElement("canvas");
    canvas.width = W;
    canvas.height = H;
    ctx2d = canvas.getContext("2d");
    draw();
    // setInterval, not requestAnimationFrame: rAF stops when the tab is not
    // visible, which would freeze the camera.
    setInterval(draw, Math.round(1000 / FPS));
    return canvas;
  }

  // ---- microphone: a WebAudio destination ----------------------------------
  let audioCtx: AudioContext | null = null;
  let dest: MediaStreamAudioDestinationNode | null = null;

  function ensureAudio(): MediaStreamAudioDestinationNode {
    if (dest && audioCtx) return dest;
    audioCtx = new AudioContext({ sampleRate: 48000 });
    dest = audioCtx.createMediaStreamDestination();
    // A constant zero source keeps the track producing (silent) samples, so
    // the call sees an open, quiet mic rather than a stalled one.
    const silence = audioCtx.createConstantSource();
    silence.offset.value = 0;
    silence.connect(dest);
    silence.start();
    void audioCtx.resume().catch(() => undefined);
    return dest;
  }

  window.__gizziSpeak = async (url: string) => {
    const d = ensureAudio();
    const ac = audioCtx as AudioContext;
    if (ac.state !== "running") await ac.resume();
    const res = await fetch(url);
    if (!res.ok) throw new Error(`__gizziSpeak: ${res.status} for ${url}`);
    const buf = await ac.decodeAudioData(await res.arrayBuffer());
    const src = ac.createBufferSource();
    src.buffer = buf;
    src.connect(d); // into the outgoing mic only, never to the speakers
    // A side branch for the tile's voice level; the mic path above is unchanged.
    const analyser = ac.createAnalyser();
    analyser.fftSize = 1024;
    src.connect(analyser);
    const samples = new Float32Array(analyser.fftSize);
    window.__gizziOutLevel = () => {
      analyser.getFloatTimeDomainData(samples);
      let sum = 0;
      for (const x of samples) sum += x * x;
      return Math.min(1, Math.sqrt(sum / samples.length) * 3);
    };
    await new Promise<void>((resolve) => {
      src.onended = () => resolve();
      src.start();
    });
    window.__gizziOutLevel = () => 0;
    return { durationSec: buf.duration };
  };

  window.__gizziSetState = (s: string) => {
    state.state = s;
  };

  // ---- the overrides ---------------------------------------------------------
  function tagTrack(track: MediaStreamTrack, deviceId: string): MediaStreamTrack {
    const orig = track.getSettings.bind(track);
    track.getSettings = () => ({ ...orig(), deviceId, groupId: GROUP_ID });
    return track;
  }

  const origEnumerate = md.enumerateDevices.bind(md);

  md.getUserMedia = async (constraints?: MediaStreamConstraints) => {
    const out = new MediaStream();
    if (constraints?.video) {
      const v = ensureCanvas().captureStream(FPS).getVideoTracks()[0];
      out.addTrack(tagTrack(v, CAM_ID));
    }
    if (constraints?.audio) {
      // A clone per call: the page can stop its copy without killing ours.
      const a = ensureAudio().stream.getAudioTracks()[0].clone();
      out.addTrack(tagTrack(a, MIC_ID));
    }
    if (out.getTracks().length === 0) {
      throw new DOMException("No audio or video requested", "TypeError");
    }
    return out;
  };

  md.enumerateDevices = async () => {
    let real: MediaDeviceInfo[] = [];
    try {
      real = await origEnumerate();
    } catch {
      real = [];
    }
    const fake = (deviceId: string, kind: MediaDeviceKind, label: string): MediaDeviceInfo =>
      ({
        deviceId,
        kind,
        label,
        groupId: GROUP_ID,
        toJSON() {
          return { deviceId, kind, label, groupId: GROUP_ID };
        },
      }) as MediaDeviceInfo;
    return [
      fake(MIC_ID, "audioinput", `${opts.name} voice`),
      fake(CAM_ID, "videoinput", `${opts.name} camera`),
      // Keep the real outputs: Chrome plays call audio into the default sink
      // (the meet_out null sink in the pod).
      ...real.filter((d) => d.kind === "audiooutput"),
    ];
  };
}
