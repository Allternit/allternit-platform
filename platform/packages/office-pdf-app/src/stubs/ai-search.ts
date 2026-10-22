/** Stub for @genoffice/ai-search — Genspark services are not ported. */
/* eslint-disable @typescript-eslint/no-explicit-any */

const NOT_AVAILABLE = 'not available in this build'

export function gskApiKey(): string | null {
  return null
}

export function hasGskAuth(): boolean {
  return false
}

export async function gskLogin(..._args: any[]): Promise<any> {
  throw new Error(NOT_AVAILABLE)
}

export async function gskLoginInfo(..._args: any[]): Promise<any> {
  return { loggedIn: false }
}

export async function gskSlideGenerate(..._args: any[]): Promise<any> {
  throw new Error(NOT_AVAILABLE)
}

export async function gskGenerateImage(..._args: any[]): Promise<any> {
  throw new Error(NOT_AVAILABLE)
}

export async function gskAnalyzeMedia(..._args: any[]): Promise<any> {
  throw new Error(NOT_AVAILABLE)
}

export async function webSearch(..._args: any[]): Promise<any> {
  return { results: [] }
}

// Same shape as @genoffice/ai-search's imageSearch (callers read .images).
export async function imageSearch(..._args: any[]): Promise<any> {
  return { images: [], unavailable: true }
}
