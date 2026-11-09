import React from 'react';
import { Box, Link, Text } from '../ink';
import { Select } from './CustomSelect/index';
import { Dialog } from './design-system/Dialog';
type Props = {
  onDone: () => void;
};
export function CostThresholdDialog({
    onDone
}: Props) {
  const t1 = <Box flexDirection="column"><Text>Learn more about how to monitor your spending:</Text><Link url="https://docs.allternit.com/cli/usage" /></Box>;

  const t2 = [{
      value: "ok",
      label: "Got it, thanks!"
    }];

  const t3 = <Select options={t2} onChange={onDone} />;

  const t4 = <Dialog title="You've spent $5 on the Anthropic API this session." onCancel={onDone}>{t1}{t3}</Dialog>;

  return t4;
}
