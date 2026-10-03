// Local speech-to-text through the Allternit voice-service sidecar
// (services/voice: Silero VAD + Moonshine/Parakeet on sherpa-onnx, models
// download on first use). Never talks to Anthropic voice_stream.

const DEFAULT_SIDECAR = 'http://127.0.0.1:8001'
const SAMPLE_RATE = 16_000
const CHANNELS = 1

export function sidecarBaseUrl(): string {
  return (
    process.env.ALLTERNIT_VOICE_URL ||
    process.env.VOICE_URL ||
    DEFAULT_SIDECAR
  ).replace(/\/+$/, '')
}

export async function isSidecarHealthy(timeoutMs = 1500): Promise<boolean> {
  try {
    const response = await fetch(`${sidecarBaseUrl()}/health`, {
      signal: AbortSignal.timeout(timeoutMs),
    })
    return response.ok
  } catch {
    return false
  }
}

export async function isLocalVoiceAvailable(): Promise<boolean> {
  return isSidecarHealthy()
}

export function pcm16leToWav(
  pcm: Buffer,
  sampleRate = SAMPLE_RATE,
  channels = CHANNELS,
): Buffer {
  if (pcm.length >= 12 && pcm.subarray(0, 4).toString() === 'RIFF') {
    return pcm
  }
  const dataLen = pcm.length
  const header = Buffer.alloc(44)
  header.write('RIFF', 0)
  header.writeUInt32LE(36 + dataLen, 4)
  header.write('WAVE', 8)
  header.write('fmt ', 12)
  header.writeUInt32LE(16, 16)
  header.writeUInt16LE(1, 20)
  header.writeUInt16LE(channels, 22)
  header.writeUInt32LE(sampleRate, 24)
  header.writeUInt32LE(sampleRate * channels * 2, 28)
  header.writeUInt16LE(channels * 2, 32)
  header.writeUInt16LE(16, 34)
  header.write('data', 36)
  header.writeUInt32LE(dataLen, 40)
  return Buffer.concat([header, pcm])
}

export async function transcribePcm(
  pcm: Buffer,
  language = 'en',
): Promise<string> {
  const wav = pcm16leToWav(pcm)
  const body = new FormData()
  body.append(
    'audio',
    new Blob([new Uint8Array(wav)], { type: 'audio/wav' }),
    'utterance.wav',
  )
  body.append('language', language)
  let response: Response
  try {
    // First use downloads the small voice pack (~142 MB), so allow time.
    response = await fetch(`${sidecarBaseUrl()}/v1/stt`, {
      method: 'POST',
      body,
      signal: AbortSignal.timeout(300_000),
    })
  } catch (err) {
    throw new Error(
      `Local voice engine is not running (${String(err)}). Start Allternit Desktop or run services/voice.`,
    )
  }
  if (!response.ok) {
    const detail = await response.text().catch(() => '')
    throw new Error(`Local voice engine failed: HTTP ${response.status} ${detail}`.trim())
  }
  const json = (await response.json()) as { text?: string }
  return (json.text ?? '').trim()
}
