import { describe, expect, test } from "bun:test"
import { Todo } from "../../src/runtime/session/todo"

const t = (o: Partial<Todo.Info> & { content: string }): Todo.Info => ({ status: "pending", priority: "medium", ...o })

describe("todowrite normalize", () => {
  test("refuses a cycle", () => {
    expect(() =>
      Todo.normalize([t({ id: "a", content: "a", blockedBy: ["b"] }), t({ id: "b", content: "b", blockedBy: ["a"] })]),
    ).toThrow(/cycle/)
  })
  test("refuses an unknown blockedBy id", () => {
    expect(() => Todo.normalize([t({ id: "a", content: "a", blockedBy: ["zzz"] })])).toThrow(/unknown id/)
  })
  test("old rows without new fields still read", () => {
    const { todos } = Todo.normalize([{ content: "old", status: "completed", priority: "low" }])
    expect(todos[0].id).toBe("t1")
    expect(todos[0].blockedBy).toBeUndefined()
  })
  test("keeps one in_progress", () => {
    const { todos, warnings } = Todo.normalize([
      t({ content: "a", status: "in_progress" }),
      t({ content: "b", status: "in_progress" }),
    ])
    expect(todos.map((x) => x.status)).toEqual(["in_progress", "pending"])
    expect(warnings.length).toBe(1)
  })
  test("accepts cancelled and interrupted", () => {
    expect(() => Todo.normalize([t({ content: "a", status: "cancelled" }), t({ content: "b", status: "interrupted" })])).not.toThrow()
  })
})
