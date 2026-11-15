/**
 * The model and effort the user picks reach the CLI (bug: agy, grok and kimi
 * ran their own default model whatever was picked, and a retired id such as
 * gemini-3.7-flash-high "worked" because it never reached agy).
 */
import { describe, expect, test } from "bun:test"
import { acpModelConfig, cliAdapterArgv } from "@/runtime/drivers/local-cli-driver"

describe("antigravity (agy --print)", () => {
  test("passes the picked model and effort before --print", () => {
    expect(
      cliAdapterArgv("antigravity", ["agy"], "hi", { taskId: "t", model: "gemini-3.8-flash-low", effort: "high" }),
    ).toEqual(["agy", "--model", "gemini-3.8-flash-low", "--effort", "high", "--print", "hi"])
  })

  test("the placeholder model leaves agy on its own default", () => {
    expect(cliAdapterArgv("antigravity", ["agy"], "hi", { taskId: "t", model: "default" })).toEqual([
      "agy",
      "--print",
      "hi",
    ])
  })

  test("a retired id is sent as its replacement", () => {
    expect(cliAdapterArgv("antigravity", ["agy"], "hi", { taskId: "t", model: "gemini-3.7-flash-high" })).toEqual([
      "agy",
      "--model",
      "gemini-3.8-flash-high",
      "--print",
      "hi",
    ])
  })

  test("an effort agy does not take is left out", () => {
    expect(cliAdapterArgv("antigravity", ["agy"], "hi", { taskId: "t", effort: "ultra" })).toEqual([
      "agy",
      "--print",
      "hi",
    ])
  })
})

describe("claude-cli", () => {
  test("a retired id is sent as its replacement", () => {
    const argv = cliAdapterArgv("claude-cli", ["claude"], "hi", { taskId: "t", model: "claude-fable-5-1" })
    expect(argv[argv.indexOf("--model") + 1]).toBe("claude-fable-5-2")
  })
})

describe("grok (grok agent … stdio)", () => {
  test("model and effort are agent options, before the stdio subcommand", () => {
    expect(cliAdapterArgv("grok", ["grok"], "hi", { taskId: "t", model: "grok-4.5", effort: "high" })).toEqual([
      "grok",
      "agent",
      "-m",
      "grok-4.5",
      "--reasoning-effort",
      "high",
      "--no-leader",
      "--always-approve",
      "stdio",
    ])
  })

  test("the placeholder model leaves grok on its own default", () => {
    expect(cliAdapterArgv("grok", ["grok"], "hi", { taskId: "t", model: "default" })).toEqual([
      "grok",
      "agent",
      "--no-leader",
      "--always-approve",
      "stdio",
    ])
  })
})

describe("kimi-cli (ACP session/set_config_option)", () => {
  // The shape kimi acp returns from session/new.
  const configOptions = [
    {
      id: "model",
      type: "select",
      currentValue: "kimi-code/kimi-for-coding",
      options: [
        { value: "kimi-code/kimi-for-coding", name: "K2.7 Code" },
        { value: "kimi-code/kimi-k3", name: "K3" },
      ],
    },
    { id: "thinking", type: "select", currentValue: "on", options: [{ value: "off", name: "Off" }, { value: "on", name: "On" }] },
  ]

  test("argv stays `kimi acp`: the acp command ignores the root -m", () => {
    expect(cliAdapterArgv("kimi-cli", ["kimi"], "hi", { taskId: "t", model: "kimi-k3" })).toEqual(["kimi", "acp"])
  })

  test("a catalog id selects kimi's provider-prefixed alias", () => {
    expect(acpModelConfig("kimi-cli", configOptions, "kimi-k3", undefined)).toEqual([
      { configId: "model", value: "kimi-code/kimi-k3" },
    ])
  })

  test("the model already current sends nothing", () => {
    expect(acpModelConfig("kimi-cli", configOptions, "kimi-for-coding", undefined)).toEqual([])
  })

  test("a model kimi does not offer fails the turn instead of running the default", () => {
    expect(() => acpModelConfig("kimi-cli", configOptions, "kimi-k9", undefined)).toThrow(
      'kimi-cli has no model "kimi-k9". It offers: kimi-code/kimi-for-coding, kimi-code/kimi-k3.',
    )
  })

  test("a thinking level the session lists is set; one it does not list is skipped", () => {
    expect(acpModelConfig("kimi-cli", configOptions, undefined, "off")).toEqual([{ configId: "thinking", value: "off" }])
    expect(acpModelConfig("kimi-cli", configOptions, undefined, "xhigh")).toEqual([])
  })
})
