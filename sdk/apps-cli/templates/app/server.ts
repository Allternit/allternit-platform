import { appTool, defineApp, view, z } from "@allternit/apps-server";

const TEMPS: Record<string, number> = { london: 14, tokyo: 22, austin: 31 };

// The View: a detail card from the view kit. It reads the look pack's CSS
// variables, so changing the look pack below restyles it.
const card = view({
  uri: "ui://__APP_NAME__/card.html",
  name: "Weather card",
  kit: { tag: "allternit-detail", fields: { title: "city", labels: { temperatureC: "Temperature (°C)" } } },
});

export default defineApp({
  name: "__APP_TITLE__",
  version: "0.1.0",
  description: "Look up the current temperature for a city.",
  instructions:
    "Look up the current temperature for a city. Use get_temperature to read data; " +
    "use show_weather only when the user wants a weather card displayed.",
  tools: [
    // A data tool: no UI, safe to call repeatedly.
    appTool({
      name: "get_temperature",
      title: "Get temperature",
      description: "Get the current temperature in Celsius for london, tokyo or austin.",
      input: { city: z.string().describe("City name, e.g. london") },
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: false },
      handler: async ({ city }) => {
        const t = TEMPS[city.toLowerCase()];
        if (t === undefined) return { isError: true, content: [{ type: "text", text: `Unknown city: ${city}` }] };
        return { content: [{ type: "text", text: `${city}: ${t} °C` }] };
      },
    }),
    // A render tool: its result is delivered to the View.
    appTool({
      name: "show_weather",
      title: "Show weather card",
      description: "Display a weather card for a city. Call only when the user wants to see it.",
      input: { city: z.string() },
      annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: false },
      view: card,
      handler: async ({ city }) => {
        const t = TEMPS[city.toLowerCase()];
        if (t === undefined) return { isError: true, content: [{ type: "text", text: `Unknown city: ${city}` }] };
        return {
          content: [{ type: "text", text: `Showing weather for ${city}: ${t} °C` }],
          structuredContent: { city, temperatureC: t },
        };
      },
    }),
  ],
  views: [card],
  lookPack: {
    id: "__APP_NAME__",
    name: "__APP_TITLE__",
    tokens: {
      color: {
        $type: "color",
        surface: { $value: "#ffffff" },
        text: { $value: "#1f1e1d" },
        brand: { $value: "#1f1e1d" },
        "on-brand": { $value: "#ffffff" },
      },
      radius: { card: { $value: "12px" } },
    },
  },
});
