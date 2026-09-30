/**
 * @allternit/apps-ui — types for the view kit's web components. The components
 * themselves are framework-free and live in runtime/allternit-ui.js, which
 * apps-server inlines into a View (see `kitSource()` from "./sources").
 */

export const KIT_TAGS = ["allternit-card", "allternit-table", "allternit-list", "allternit-form", "allternit-detail"] as const;
export type KitTag = (typeof KIT_TAGS)[number];

/** Where to find things in the data. Every value is a key into the data, never code. */
export interface KitFields {
  title?: string;
  subtitle?: string;
  description?: string;
  /** Link target (https only). */
  url?: string;
  /** Image (https only). */
  image?: string;
  /** Extra key/value rows for Card and List. */
  meta?: string[];
  /** Table columns / Detail keys, in order. */
  columns?: string[];
  labels?: Record<string, string>;
}

export interface KitFormField {
  name: string;
  label?: string;
  type?: "text" | "textarea" | "number" | "email" | "date" | "checkbox" | "url";
  required?: boolean;
  options?: string[];
  value?: string | number;
}

export interface KitFormData {
  fields: KitFormField[];
  submitLabel?: string;
}

export interface KitElement<T = unknown> extends HTMLElement {
  data: T;
  fields: KitFields;
}

/** `allternit-submit` event detail fired by <allternit-form>. */
export interface KitSubmitDetail {
  values: Record<string, string | number | boolean>;
}

/** Create a kit element, set its props and append it. Needs the runtime script loaded. */
export function mount<T>(tag: KitTag, parent: Element, data: T, fields: KitFields = {}): KitElement<T> {
  const node = parent.ownerDocument.createElement(tag) as KitElement<T>;
  node.fields = fields;
  parent.appendChild(node);
  node.data = data;
  return node;
}
