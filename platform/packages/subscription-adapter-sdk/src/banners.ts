// §A3.1 — detectBanners: regex packs → QuotaSignal classification.
import type { QuotaSignal } from "@allternit/subscription-fabric-contracts";

export type QuotaSignalKind = QuotaSignal["kind"];

export interface BannerPattern {
  kind: QuotaSignalKind;
  pattern: RegExp;
  // The provider won't take a message while this shows ("you've reached your
  // limit"), unlike a warning ("approaching your limit"). Seen before a send,
  // the task stops without typing.
  blocksSend?: boolean;
}

export interface BannerMatch {
  kind: QuotaSignalKind;
  raw_excerpt: string;
  blocksSend?: boolean;
}

export interface BannerClassifier {
  classify(text: string): BannerMatch | null;
}

export function createBannerClassifier(pack: BannerPattern[]): BannerClassifier {
  return {
    classify(text: string): BannerMatch | null {
      for (const { kind, pattern, blocksSend } of pack) {
        if (pattern.test(text)) {
          return { kind, raw_excerpt: text.slice(0, 500), ...(blocksSend ? { blocksSend } : {}) };
        }
      }
      return null;
    },
  };
}
