/** DTCG (Design Tokens Community Group) subset used by look packs. */

export type TokenKind = "color" | "typography" | "radius" | "spacing";

export type DtcgValue = string | number | string[] | { value: number; unit: string };

export interface DtcgToken {
  $value: DtcgValue;
  $type?: string;
  $description?: string;
}

export interface DtcgGroup {
  $type?: string;
  [key: string]: DtcgToken | DtcgGroup | string | undefined;
}

/** A DTCG document: nested groups whose leaves carry `$value`. */
export type DtcgDocument = DtcgGroup;

export interface LookPack {
  /** Lowercase letters, digits and dashes. Used as the `data-look-pack` scope. */
  id: string;
  name: string;
  version?: string;
  /** Light (default) tokens. */
  tokens: DtcgDocument;
  /** Optional dark-mode overrides, same shape as `tokens`. */
  dark?: DtcgDocument;
}

export interface ImportedToken {
  path: string[];
  kind: TokenKind;
  cssVar: string;
  value: string;
}

export interface DtcgImport {
  tokens: ImportedToken[];
  vars: Record<string, string>;
  issues: string[];
}

export type FindingSeverity = "error" | "warning" | "info";

export interface LookPackFinding {
  severity: FindingSeverity;
  code: string;
  message: string;
  subject?: string;
}
