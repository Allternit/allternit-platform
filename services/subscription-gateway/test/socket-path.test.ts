import { describe, expect, it } from "vitest";
import { gatewaySocketPathFor } from "../src/config.js";

describe("gatewaySocketPathFor", () => {
  it("uses gateway.sock in the state dir off Windows", () => {
    expect(gatewaySocketPathFor("/tmp/subs", "darwin")).toBe("/tmp/subs/gateway.sock");
  });

  it("uses a named pipe on Windows, the same for a trailing slash", () => {
    const pipe = gatewaySocketPathFor("/tmp/subs/", "win32");
    expect(pipe).toMatch(/^\\\\\.\\pipe\\allternit-subs-gateway-[0-9a-f]{16}$/);
    expect(gatewaySocketPathFor("/tmp/subs", "win32")).toBe(pipe);
    expect(gatewaySocketPathFor("/tmp/other", "win32")).not.toBe(pipe);
  });
});
