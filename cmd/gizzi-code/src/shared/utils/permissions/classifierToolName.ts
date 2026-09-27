/**
 * Name of the auto-mode classifier's report tool. A leaf module so
 * classifierDecision.ts can use it without importing yoloClassifier.ts, which
 * sits in an import cycle with it (permissions → classifierDecision →
 * yoloClassifier → … → permissions).
 */
export const YOLO_CLASSIFIER_TOOL_NAME = 'classify_result'
