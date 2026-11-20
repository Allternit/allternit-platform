import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { CreatedNotice, RedirectEditor, StateActions } from "../parts";
import { validateRedirectUri } from "@/lib/oauth-apps";

const noop = () => {};

describe("oauth app console", () => {
  it("create_shows_secret_once", () => {
    const html = renderToStaticMarkup(
      <CreatedNotice app={{ name: "Notes", client_id: "oapp_1", client_type: "confidential" }} secret="alt_ocs_abc123" onClose={noop} />,
    );
    expect(html).toContain("alt_ocs_abc123");
    expect(html).toContain("Copy it now. You won&#x27;t see it again.");
    // Once the parent clears the secret (box closed) it is gone from the page.
    const after = renderToStaticMarkup(
      <CreatedNotice app={{ name: "Notes", client_id: "oapp_1", client_type: "confidential" }} secret={null} onClose={noop} />,
    );
    expect(after).not.toContain("alt_ocs_abc123");
    expect(after).not.toContain("Copy it now");
  });

  it("redirect_editor_rejects_custom_scheme", () => {
    const html = renderToStaticMarkup(
      <RedirectEditor uris={[]} onAdd={noop} onRemove={noop} initialDraft="myapp://callback" />,
    );
    expect(html).toContain("Custom schemes are not allowed");
    expect(validateRedirectUri("myapp://callback")).toMatch(/Custom schemes/);
    expect(validateRedirectUri("https://a.com/cb#x")).toMatch(/fragment/);
    expect(validateRedirectUri("https://a.com/*")).toMatch(/Wildcards/);
    expect(validateRedirectUri("http://example.com/cb")).toMatch(/localhost/);
    expect(validateRedirectUri("http://[::1]:3000/cb")).toBeNull();
    expect(validateRedirectUri("https://a.com/cb")).toBeNull();
  });

  it("public_app_has_no_secret_box", () => {
    const html = renderToStaticMarkup(
      <CreatedNotice app={{ name: "Spa", client_id: "oapp_2", client_type: "public" }} secret={null} onClose={noop} />,
    );
    expect(html).not.toContain("Client secret");
    expect(html).not.toContain("Copy it now");
    expect(html).toContain("Public apps have no client secret");
  });

  it("submit_button_only_in_testing", () => {
    const render = (state: "testing" | "in_review" | "live" | "suspended") =>
      renderToStaticMarkup(
        <StateActions state={state} onSubmit={noop} onWithdraw={noop} onRotate={noop} onDelete={noop} />,
      );
    expect(render("testing")).toContain("Submit for review");
    for (const s of ["in_review", "live", "suspended"] as const) expect(render(s)).not.toContain("Submit for review");
    expect(render("in_review")).toContain("Withdraw");
    expect(render("testing")).not.toContain("Withdraw");
  });
});
