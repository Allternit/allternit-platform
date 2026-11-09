import { afterEach, describe, expect, test } from "bun:test"
import { cloudOrigin } from "../../src/runtime/providers/discovery/allternit-cloud"

const keys = ["ALLTERNIT_CLOUD_URL", "ALLTERNIT_API_URL", "ALLTERNIT_API_BASE_URL"] as const
const saved = Object.fromEntries(keys.map((k) => [k, process.env[k]]))
afterEach(() => {
  for (const k of keys) {
    if (saved[k] === undefined) delete process.env[k]
    else process.env[k] = saved[k]
  }
})

describe("cloudOrigin", () => {
  test("Desktop's local API URL is never used as the Cloud", () => {
    for (const k of keys) delete process.env[k]
    process.env.ALLTERNIT_API_URL = "http://127.0.0.1:8013"
    expect(cloudOrigin()).toBe("https://api.allternit.com")
    process.env.ALLTERNIT_API_URL = "http://localhost:3001/"
    expect(cloudOrigin()).toBe("https://api.allternit.com")
  })
  test("a remote override and ALLTERNIT_CLOUD_URL still apply", () => {
    for (const k of keys) delete process.env[k]
    process.env.ALLTERNIT_API_URL = "https://staging.allternit.com/"
    expect(cloudOrigin()).toBe("https://staging.allternit.com")
    process.env.ALLTERNIT_CLOUD_URL = "https://cloud.example.com"
    expect(cloudOrigin()).toBe("https://cloud.example.com")
  })
  test("an explicit ALLTERNIT_CLOUD_URL is honored even on loopback (local cloud-api, fixtures)", () => {
    for (const k of keys) delete process.env[k]
    process.env.ALLTERNIT_API_URL = "http://127.0.0.1:8013"
    process.env.ALLTERNIT_CLOUD_URL = "http://127.0.0.1:4555/"
    expect(cloudOrigin()).toBe("http://127.0.0.1:4555")
  })
})
