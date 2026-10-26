// @ts-nocheck
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import path from "path"
import { InstructionPrompt } from "../../src/runtime/session/instruction"
import { Instance } from "../../src/runtime/context/project/instance"
import { Global } from "../../src/runtime/context/global"
import { tmpdir } from "../fixture/fixture"

describe("InstructionPrompt.systemPaths per-directory-level precedence", () => {
  test("honors each directory's own winner instead of one global filename choice", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "GIZZI.md"), "# Root (GIZZI.md)")
        await Bun.write(path.join(dir, "sub", "CLAUDE.md"), "# Sub (CLAUDE.md)")
      },
    })
    await Instance.provide({
      directory: path.join(tmp.path, "sub"),
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(tmp.path, "GIZZI.md"))).toBe(true)
        expect(paths.has(path.join(tmp.path, "sub", "CLAUDE.md"))).toBe(true)
      },
    })
  })

  test("GIZZI.md wins over CLAUDE.md at the same directory level", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "GIZZI.md"), "# GIZZI wins")
        await Bun.write(path.join(dir, "CLAUDE.md"), "# CLAUDE loses")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(tmp.path, "GIZZI.md"))).toBe(true)
        expect(paths.has(path.join(tmp.path, "CLAUDE.md"))).toBe(false)
      },
    })
  })

  test("AGENTS.md is honored when GIZZI.md/CLAUDE.md are absent", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "AGENTS.md"), "# AGENTS.md")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(tmp.path, "AGENTS.md"))).toBe(true)
      },
    })
  })
})

describe("InstructionPrompt.resolve", () => {
  test("returns empty when AGENTS.md is at project root (already in systemPaths)", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "AGENTS.md"), "# Root Instructions")
        await Bun.write(path.join(dir, "src", "file.ts"), "const x = 1")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const system = await InstructionPrompt.systemPaths()
        expect(system.has(path.join(tmp.path, "AGENTS.md"))).toBe(true)

        const results = await InstructionPrompt.resolve([], path.join(tmp.path, "src", "file.ts"), "test-message-1")
        expect(results).toEqual([])
      },
    })
  })

  test("returns AGENTS.md from subdirectory (not in systemPaths)", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "subdir", "AGENTS.md"), "# Subdir Instructions")
        await Bun.write(path.join(dir, "subdir", "nested", "file.ts"), "const x = 1")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const system = await InstructionPrompt.systemPaths()
        expect(system.has(path.join(tmp.path, "subdir", "AGENTS.md"))).toBe(false)

        const results = await InstructionPrompt.resolve(
          [],
          path.join(tmp.path, "subdir", "nested", "file.ts"),
          "test-message-2",
        )
        expect(results.length).toBe(1)
        expect(results[0].filepath).toBe(path.join(tmp.path, "subdir", "AGENTS.md"))
      },
    })
  })

  test("doesn't reload AGENTS.md when reading it directly", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "subdir", "AGENTS.md"), "# Subdir Instructions")
        await Bun.write(path.join(dir, "subdir", "nested", "file.ts"), "const x = 1")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const filepath = path.join(tmp.path, "subdir", "AGENTS.md")
        const system = await InstructionPrompt.systemPaths()
        expect(system.has(filepath)).toBe(false)

        const results = await InstructionPrompt.resolve([], filepath, "test-message-2")
        expect(results).toEqual([])
      },
    })
  })
})

describe("InstructionPrompt.systemPaths GIZZI_CONFIG_DIR", () => {
  let originalConfigDir: string | undefined

  beforeEach(() => {
    originalConfigDir = process.env["GIZZI_CONFIG_DIR"]
  })

  afterEach(() => {
    if (originalConfigDir === undefined) {
      delete process.env["GIZZI_CONFIG_DIR"]
    } else {
      process.env["GIZZI_CONFIG_DIR"] = originalConfigDir
    }
  })

  test("prefers GIZZI_CONFIG_DIR AGENTS.md over global when both exist", async () => {
    await using profileTmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "AGENTS.md"), "# Profile Instructions")
      },
    })
    await using globalTmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "AGENTS.md"), "# Global Instructions")
      },
    })
    await using projectTmp = await tmpdir()

    process.env["GIZZI_CONFIG_DIR"] = profileTmp.path
    const originalGlobalConfig = Global.Path.config
    ;(Global.Path as { config: string }).config = globalTmp.path

    try {
      await Instance.provide({
        directory: projectTmp.path,
        fn: async () => {
          const paths = await InstructionPrompt.systemPaths()
          expect(paths.has(path.join(profileTmp.path, "AGENTS.md"))).toBe(true)
          expect(paths.has(path.join(globalTmp.path, "AGENTS.md"))).toBe(false)
        },
      })
    } finally {
      ;(Global.Path as { config: string }).config = originalGlobalConfig
    }
  })

  test("falls back to global AGENTS.md when GIZZI_CONFIG_DIR has no AGENTS.md", async () => {
    await using profileTmp = await tmpdir()
    await using globalTmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "AGENTS.md"), "# Global Instructions")
      },
    })
    await using projectTmp = await tmpdir()

    process.env["GIZZI_CONFIG_DIR"] = profileTmp.path
    const originalGlobalConfig = Global.Path.config
    ;(Global.Path as { config: string }).config = globalTmp.path

    try {
      await Instance.provide({
        directory: projectTmp.path,
        fn: async () => {
          const paths = await InstructionPrompt.systemPaths()
          expect(paths.has(path.join(profileTmp.path, "AGENTS.md"))).toBe(false)
          expect(paths.has(path.join(globalTmp.path, "AGENTS.md"))).toBe(true)
        },
      })
    } finally {
      ;(Global.Path as { config: string }).config = originalGlobalConfig
    }
  })

  test("uses global AGENTS.md when GIZZI_CONFIG_DIR is not set", async () => {
    await using globalTmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "AGENTS.md"), "# Global Instructions")
      },
    })
    await using projectTmp = await tmpdir()
    // Other test files write instruction files into the shared test home's
    // ~/.gizzi, which would win over the global config dir.
    await using homeTmp = await tmpdir()
    const originalHome = process.env["GIZZI_TEST_HOME"]
    process.env["GIZZI_TEST_HOME"] = homeTmp.path

    delete process.env["GIZZI_CONFIG_DIR"]
    const originalGlobalConfig = Global.Path.config
    ;(Global.Path as { config: string }).config = globalTmp.path

    try {
      await Instance.provide({
        directory: projectTmp.path,
        fn: async () => {
          const paths = await InstructionPrompt.systemPaths()
          expect(paths.has(path.join(globalTmp.path, "AGENTS.md"))).toBe(true)
        },
      })
    } finally {
      process.env["GIZZI_TEST_HOME"] = originalHome
      ;(Global.Path as { config: string }).config = originalGlobalConfig
    }
  })
})

describe("InstructionPrompt.systemPaths local instruction files", () => {
  test("loads GIZZI.local.md alongside the root marker", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "GIZZI.md"), "# Root")
        await Bun.write(path.join(dir, "GIZZI.local.md"), "# Local")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(tmp.path, "GIZZI.md"))).toBe(true)
        expect(paths.has(path.join(tmp.path, "GIZZI.local.md"))).toBe(true)
      },
    })
  })

  test("GIZZI.local.md wins over CLAUDE.local.md at the same level", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "GIZZI.local.md"), "# GIZZI local")
        await Bun.write(path.join(dir, "CLAUDE.local.md"), "# CLAUDE local")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(tmp.path, "GIZZI.local.md"))).toBe(true)
        expect(paths.has(path.join(tmp.path, "CLAUDE.local.md"))).toBe(false)
      },
    })
  })

  test("CLAUDE.local.md is honored when GIZZI.local.md is absent", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "CLAUDE.local.md"), "# CLAUDE local")
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(tmp.path, "CLAUDE.local.md"))).toBe(true)
      },
    })
  })

  test("local files load from parent directories up to the worktree root", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "GIZZI.local.md"), "# Root local")
        await Bun.write(path.join(dir, "sub", "file.ts"), "const x = 1")
      },
    })
    await Instance.provide({
      directory: path.join(tmp.path, "sub"),
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(tmp.path, "GIZZI.local.md"))).toBe(true)
      },
    })
  })
})

describe("InstructionPrompt.systemPaths user-global GIZZI.md", () => {
  let originalConfigDir: string | undefined

  beforeEach(() => {
    originalConfigDir = process.env["GIZZI_CONFIG_DIR"]
  })

  afterEach(() => {
    if (originalConfigDir === undefined) {
      delete process.env["GIZZI_CONFIG_DIR"]
    } else {
      process.env["GIZZI_CONFIG_DIR"] = originalConfigDir
    }
  })

  test("loads GIZZI.md from GIZZI_CONFIG_DIR (TUI user-memory parity)", async () => {
    await using profileTmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "GIZZI.md"), "# User memory")
      },
    })
    await using projectTmp = await tmpdir()

    process.env["GIZZI_CONFIG_DIR"] = profileTmp.path
    await Instance.provide({
      directory: projectTmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(profileTmp.path, "GIZZI.md"))).toBe(true)
      },
    })
  })

  test("GIZZI.md wins over CLAUDE.md in the same config dir", async () => {
    await using profileTmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "GIZZI.md"), "# GIZZI wins")
        await Bun.write(path.join(dir, "CLAUDE.md"), "# CLAUDE loses")
      },
    })
    await using projectTmp = await tmpdir()

    process.env["GIZZI_CONFIG_DIR"] = profileTmp.path
    await Instance.provide({
      directory: projectTmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(profileTmp.path, "GIZZI.md"))).toBe(true)
        expect(paths.has(path.join(profileTmp.path, "CLAUDE.md"))).toBe(false)
      },
    })
  })

  test("falls back to CLAUDE.md when no GIZZI.md exists", async () => {
    await using profileTmp = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "CLAUDE.md"), "# Legacy user memory")
      },
    })
    await using projectTmp = await tmpdir()

    process.env["GIZZI_CONFIG_DIR"] = profileTmp.path
    await Instance.provide({
      directory: projectTmp.path,
      fn: async () => {
        const paths = await InstructionPrompt.systemPaths()
        expect(paths.has(path.join(profileTmp.path, "CLAUDE.md"))).toBe(true)
      },
    })
  })
})
