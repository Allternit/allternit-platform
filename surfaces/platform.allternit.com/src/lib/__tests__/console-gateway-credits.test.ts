import { describe, expect, it } from "vitest";
import { describeGatewayError } from "../console-gateway";

describe("describeGatewayError credits", () => {
  it("shows the contract message for credits_required", () => {
    const m = describeGatewayError(402, "x", "credits_required");
    expect(m).toContain("Allternit Cloud models need credits.");
    expect(m).toContain("https://ai.allternit.com/settings/billing");
  });
  it("shows the contract message for credits_exhausted", () => {
    const m = describeGatewayError(402, "x", "credits_exhausted");
    expect(m).toContain("Your Allternit Cloud credits are used up.");
  });
  it("keeps the project-billing wording for other 402s", () => {
    expect(describeGatewayError(402)).toContain("out of credits");
  });
});
