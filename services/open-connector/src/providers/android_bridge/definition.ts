import type { ProviderDefinition } from "../../core/types.ts";

import { androidBridgeActions } from "./actions.ts";

const service = "android_bridge";

export const provider: ProviderDefinition = {
  service,
  displayName: "Android Bridge",
  categories: ["Communication", "Device Control"],
  authTypes: ["custom_credential"],
  auth: [
    {
      type: "custom_credential",
      fields: [
        {
          key: "baseUrl",
          label: "Bridge base URL",
          inputType: "text",
          required: true,
          secret: false,
          placeholder: "http://127.0.0.1:8020",
          description: "Base URL of the Android Bridge HTTP service.",
        },
        {
          key: "deviceId",
          label: "Device ID",
          inputType: "text",
          required: false,
          secret: false,
          placeholder: "serial-or-alias",
          description: "Optional device identifier when multiple phones are bridged.",
        },
      ],
    },
  ],
  homepageUrl: "https://developer.android.com/studio/command-line/adb",
  actions: androidBridgeActions,
};
