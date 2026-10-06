/**
 * Ambient type declarations for packages that are referenced at build/type-check
 * time but not installed in node_modules. These shims keep the project
 * type-checking while the real packages are wired up.
 */

declare module '@anthropic-ai/claude-agent-sdk' {
  export type PermissionMode = 'accept' | 'deny' | 'ask'
}

declare module 'execa' {
  export function execa(file: string, args?: any, options?: any): any
  export function execaSync(file: string, args?: any, options?: any): any
  const _default: any
  export default _default
}

declare module 'glob' {
  export interface GlobOptions {
    [key: string]: any
  }
  export function globSync(pattern: string, options?: GlobOptions): string[]
}

declare module '@allternit/extension' {
  export type PermissionMode = 'accept' | 'deny' | 'ask'
  export interface ClaudeForChromeContext {
    [key: string]: any
  }
  export interface Logger {
    [key: string]: any
  }
  export function createClaudeForChromeMcpServer(context: ClaudeForChromeContext): any
}

