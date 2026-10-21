/**
 * State Module
 * 
 * JSON Patch engine (RFC 6902) and state store for Allternit-IX.
 */

export {
  createStateStore,
  getStateStore,
  clearStateStore,
  type StateStore,
  type StateStoreConfig,
} from './store';

export {
  applyPatch,
  applyPatchToStore,
  applyOperation,
  generatePatch,
  invertPatch,
  validatePatch,
  validateOperation,
  getValueAtPath,
  setValueAtPath,
  decodeJsonPointer,
  encodeJsonPointer,
  type JSONPatch,
  type JSONPatchOperation,
  type PatchResult,
} from './patch';
