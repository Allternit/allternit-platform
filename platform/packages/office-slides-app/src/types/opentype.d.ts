/**
 * opentype.js ships no type declarations (its package.json has no types field and the
 * dist build is plain JS). Only this package's
 * font metrics probe consumes it, and only `parse()`. If opentype.js gains bundled
 * types or the consumer starts using more of the API, replace this with a real
 * dependency on @types/opentype.js.
 */
declare module 'opentype.js' {
  export interface Font {
    // Opaque here — callers cast the parse result to their own structural view
    // (OpentypeFontLike in @allternit/office-pptx-render).
  }
  export function parse(buffer: ArrayBuffer | Uint8Array): Font;
}
