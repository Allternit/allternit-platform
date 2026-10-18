import { describe, expect, test } from "bun:test"
import { paneEndpointFor } from "../../src/runtime/integrations/pty/endpoint"
const Pty = { paneEndpoint: paneEndpointFor }

describe("Pty.paneEndpoint", () => {
  test("Unix: the socket path as is", () => {
    expect(Pty.paneEndpoint("/Users/me/.config/herdr/ao/herdr.sock", "darwin")).toBe(
      "/Users/me/.config/herdr/ao/herdr.sock",
    )
  })

  test("Windows: the named pipe the pane engine serves for its marker path", () => {
    expect(Pty.paneEndpoint("C:\\Users\\me\\herdr\\ao\\herdr.sock", "win32")).toBe(
      "\\\\.\\pipe\\C:\\Users\\me\\herdr\\ao\\herdr.sock",
    )
    expect(Pty.paneEndpoint("\\\\.\\pipe\\x", "win32")).toBe("\\\\.\\pipe\\x")
  })
})
