import { z } from "zod/v4"

/**
 * Wrap `cb` so its input is validated by `schema` first.
 * For an async `cb`, a validation error comes back as a rejected promise, not a
 * synchronous throw, so `fnAsync(bad).catch(() => undefined)` handles it like any
 * other failure. (A sync throw skipped those `.catch` handlers: `gizzi teleport`
 * with a Claude session id failed with "must start with ses" instead of falling back.)
 */
export function fn<T extends z.ZodType, Result>(schema: T, cb: (input: z.infer<T>) => Result) {
  const isAsync = cb.constructor?.name === "AsyncFunction"
  const result = (input: z.infer<T>): Result => {
    if (!isAsync) return cb(schema.parse(input))
    const parsed = schema.safeParse(input)
    if (!parsed.success) return Promise.reject(parsed.error) as Result
    return cb(parsed.data)
  }
  result.force = (input: z.infer<T>) => cb(input)
  result.schema = schema
  return result
}
