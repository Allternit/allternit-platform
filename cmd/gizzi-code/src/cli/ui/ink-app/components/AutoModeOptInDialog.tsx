import React from 'react';
import { logEvent } from './../services/analytics/index.ts';
import { Box, Link, Text } from '../ink';
import { updateSettingsForSource } from '../utils/settings/settings';
import { Select } from './CustomSelect/index';
import { Dialog } from './design-system/Dialog';

// NOTE: This copy is legally reviewed — do not modify without Legal team approval.
export const AUTO_MODE_DESCRIPTION = "Auto mode lets Gizzi handle permission prompts automatically — Gizzi checks each tool call for risky actions and prompt injection before executing. Actions Gizzi identifies as safe are executed, while actions Gizzi identifies as risky are blocked and Gizzi may try a different approach. Ideal for long-running tasks. Sessions are slightly more expensive. Gizzi can make mistakes that allow harmful commands to run, it's recommended to only use in isolated environments. Shift+Tab to change mode.";
type Props = {
  onAccept(): void;
  onDecline(): void;
  // Startup gate: decline exits the process, so relabel accordingly.
  declineExits?: boolean;
};
export function AutoModeOptInDialog(t0) {
  const {
    onAccept,
    onDecline,
    declineExits
  } = t0;
  const t1 = [];

  React.useEffect(_temp, t1);
  const t2 = function onChange(value) {
      bb3: switch (value) {
        case "accept":
          {
            logEvent("tengu_auto_mode_opt_in_dialog_accept", {});
            updateSettingsForSource("userSettings", {
              skipAutoPermissionPrompt: true
            });
            onAccept();
            break bb3;
          }
        case "accept-default":
          {
            logEvent("tengu_auto_mode_opt_in_dialog_accept_default", {});
            updateSettingsForSource("userSettings", {
              skipAutoPermissionPrompt: true,
              permissions: {
                defaultMode: "auto"
              }
            });
            onAccept();
            break bb3;
          }
        case "decline":
          {
            logEvent("tengu_auto_mode_opt_in_dialog_decline", {});
            onDecline();
          }
      }
    };

  const onChange = t2;
  const t3 = <Box flexDirection="column" gap={1}><Text>{AUTO_MODE_DESCRIPTION}</Text><Link url="https://docs.allternit.com/security/overview" /></Box>;

  const t4 = true ? [{
      label: "Yes, and make it my default mode",
      value: "accept-default" as const
    }] : [];

  const t5 = {
      label: "Yes, enable auto mode",
      value: "accept" as const
    };

  const t6 = declineExits ? "No, exit" : "No, go back";
  const t7 = [...t4, t5, {
      label: t6,
      value: "decline" as const
    }];

  const t8 = value_0 => onChange(value_0 as 'accept' | 'accept-default' | 'decline');

  const t9 = <Select options={t7} onChange={t8} onCancel={onDecline} />;

  const t10 = <Dialog title="Enable auto mode?" color="warning" onCancel={onDecline}>{t3}{t9}</Dialog>;

  return t10;
}
function _temp() {
  logEvent("tengu_auto_mode_opt_in_dialog_shown", {});
}
