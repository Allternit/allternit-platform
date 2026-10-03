import type { LocalCommandCall } from '../../types/command.js'
import { isLocalVoiceAvailable } from '../../services/localVoiceSTT.js'
import { settingsChangeDetector } from '../../utils/settings/changeDetector.js'
import {
  getInitialSettings,
  updateSettingsForSource,
} from '../../utils/settings/settings.js'

export const call: LocalCommandCall = async () => {
  const current = getInitialSettings().voiceEnabled === true
  const next = !current
  const result = updateSettingsForSource('userSettings', {
    voiceEnabled: next,
  })
  if (result.error) {
    return {
      type: 'text',
      value: `Could not update voice setting: ${result.error.message}`,
    }
  }
  settingsChangeDetector.notifyChange('userSettings')

  if (!next) {
    return { type: 'text', value: 'Voice dictation off.' }
  }

  const available = await isLocalVoiceAvailable()
  if (!available) {
    return {
      type: 'text',
      value:
        'Voice dictation on, but the local voice engine is not running. Start Allternit Desktop (or run services/voice); voice models download on first use. Hold Ctrl+Space or F8 to talk once it is available.',
    }
  }
  return {
    type: 'text',
    value:
      'Voice dictation on. Hold Ctrl+Space (or F8) to talk. Speech stays on this machine.',
  }
}
