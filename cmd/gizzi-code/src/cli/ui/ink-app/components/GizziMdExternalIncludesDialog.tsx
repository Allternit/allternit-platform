import React, { useCallback } from 'react';
import { logEvent } from './../services/analytics/index.ts';
import { Box, Link, Text } from '../ink';
import type { ExternalClaudeMdInclude } from '../utils/gizzimd';
import { saveCurrentProjectConfig } from '../utils/config';
import { Select } from './CustomSelect/index';
import { Dialog } from './design-system/Dialog';
type Props = {
  onDone(): void;
  isStandaloneDialog?: boolean;
  externalIncludes?: ExternalClaudeMdInclude[];
};
export function GizziMdExternalIncludesDialog(t0) {
  const {
    onDone,
    isStandaloneDialog,
    externalIncludes
  } = t0;
  const t1 = [];

  React.useEffect(_temp, t1);
  const t2 = value => {
      if (value === "no") {
        logEvent("tengu_claude_md_external_includes_dialog_declined", {});
        saveCurrentProjectConfig(_temp2);
      } else {
        logEvent("tengu_claude_md_external_includes_dialog_accepted", {});
        saveCurrentProjectConfig(_temp3);
      }
      onDone();
    };

  const handleSelection = t2;
  const t3 = () => {
      handleSelection("no");
    };

  const handleEscape = t3;
  const t4 = !isStandaloneDialog;
  const t5 = !isStandaloneDialog;
  const t6 = <Text>This project's GIZZI.md imports files outside the current working directory. Never allow this for third-party repositories.</Text>;

  const t7 = externalIncludes && externalIncludes.length > 0 && <Box flexDirection="column"><Text dimColor={true}>External imports:</Text>{externalIncludes.map(_temp4)}</Box>;

  const t8 = <Text dimColor={true}>Important: Only use Gizzi Code with files you trust. Accessing untrusted files may pose security risks{" "}<Link url="https://docs.allternit.com/security/overview" />{" "}</Text>;

  const t9 = [{
      label: "Yes, allow external imports",
      value: "yes"
    }, {
      label: "No, disable external imports",
      value: "no"
    }];

  const t10 = <Select options={t9} onChange={value_0 => handleSelection(value_0 as 'yes' | 'no')} />;

  const t11 = <Dialog title="Allow external GIZZI.md file imports?" color="warning" onCancel={handleEscape} hideBorder={t4} hideInputGuide={t5}>{t6}{t7}{t8}{t10}</Dialog>;

  return t11;
}
function _temp4(include, i) {
  return <Text key={i} dimColor={true}>{"  "}{include.path}</Text>;
}
function _temp3(current_0) {
  return {
    ...current_0,
    hasClaudeMdExternalIncludesApproved: true,
    hasClaudeMdExternalIncludesWarningShown: true
  };
}
function _temp2(current) {
  return {
    ...current,
    hasClaudeMdExternalIncludesApproved: false,
    hasClaudeMdExternalIncludesWarningShown: true
  };
}
function _temp() {
  logEvent("tengu_claude_md_includes_dialog_shown", {});
}
