/**
 * Session Storage
 * Production-quality session management with filesystem persistence
 */

import { writeFile, readFile, mkdir, access, constants } from 'fs/promises'
import { join } from 'path'
import { log } from '../runtime/util/log.js'
import { GlobalPaths } from '../runtime/context/global/paths.js'

export type UserType = 'anonymous' | 'authenticated' | 'admin' | 'ant' | 'beta'

export interface SessionData {
  sessionId: string
  userId?: string
  userType: UserType
  email?: string
  accessToken?: string
  refreshToken?: string
  createdAt: number
  lastActivity: number
  expiresAt?: number
  metadata?: Record<string, unknown>
}

export interface UserPreferences {
  theme?: string
  fontSize?: number
  autoSave?: boolean
  notifications?: boolean
  telemetry?: boolean
  [key: string]: unknown
}

export interface Message {
  id: string
  role: 'user' | 'assistant' | 'system'
  content: string
  timestamp: number
  attachments?: Attachment[]
}

export interface Attachment {
  id: string
  filename: string
  contentType: string
  size: number
  path: string
}

// In-memory cache
const memoryStorage = new Map<string, unknown>()
let currentSession: SessionData | null = null
let preferencesCache: UserPreferences | null = null

// Config directory
// ~/.config/gizzi-code. Files from the pre-rename ~/.config/gizzi are copied
// here by Global.init().
function getConfigDir(): string {
  return GlobalPaths.config
}

function getSessionFilePath(): string {
  return join(getConfigDir(), 'session.json')
}

function getPreferencesFilePath(): string {
  return join(getConfigDir(), 'preferences.json')
}

function getMessagesFilePath(): string {
  return join(getConfigDir(), 'messages.json')
}

async function ensureConfigDir(): Promise<void> {
  const configDir = getConfigDir()
  try {
    await access(configDir, constants.F_OK)
  } catch {
    await mkdir(configDir, { recursive: true })
  }
}

// Session Management
export async function saveSession(data: SessionData): Promise<void> {
  await ensureConfigDir()
  
  const session: SessionData = {
    ...data,
    lastActivity: Date.now(),
  }
  
  currentSession = session
  memoryStorage.set('__session', session)
  
  // Persist to disk
  try {
    await writeFile(getSessionFilePath(), JSON.stringify(session, null, 2))
    log('debug', 'Session persisted to disk', { sessionId: data.sessionId })
  } catch (error) {
    log('error', 'Failed to persist session', error)
    throw new Error('Failed to save session')
  }
}

export async function loadSession(): Promise<SessionData | null> {
  // Check memory first
  if (currentSession) {
    return currentSession
  }
  
  // Try to load from disk
  try {
    const data = await readFile(getSessionFilePath(), 'utf8')
    const session: SessionData = JSON.parse(data)
    
    // Check if session is expired
    if (session.expiresAt && session.expiresAt < Date.now()) {
      log('info', 'Session expired')
      await clearSession()
      return null
    }
    
    currentSession = session
    memoryStorage.set('__session', session)
    
    // Update last activity
    session.lastActivity = Date.now()
    await saveSession(session)
    
    return session
  } catch (error) {
    // File doesn't exist or is corrupted
    return null
  }
}

export async function clearSession(): Promise<void> {
  currentSession = null
  memoryStorage.delete('__session')
  
  try {
    await writeFile(getSessionFilePath(), '{}')
    log('info', 'Session cleared')
  } catch (error) {
    log('error', 'Failed to clear session file', error)
  }
}

export function hasActiveSession(): boolean {
  if (currentSession) return true
  return memoryStorage.has('__session')
}

export function getSessionId(): string | undefined {
  return currentSession?.sessionId
}

export function getUserType(): UserType {
  return currentSession?.userType || 'anonymous'
}

export function isUserType(type: UserType): boolean {
  return getUserType() === type
}

export function isAuthenticated(): boolean {
  return currentSession?.userType === 'authenticated' || 
         currentSession?.userType === 'admin' ||
         currentSession?.userType === 'ant'
}

export async function updateLastActivity(): Promise<void> {
  if (currentSession) {
    currentSession.lastActivity = Date.now()
    await saveSession(currentSession)
  }
}

export function getLastActivity(): number | undefined {
  return currentSession?.lastActivity
}

// Preferences Management
export async function savePreferences(prefs: UserPreferences): Promise<void> {
  await ensureConfigDir()
  preferencesCache = prefs
  
  try {
    await writeFile(getPreferencesFilePath(), JSON.stringify(prefs, null, 2))
    log('debug', 'Preferences saved')
  } catch (error) {
    log('error', 'Failed to save preferences', error)
    throw new Error('Failed to save preferences')
  }
}

export async function loadPreferences(): Promise<UserPreferences> {
  if (preferencesCache) {
    return preferencesCache
  }
  
  try {
    const data = await readFile(getPreferencesFilePath(), 'utf8')
    const prefs: UserPreferences = JSON.parse(data)
    preferencesCache = prefs
    return prefs
  } catch {
    // Return defaults
    return {
      theme: 'dark',
      fontSize: 14,
      autoSave: true,
      notifications: true,
      telemetry: true,
    }
  }
}

export async function getPreference<T>(key: string, defaultValue: T): Promise<T> {
  const prefs = await loadPreferences()
  return (prefs[key] as T) ?? defaultValue
}

export async function setPreference<T>(key: string, value: T): Promise<void> {
  const prefs = await loadPreferences()
  prefs[key] = value
  await savePreferences(prefs)
}

// Message Persistence
export async function saveMessages(messages: Message[]): Promise<void> {
  await ensureConfigDir()
  
  try {
    await writeFile(getMessagesFilePath(), JSON.stringify(messages, null, 2))
  } catch (error) {
    log('error', 'Failed to save messages', error)
  }
}

export async function loadMessages(): Promise<Message[]> {
  try {
    const data = await readFile(getMessagesFilePath(), 'utf8')
    return JSON.parse(data)
  } catch {
    return []
  }
}

export async function addMessage(message: Message): Promise<void> {
  const messages = await loadMessages()
  messages.push(message)
  
  // Keep only last 1000 messages
  if (messages.length > 1000) {
    messages.splice(0, messages.length - 1000)
  }
  
  await saveMessages(messages)
}

// Low-level storage (for other uses)
export async function getSessionItem<T>(key: string): Promise<T | undefined> {
  // Check memory first
  if (memoryStorage.has(key)) {
    return memoryStorage.get(key) as T
  }
  
  // Could extend to read from separate files per key
  return undefined
}

export async function setSessionItem<T>(key: string, value: T): Promise<void> {
  memoryStorage.set(key, value)
  
  // Persist to separate file
  await ensureConfigDir()
  try {
    const filePath = join(getConfigDir(), `${key}.json`)
    await writeFile(filePath, JSON.stringify(value, null, 2))
  } catch (error) {
    log('error', `Failed to persist ${key}`, error)
  }
}

export async function removeSessionItem(key: string): Promise<void> {
  memoryStorage.delete(key)
  
  try {
    const filePath = join(getConfigDir(), `${key}.json`)
    await writeFile(filePath, '{}')
  } catch {
    // Ignore
  }
}

// Default export
export default {
  // Session
  saveSession,
  loadSession,
  clearSession,
  hasActiveSession,
  getSessionId,
  getUserType,
  isUserType,
  isAuthenticated,
  updateLastActivity,
  getLastActivity,
  
  // Preferences
  savePreferences,
  loadPreferences,
  getPreference,
  setPreference,
  
  // Messages
  saveMessages,
  loadMessages,
  addMessage,
  
  // Low-level
  getSessionItem,
  setSessionItem,
  removeSessionItem,
}

// Merge-by-re-export: complete counterpart (local exports win on conflict)
export { MAX_TRANSCRIPT_READ_BYTES, adoptResumedSessionFile, buildConversationChain, cacheSessionTitle, checkResumeConsistency, cleanMessagesForLogging, clearAgentTranscriptSubdir, clearSessionMessagesCache, clearSessionMetadata, deleteRemoteAgentMetadata, doesMessageExistInSession, enrichLogs, extractAgentIdsFromMessages, extractTeammateTranscriptsFromTasks, fetchLogs, findUnresolvedToolUse, flushSessionStorage, getAgentTranscript, getAgentTranscriptPath, getCurrentSessionAgentColor, getCurrentSessionTag, getCurrentSessionTitle, getFirstMeaningfulUserMessageTextContent, getLastSessionLog, getLogByIndex, getNodeEnv, getProjectsDir, getSessionFilesLite, getSessionFilesWithMtime, getSessionIdFromLog, getTranscriptPath, getTranscriptPathForSession, hydrateFromCCRv2InternalEvents, hydrateRemoteSession, isChainParticipant, isCustomTitleEnabled, isEphemeralToolProgress, isLiteLog, isLoggableMessage, isTranscriptMessage, linkSessionToPR, listRemoteAgentMetadata, loadAllLogsFromSessionFile, loadAllProjectsMessageLogs, loadAllProjectsMessageLogsProgressive, loadAllSubagentTranscriptsFromDisk, loadFullLog, loadMessageLogs, loadSameRepoMessageLogs, loadSameRepoMessageLogsProgressive, loadSubagentTranscripts, loadTranscriptFile, loadTranscriptFromFile, reAppendSessionMetadata, readAgentMetadata, readRemoteAgentMetadata, recordAttributionSnapshot, recordContentReplacement, recordContextCollapseCommit, recordContextCollapseSnapshot, recordFileHistorySnapshot, recordQueueOperation, recordSidechainTranscript, recordTranscript, removeExtraFields, removeTranscriptMessage, resetProjectFlushStateForTesting, resetProjectForTesting, resetSessionFilePointer, restoreSessionMetadata, saveAgentColor, saveAgentName, saveAgentSetting, saveAiGeneratedTitle, saveCustomTitle, saveMode, saveTag, saveTaskSummary, saveWorktreeState, searchSessionsByCustomTitle, sessionIdExists, setAgentTranscriptSubdir, setInternalEventReader, setInternalEventWriter, setRemoteIngressUrlForTesting, setSessionFileForTesting, writeAgentMetadata, writeRemoteAgentMetadata } from "../shared/utils/sessionStorage.js";
export type { AgentMetadata, RemoteAgentMetadata, SessionLogResult, TeamInfo } from "../shared/utils/sessionStorage.js";
