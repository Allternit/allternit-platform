/**
 * Settings Module
 * 
 * Manages user settings in `.gizzi/settings.json` (project level)
 * and `~/.config/gizzi-code/settings.json` (global level).
 * 
 * Similar to gizzi-code's `.claude.json` system.
 */

import path from "path"
import { Global } from "@/runtime/context/global"
import z from "zod/v4"
import { Filesystem } from "@/shared/util/filesystem"
import { Log } from "@/shared/util/log"
import { Instance } from "@/runtime/context/project/instance"
import { GlobalBus } from "@/shared/bus/global"
import { mergeDeep } from "remeda"

const log = Log.create({ service: "settings" })

// Settings schema
export const SettingsSchema = z.object({
  // User preferences
  user: z.object({
    firstName: z.string().optional(),
    lastName: z.string().optional(),
    preferredName: z.string().optional(),
    email: z.string().email().optional(),
    organization: z.string().optional(),
  }).optional(),
  
  // Model preferences
  model: z.object({
    default: z.string().optional(), // provider/model format
    favorites: z.array(z.string()).optional(),
    lastUsed: z.string().optional(),
  }).optional(),
  
  // Onboarding state
  onboarding: z.object({
    completed: z.boolean().optional(),
    completedAt: z.number().optional(),
    skipped: z.boolean().optional(),
  }).optional(),
  
  // UI preferences
  ui: z.object({
    theme: z.string().optional(),
    welcomeShown: z.boolean().optional(),
    tipsHistory: z.record(z.string(), z.number()).optional(),
  }).optional(),
  
  // Stats
  stats: z.object({
    numStartups: z.number().optional(),
    firstUsedAt: z.number().optional(),
    lastUsedAt: z.number().optional(),
  }).optional(),
  
  // Custom API key responses
  customApiKeyResponses: z.object({
    approved: z.array(z.string()).optional(),
    rejected: z.array(z.string()).optional(),
  }).optional(),
})

export type Settings = z.infer<typeof SettingsSchema>

const SETTINGS_FILE = "settings.json"
const DOT_GIZZI_DIR = ".gizzi"

// Default settings
const defaultSettings: Settings = {
  user: {},
  model: {
    favorites: [],
  },
  onboarding: {
    completed: false,
  },
  ui: {
    welcomeShown: false,
    tipsHistory: {},
  },
  stats: {
    numStartups: 0,
  },
}

/**
 * Get global settings file path
 */
function getGlobalSettingsPath(): string {
  return path.join(Global.Path.config, SETTINGS_FILE)
}

/**
 * Get project settings file path
 */
function getProjectSettingsPath(): string {
  try {
    const worktree = Instance.worktree
    if (worktree === "/") return ""
    return path.join(worktree, DOT_GIZZI_DIR, SETTINGS_FILE)
  } catch {
    return ""
  }
}

/**
 * Load settings from a file
 */
async function loadSettingsFile(filePath: string): Promise<Partial<Settings> | null> {
  if (!filePath) return null
  
  try {
    const data = await Filesystem.readJson<unknown>(filePath)
    if (!data) return null
    
    const parsed = SettingsSchema.safeParse(data)
    if (!parsed.success) {
      log.warn("Invalid settings file", { 
        path: filePath, 
        errors: parsed.error.issues 
      })
      return null
    }
    
    return parsed.data
  } catch (error) {
    return null
  }
}

/**
 * Save settings to a file
 */
async function saveSettingsFile(filePath: string, settings: Settings): Promise<void> {
  try {
    // Ensure directory exists
    const dir = path.dirname(filePath)
    await Filesystem.mkdir(dir, { recursive: true })
    
    await Filesystem.write(filePath, JSON.stringify(settings, null, 2))
  } catch (error) {
    log.error("Failed to save settings", { 
      path: filePath,
      error: error instanceof Error ? error.message : String(error) 
    })
    throw error
  }
}

/**
 * Get merged settings (global + project)
 */
async function getMergedSettings(): Promise<Settings> {
  const global = await loadSettingsFile(getGlobalSettingsPath()) || {}
  const project = await loadSettingsFile(getProjectSettingsPath()) || {}
  
  // Project settings override global
  return mergeDeep(defaultSettings, mergeDeep(global, project))
}

export const SettingsManager = {
  /**
   * Get all settings (merged global + project)
   */
  async get(): Promise<Settings> {
    return getMergedSettings()
  },
  
  /**
   * Get only global settings
   */
  async getGlobal(): Promise<Settings> {
    const global = await loadSettingsFile(getGlobalSettingsPath())
    return mergeDeep(defaultSettings, global ?? {})
  },
  
  /**
   * Get only project settings
   */
  async getProject(): Promise<Settings> {
    const project = await loadSettingsFile(getProjectSettingsPath())
    return mergeDeep(defaultSettings, project ?? {})
  },
  
  /**
   * Update global settings
   */
  async updateGlobal(updates: Partial<Settings>): Promise<Settings> {
    const current = await loadSettingsFile(getGlobalSettingsPath()) || {}
    const merged = mergeDeep(defaultSettings, mergeDeep(current, updates))
    const validated = SettingsSchema.parse(merged)
    
    await saveSettingsFile(getGlobalSettingsPath(), validated)
    
    // Emit update event
    GlobalBus.emit("event", { payload: { type: "settings.updated", scope: "global", settings: validated } })
    
    return validated
  },
  
  /**
   * Update project settings
   */
  async updateProject(updates: Partial<Settings>): Promise<Settings> {
    const projectPath = getProjectSettingsPath()
    if (!projectPath) {
      throw new Error("No project context available")
    }
    
    const current = await loadSettingsFile(projectPath) || {}
    const merged = mergeDeep(defaultSettings, mergeDeep(current, updates))
    const validated = SettingsSchema.parse(merged)
    
    await saveSettingsFile(projectPath, validated)
    
    // Emit update event
    GlobalBus.emit("event", { payload: { type: "settings.updated", scope: "project", settings: validated } })
    
    return validated
  },
  
  /**
   * Update user info in global settings
   */
  async updateUser(user: Partial<Settings["user"]>): Promise<void> {
    await this.updateGlobal({ user })
  },
  
  /**
   * Set default model
   */
  async setDefaultModel(modelId: string): Promise<void> {
    await this.updateGlobal({
      model: { default: modelId }
    })
  },
  
  /**
   * Mark onboarding as complete
   */
  async completeOnboarding(): Promise<void> {
    await this.updateGlobal({
      onboarding: {
        completed: true,
        completedAt: Date.now(),
      }
    })
  },
  
  /**
   * Increment startup counter
   */
  async incrementStartup(): Promise<void> {
    const settings = await this.getGlobal()
    await this.updateGlobal({
      stats: {
        ...settings.stats,
        numStartups: (settings.stats?.numStartups || 0) + 1,
        firstUsedAt: settings.stats?.firstUsedAt || Date.now(),
        lastUsedAt: Date.now(),
      }
    })
  },
  
  /**
   * Record a tip being shown
   */
  async recordTip(tipId: string): Promise<void> {
    const settings = await this.getGlobal()
    const tipsHistory = settings.ui?.tipsHistory || {}
    tipsHistory[tipId] = (tipsHistory[tipId] ?? 0) + 1
    
    await this.updateGlobal({
      ui: {
        ...settings.ui,
        tipsHistory,
      }
    })
  },
  
  /**
   * Get the effective default model
   */
  async getDefaultModel(): Promise<string | undefined> {
    const settings = await this.get()
    
    // First check settings
    if (settings.model?.default) {
      return settings.model.default
    }
    
    // Fall back to first available provider's first model
    return undefined
  },
  
  /**
   * Get user's preferred name for welcome message
   */
  async getPreferredName(): Promise<string | undefined> {
    const settings = await this.get()
    return settings.user?.preferredName 
      || settings.user?.firstName 
      || undefined
  },
}
