/**
 * Git DAG Tracker
 * 
 * Tracks commit DAG for agent collaboration.
 * Integrates agenthub-style DAG with the Allternit Factory DAG.
 * 
 * Features:
 * - Track commit parent/child relationships
 * - Find frontier commits (leaves)
 * - Trace lineage (ancestry)
 * - Map to Allternit Factory DAG nodes
 */

import { exec } from 'child_process'
import { promisify } from 'util'
import { Log } from '@/shared/util/log'
import { Bus } from '@/shared/bus'
import { BusEvent } from '@/shared/bus/bus-event'
import { z } from 'zod/v4'

const execAsync = promisify(exec)

export namespace GitDAGTracker {
  const log = Log.create({ service: 'git-dag' })

  // ============================================================================
  // Types
  // ============================================================================

  export interface CommitInfo {
    hash: string
    shortHash: string
    author: string
    authorEmail: string
    timestamp: number
    message: string
    parents: string[]
    children: string[]
    isFrontier: boolean
    agentId?: string
    railsNodeId?: string
  }

  export interface LineagePath {
    commitHash: string
    path: string[]
    depth: number
  }

  // ============================================================================
  // Events
  // ============================================================================

  export const CommitTracked = BusEvent.define(
    'git.dag.commit.tracked',
    z.object({
      hash: z.string(),
      agentId: z.string().optional(),
      railsNodeId: z.string().optional(),
    }),
  )

  export const FrontierUpdated = BusEvent.define(
    'git.dag.frontier.updated',
    z.object({
      frontierCommits: z.array(z.string()),
      count: z.number(),
    }),
  )

  // ============================================================================
  // State
  // ============================================================================

  const commitCache = new Map<string, CommitInfo>()
  const childrenIndex = new Map<string, Set<string>>() // parent -> children
  let frontierCache: Set<string> | null = null

  // ============================================================================
  // Core Functions
  // ============================================================================

  /**
   * Initialize DAG tracker for a repository
   */
  export async function initialize(repoPath: string): Promise<void> {
    log.info('Initializing Git DAG tracker', { repoPath })
    
    // Scan all commits
    await scanAllCommits(repoPath)
    
    // Build children index
    await buildChildrenIndex(repoPath)
    
    // Identify frontier commits
    await identifyFrontier(repoPath)
    
    log.info('Git DAG tracker initialized', {
      commitCount: commitCache.size,
      frontierCount: frontierCache?.size || 0,
    })
  }

  /**
   * Scan all commits in repository
   */
  async function scanAllCommits(repoPath: string): Promise<void> {
    const { stdout } = await execAsync(
      `git -C "${repoPath}" log --all --format='%H|%h|%an|%ae|%at|%s|%P'`,
    )
    
    const lines = stdout.trim().split('\n').filter(Boolean)
    
    for (const line of lines) {
      const parts = line.split('|')
      if (parts.length < 6) continue
      
      const hash = parts[0]
      const shortHash = parts[1]
      const author = parts[2]
      const authorEmail = parts[3]
      const timestamp = parseInt(parts[4]) * 1000
      const message = parts[5]
      const parents = parts[6] ? parts[6].split(' ') : []
      
      commitCache.set(hash, {
        hash,
        shortHash,
        author,
        authorEmail,
        timestamp,
        message,
        parents,
        children: [],
        isFrontier: false,
      })
    }
    
    log.info('Scanned commits', { count: commitCache.size })
  }

  /**
   * Build children index (reverse of parents)
   */
  async function buildChildrenIndex(repoPath: string): Promise<void> {
    for (const [hash, commit] of commitCache.entries()) {
      for (const parent of commit.parents) {
        if (!childrenIndex.has(parent)) {
          childrenIndex.set(parent, new Set())
        }
        childrenIndex.get(parent)!.add(hash)
        commit.children.push(hash)
      }
    }
  }

  /**
   * Identify frontier commits (commits with no children)
   */
  async function identifyFrontier(repoPath: string): Promise<void> {
    frontierCache = new Set<string>()
    
    for (const [hash, commit] of commitCache.entries()) {
      if (commit.children.length === 0) {
        commit.isFrontier = true
        frontierCache.add(hash)
      }
    }
    
    Bus.publish(FrontierUpdated, {
      frontierCommits: Array.from(frontierCache),
      count: frontierCache.size,
    })
  }

  // ============================================================================
  // Public API
  // ============================================================================

  /**
   * Get commit info
   */
  export function getCommit(hash: string): CommitInfo | undefined {
    return commitCache.get(hash)
  }

  /**
   * Get children of a commit
   */
  export function getChildren(hash: string): string[] {
    const commit = commitCache.get(hash)
    return commit ? commit.children : []
  }

  /**
   * Get parents of a commit
   */
  export function getParents(hash: string): string[] {
    const commit = commitCache.get(hash)
    return commit ? commit.parents : []
  }

  /**
   * Get frontier commits (leaves)
   */
  export function getFrontier(): string[] {
    return frontierCache ? Array.from(frontierCache) : []
  }

  /**
   * Get lineage (ancestry path to root)
   */
  export function getLineage(hash: string): LineagePath {
    const path: string[] = []
    let current = hash
    
    while (current) {
      path.push(current)
      const commit = commitCache.get(current)
      if (!commit || commit.parents.length === 0) break
      current = commit.parents[0] // Follow first parent
    }
    
    return {
      commitHash: hash,
      path,
      depth: path.length,
    }
  }

  /**
   * Track new commit
   */
  export async function trackCommit(
    repoPath: string,
    hash: string,
    agentId?: string,
  ): Promise<CommitInfo | null> {
    try {
      const { stdout } = await execAsync(
        `git -C "${repoPath}" show -s --format='%H|%h|%an|%ae|%at|%s|%P' ${hash}`,
      )
      
      const parts = stdout.trim().split('|')
      if (parts.length < 6) return null
      
      const commit: CommitInfo = {
        hash: parts[0],
        shortHash: parts[1],
        author: parts[2],
        authorEmail: parts[3],
        timestamp: parseInt(parts[4]) * 1000,
        message: parts[5],
        parents: parts[6] ? parts[6].split(' ') : [],
        children: [],
        isFrontier: true,
        agentId,
      }
      
      // Update cache
      commitCache.set(hash, commit)
      
      // Update children index
      for (const parent of commit.parents) {
        if (!childrenIndex.has(parent)) {
          childrenIndex.set(parent, new Set())
        }
        childrenIndex.get(parent)!.add(hash)
        
        const parentCommit = commitCache.get(parent)
        if (parentCommit) {
          parentCommit.children.push(hash)
          parentCommit.isFrontier = false
          frontierCache?.delete(parent)
        }
      }
      
      // Add to frontier
      frontierCache?.add(hash)
      
      // Publish event
      Bus.publish(CommitTracked, {
        hash,
        agentId,
        railsNodeId: commit.railsNodeId,
      })
      
      log.info('Tracked commit', { hash, agentId })
      
      return commit
    } catch (error: any) {
      log.error('Failed to track commit', { hash, error: error.message })
      return null
    }
  }

  /**
   * Map commit to Allternit Factory DAG node
   */
  export function mapToRailsNode(commitHash: string, railsNodeId: string): void {
    const commit = commitCache.get(commitHash)
    if (commit) {
      commit.railsNodeId = railsNodeId
      log.info('Mapped commit to Factory node', { commitHash, railsNodeId })
    }
  }

  /**
   * Get all commits by an agent
   */
  export function getAgentCommits(agentId: string): CommitInfo[] {
    return Array.from(commitCache.values()).filter(
      (commit) => commit.agentId === agentId,
    )
  }

  /**
   * Get commits between two points
   */
  export function getDiff(hashA: string, hashB: string): {
    hashA: CommitInfo | undefined
    hashB: CommitInfo | undefined
    commonAncestor?: string
  } {
    const commitA = commitCache.get(hashA)
    const commitB = commitCache.get(hashB)
    
    // Find common ancestor (simplified - first common parent)
    const lineageA = getLineage(hashA)
    const lineageB = getLineage(hashB)
    
    let commonAncestor: string | undefined
    for (const a of lineageA.path) {
      if (lineageB.path.includes(a)) {
        commonAncestor = a
        break
      }
    }
    
    return {
      hashA: commitA,
      hashB: commitB,
      commonAncestor,
    }
  }

  /**
   * Clear cache (for testing)
   */
  export function clearCache(): void {
    commitCache.clear()
    childrenIndex.clear()
    frontierCache = null
  }
}
