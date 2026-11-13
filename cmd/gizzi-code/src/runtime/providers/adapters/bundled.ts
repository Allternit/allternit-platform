/**
 * Bundled AI SDK Provider Imports
 *
 * All @ai-sdk/* imports are confined here.
 * No other file outside runtime/providers/ should import these directly.
 *
 * Supported providers: Anthropic, OpenAI, Google, Bedrock, Vertex, Azure/Foundry, Gizzi, Qwen, Kimi
 */

import type { Provider as SDK } from "ai"
export { NoSuchModelError, type Provider as SDK } from "ai"

import { createAnthropic } from "@ai-sdk/anthropic"
import { createGoogleGenerativeAI } from "@ai-sdk/google"
import { createOpenAI } from "@ai-sdk/openai"
import { createOpenAICompatible } from "@ai-sdk/openai-compatible"
import { createMistral } from "@ai-sdk/mistral"
import { createAmazonBedrock } from "@ai-sdk/amazon-bedrock"
import { createVertex } from "@ai-sdk/google-vertex"
import { createVertexAnthropic } from "@ai-sdk/google-vertex/anthropic"
import { createAzure } from "@ai-sdk/azure"
import { SelfhostInference } from "../selfhost-inference"

export type { LanguageModelV2 } from "@ai-sdk/provider"

/**
 * Map of npm package names to their SDK factory functions.
 * Used by getSDK() to instantiate providers without dynamic imports.
 *
 * Providers: Anthropic, OpenAI, Google, Mistral, Gizzi, Qwen, Kimi
 * (Qwen/Kimi/MiniMax/GLM use @ai-sdk/openai-compatible)
 */
export const BUNDLED_PROVIDERS: Record<string, (options: any) => SDK> = {
  "@ai-sdk/anthropic": createAnthropic as unknown as (options: any) => SDK,
  "@ai-sdk/google": createGoogleGenerativeAI as unknown as (options: any) => SDK,
  "@ai-sdk/openai": createOpenAI as unknown as (options: any) => SDK,
  "@ai-sdk/openai-compatible": createOpenAICompatible as unknown as (options: any) => SDK,
  "@ai-sdk/mistral": createMistral as unknown as (options: any) => SDK,
  // Self-hosted inference providers. An empty baseURL (the catalog default) would break
  // every request, so SelfhostInference.clean drops it before the SDK sees it.
  "@ai-sdk/amazon-bedrock": ((o: any) => createAmazonBedrock(SelfhostInference.clean(o))) as unknown as (options: any) => SDK,
  "@ai-sdk/google-vertex": ((o: any) => createVertex(SelfhostInference.clean(o))) as unknown as (options: any) => SDK,
  // Claude on Vertex (and the vertex-raw-predict gateway format): the SDK's Anthropic subpath.
  "@ai-sdk/google-vertex/anthropic": ((o: any) => createVertexAnthropic(SelfhostInference.clean(o))) as unknown as (options: any) => SDK,
  "@ai-sdk/azure": ((o: any) => createAzure(SelfhostInference.clean(o))) as unknown as (options: any) => SDK,
  // Azure AI Foundry: Claude via its Anthropic-compatible endpoint, GPT-class via Azure OpenAI.
  "@allternit/foundry-anthropic": ((o: any) => createAnthropic(SelfhostInference.foundryAnthropicOptions(o) as any)) as unknown as (options: any) => SDK,
  "@allternit/foundry-openai": ((o: any) => createAzure(SelfhostInference.foundryOpenAIOptions(o) as any)) as unknown as (options: any) => SDK,
}
