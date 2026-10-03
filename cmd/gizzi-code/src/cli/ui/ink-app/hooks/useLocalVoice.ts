// Hold-to-talk dictation using the local voice sidecar (sherpa-onnx STT).
// Same return shape as useVoice so useVoiceIntegration can swap backends.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useSetVoiceState } from '../context/voice.js'
import { transcribePcm } from '../services/localVoiceSTT.js'
import { logForDebugging } from '../utils/debug.js'
import { toError } from '../utils/errors.js'
import { logError } from '../utils/log.js'

type VoiceModule = typeof import('../services/voice.js')
let voiceModule: VoiceModule | null = null

type VoiceState = 'idle' | 'recording' | 'processing'

type UseVoiceOptions = {
  onTranscript: (text: string) => void
  onError?: (message: string) => void
  enabled: boolean
  focusMode: boolean
}

type UseVoiceReturn = {
  state: VoiceState
  handleKeyEvent: (fallbackMs?: number) => void
}

const RELEASE_TIMEOUT_MS = 200
const REPEAT_FALLBACK_MS = 600
export const FIRST_PRESS_FALLBACK_MS = 2000

export function useLocalVoice({
  onTranscript,
  onError,
  enabled,
}: UseVoiceOptions): UseVoiceReturn {
  const [state, setState] = useState<VoiceState>('idle')
  const stateRef = useRef<VoiceState>('idle')
  const chunksRef = useRef<Buffer[]>([])
  const releaseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const repeatFallbackTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  )
  const onTranscriptRef = useRef(onTranscript)
  const onErrorRef = useRef(onError)
  const setVoiceState = useSetVoiceState()
  const sessionGenRef = useRef(0)

  onTranscriptRef.current = onTranscript
  onErrorRef.current = onError

  function updateState(next: VoiceState): void {
    stateRef.current = next
    setState(next)
    setVoiceState(prev =>
      prev.voiceState === next ? prev : { ...prev, voiceState: next },
    )
  }

  useEffect(() => {
    if (enabled && !voiceModule) {
      void import('../services/voice.js').then(mod => {
        voiceModule = mod
      })
    }
  }, [enabled])

  const finishRecording = useCallback((): void => {
    if (stateRef.current !== 'recording') return
    const gen = sessionGenRef.current
    updateState('processing')
    voiceModule?.stopRecording()
    const chunks = chunksRef.current
    chunksRef.current = []
    const pcm = Buffer.concat(chunks)

    void (async () => {
      try {
        if (pcm.length < 1600) {
          if (sessionGenRef.current !== gen) return
          onErrorRef.current?.('No speech was detected. Voice stopped.')
          updateState('idle')
          return
        }
        const text = await transcribePcm(pcm)
        if (sessionGenRef.current !== gen) return
        if (text) {
          onTranscriptRef.current(text)
        } else {
          onErrorRef.current?.('No speech was detected. Voice stopped.')
        }
      } catch (err) {
        logError(toError(err))
        if (sessionGenRef.current === gen) {
          onErrorRef.current?.(
            err instanceof Error
              ? err.message
              : 'Voice transcription failed.',
          )
        }
      } finally {
        if (sessionGenRef.current === gen) updateState('idle')
        setVoiceState(prev =>
          prev.voiceInterimTranscript === '' && prev.voiceAudioLevels.length === 0
            ? prev
            : { ...prev, voiceInterimTranscript: '', voiceAudioLevels: [] },
        )
      }
    })()
  }, [setVoiceState])

  const startRecordingSession = useCallback(async (): Promise<void> => {
    if (stateRef.current !== 'idle') return
    const mod = voiceModule ?? (await import('../services/voice.js'))
    voiceModule = mod
    const available = await mod.checkRecordingAvailability()
    if (!available.available) {
      onErrorRef.current?.(
        available.reason ??
          'Microphone is not available. Check System Settings → Privacy → Microphone.',
      )
      return
    }
    chunksRef.current = []
    sessionGenRef.current += 1
    const started = await mod.startRecording(
      (chunk: Buffer) => {
        chunksRef.current.push(chunk)
        const rms = rmsLevel(chunk)
        setVoiceState(prev => ({
          ...prev,
          voiceAudioLevels: [...prev.voiceAudioLevels.slice(-7), rms],
        }))
      },
      () => {
        // Native silence end — ignore in push-to-talk; finish on key release.
      },
      { silenceDetection: false },
    )
    if (!started) {
      onErrorRef.current?.(
        'Could not start microphone capture. Check that Gizzi has microphone access.',
      )
      return
    }
    logForDebugging('[local-voice] recording started')
    updateState('recording')
  }, [setVoiceState])

  const handleKeyEvent = useCallback(
    (fallbackMs?: number): void => {
      if (!enabled) return
      if (releaseTimerRef.current) {
        clearTimeout(releaseTimerRef.current)
        releaseTimerRef.current = null
      }
      if (repeatFallbackTimerRef.current) {
        clearTimeout(repeatFallbackTimerRef.current)
        repeatFallbackTimerRef.current = null
      }
      if (stateRef.current === 'idle') {
        void startRecordingSession()
      }
      const delay = fallbackMs ?? REPEAT_FALLBACK_MS
      releaseTimerRef.current = setTimeout(() => {
        releaseTimerRef.current = null
        finishRecording()
      }, RELEASE_TIMEOUT_MS)
      repeatFallbackTimerRef.current = setTimeout(() => {
        repeatFallbackTimerRef.current = null
        if (!releaseTimerRef.current && stateRef.current === 'recording') {
          finishRecording()
        }
      }, delay)
    },
    [enabled, finishRecording, startRecordingSession],
  )

  useEffect(() => {
    if (enabled) return
    sessionGenRef.current += 1
    if (stateRef.current === 'recording') {
      voiceModule?.stopRecording()
      chunksRef.current = []
      updateState('idle')
    }
  }, [enabled])

  useEffect(() => {
    return () => {
      sessionGenRef.current += 1
      if (releaseTimerRef.current) clearTimeout(releaseTimerRef.current)
      if (repeatFallbackTimerRef.current)
        clearTimeout(repeatFallbackTimerRef.current)
      voiceModule?.stopRecording()
    }
  }, [])

  return { state, handleKeyEvent }
}

function rmsLevel(chunk: Buffer): number {
  if (chunk.length < 2) return 0
  let sum = 0
  const samples = Math.floor(chunk.length / 2)
  for (let i = 0; i < samples; i++) {
    const sample = chunk.readInt16LE(i * 2) / 32768
    sum += sample * sample
  }
  return Math.min(1, Math.sqrt(sum / samples) * 4)
}
