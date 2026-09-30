// @vitest-environment happy-dom
import { beforeAll, describe, expect, it } from "vitest";
import { KIT_TAGS, mount } from "../src/index.js";
import { kitSource } from "../src/sources.js";

beforeAll(() => {
  new Function("window", kitSource()).call(window, window);
});

const shadow = (el: Element) => (el as HTMLElement).shadowRoot!;

describe("view kit", () => {
  it("registers every kit tag", () => {
    for (const tag of KIT_TAGS) expect(customElements.get(tag)).toBeDefined();
  });

  it("card renders data as text, never markup", () => {
    const card = mount("allternit-card", document.body, { name: "<img src=x onerror=alert(1)>", role: "admin" }, { title: "name" });
    const root = shadow(card);
    expect(root.querySelector("img")).toBeNull();
    expect(root.querySelector(".title")!.textContent).toBe("<img src=x onerror=alert(1)>");
    expect(root.textContent).toContain("admin");
  });

  it("links and images must be https", () => {
    const card = mount(
      "allternit-card",
      document.body,
      { name: "A", link: "javascript:alert(1)", pic: "http://x.test/a.png" },
      { title: "name", url: "link", image: "pic" },
    );
    expect(shadow(card).querySelector("a")).toBeNull();
    expect(shadow(card).querySelector("img")).toBeNull();
    const ok = mount("allternit-card", document.body, { name: "A", link: "https://ex.test/a" }, { title: "name", url: "link" });
    const a = shadow(ok).querySelector("a")!;
    expect(a.href).toBe("https://ex.test/a");
    expect(a.rel).toBe("noopener noreferrer");
  });

  it("table takes columns from the mapping or the first row", () => {
    const rows = [{ id: 1, name: "a", nested: { x: 1 } }, { id: 2, name: "b" }];
    const auto = mount("allternit-table", document.body, rows);
    expect([...shadow(auto).querySelectorAll("th")].map((t) => t.textContent)).toEqual(["Id", "Name"]);
    const mapped = mount("allternit-table", document.body, rows, { columns: ["name"], labels: { name: "Who" } });
    expect([...shadow(mapped).querySelectorAll("th")].map((t) => t.textContent)).toEqual(["Who"]);
    expect(shadow(mapped).querySelectorAll("tbody tr")).toHaveLength(2);
  });

  it("list and detail show empty states instead of throwing", () => {
    expect(shadow(mount("allternit-list", document.body, null)).textContent).toContain("Nothing here");
    expect(shadow(mount("allternit-detail", document.body, "nope")).textContent).toContain("No data");
    expect(shadow(mount("allternit-table", document.body, [])).textContent).toContain("No rows");
  });

  it("detail lists scalar fields", () => {
    const d = mount("allternit-detail", document.body, { title: "T", status: "open", deep: { a: 1 } }, { title: "title" });
    expect(shadow(d).querySelector("dd")!.textContent).toBe("open");
    expect(shadow(d).querySelectorAll("dt")).toHaveLength(1);
  });

  it("form emits allternit-submit with typed values", () => {
    const form = mount("allternit-form", document.body, {
      fields: [
        { name: "city", required: true, value: "Tokyo" },
        { name: "days", type: "number", value: 3 },
        { name: "units", options: ["c", "f"] },
      ],
      submitLabel: "Go",
    });
    let detail: any;
    form.addEventListener("allternit-submit", (e) => (detail = (e as CustomEvent).detail));
    const root = shadow(form);
    expect(root.querySelector("button")!.textContent).toBe("Go");
    root.querySelector("form")!.dispatchEvent(new Event("submit", { cancelable: true }));
    expect(detail.values).toEqual({ city: "Tokyo", days: 3, units: "c" });
  });

  it("reads look-pack variables with the Allternit default as fallback", () => {
    const src = kitSource();
    expect(src).toContain("var(--vp-color-surface,#fff)");
    expect(src).toContain("var(--vp-radius-card,10px)");
    expect(src).not.toMatch(/#f[0-9a-f]{5}\b.*tan|--bg-primary|--surface-panel/);
  });
});
