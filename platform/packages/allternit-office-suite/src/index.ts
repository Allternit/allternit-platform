// Host contract and context
export {
  OfficeHostProvider,
  useOfficeHost,
  useOfficeHostRequired,
  useOfficeAi,
  localStorageProvider,
} from './bridge/OfficeHostContext';
export type {
  OfficeHost,
  OpenedFile,
  OpenOptions,
  RecentFile,
  SaveOptions,
  OfficeAiClient,
  OfficeMessage,
  OfficeModelInfo,
  OfficeAppKey,
  OfficeModelOption,
  OfficeAgentLoop,
  OfficeAgentLoopOptions,
  OfficeAgentLoopEvents,
  OfficeAgentLoopConstructor,
  OfficeToolExecution,
  OfficeStorageProvider,
  XlsxEngineHost,
  XlsxSessionHandle,
  OfficeExtensionContext,
  OfficeExtensionDescriptor,
  OfficeHostChat,
  OfficeDesignSystem,
} from './bridge/types';

// Extension slot for the per-app AI chat section
export {
  OfficeAiSlot,
  useOfficeExtensions,
  registerActiveDocument,
  reportActiveDocument,
  getActiveDocument,
  useActiveDocument,
  createAllternitAssistantExtension,
  requestAssistantPreset,
  onAssistantPreset,
} from './extensions';
export type { OfficeAiSlotProps, ActiveDocumentInfo } from './extensions';

// Brand
export { AllternitBrandMark, type AllternitBrandMarkProps } from './components/AllternitBrandMark';

// AI configuration
export type { OfficeAiConfig } from './ai/types';

// Office app adapters (host-aware wrappers around the vendored apps)
export { DocsApp, type DocsAppProps } from './apps/DocsApp';
export { SheetsApp, type SheetsAppProps } from './apps/SheetsApp';
export { SlidesApp, type SlidesAppProps } from './apps/SlidesApp';
export { PdfApp, type PdfAppProps } from './apps/PdfApp';

// Allternit Sign
export { SignApp, type SignAppProps } from './sign/SignApp';
export {
  buildSignedPdf,
  canvasToPngBytes,
  initPdfWorker,
  loadPdfDocument,
  pngBytesToDataUrl,
  renderPageToCanvas,
  type Signer,
  type SignatureField,
} from './sign/pdf-signing';

// Browser host helper for standalone surfaces
export { createBrowserHost, type BrowserHostOptions } from './hosts/browser';

// Default theme for standalone surfaces
import './theme/allternit-office-suite.css';
