/**
 * Allternit-IX SDK
 * 
 * Client SDK for Allternit-IX integration.
 */

// API Client
export {
  IXCapsuleClient,
  createIXClient,
  type IXCapsuleClientConfig,
  type IXCapsuleEvent,
  type CreateCapsuleRequest,
  type CreateCapsuleResponse,
  type CapsuleState,
  type JsonRenderRequest,
  type ActionRequest,
} from './api';

// React Integration
export {
  IXProvider,
  IXRenderer,
  useIX,
  useIXState,
  useIXAction,
} from './react';

// Re-export types
export type { UIRoot, JSONPatch } from '../types';
