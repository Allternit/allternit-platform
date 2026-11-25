import React, { useState } from 'react';
import { Box, Text, useInput } from '../../../ink';
import { openBrowser } from '../../../utils/browser';
import { execFileNoThrow } from '../../../utils/execFileNoThrow';

const WEB_BASE = 'https://ai.allternit.com';

type Plan = {
  steps: { title: string; detail?: string }[];
  title?: string;
  artifact?: { id: string; persisted: boolean };
};

/** Opens the saved plan in the Allternit app (allternit://artifact/<id>), else in the browser. */
async function openPlan(id: string): Promise<'app' | 'web' | 'failed'> {
  const link = `allternit://artifact/${encodeURIComponent(id)}`;
  const cmd = process.platform === 'darwin' ? ['open', [link]] : process.platform === 'win32' ? ['cmd', ['/c', 'start', '', link]] : ['xdg-open', [link]];
  const { code } = await execFileNoThrow(cmd[0] as string, cmd[1] as string[]);
  if (code === 0) return 'app';
  return (await openBrowser(`${WEB_BASE}/?artifact=${encodeURIComponent(id)}`)) ? 'web' : 'failed';
}

/** The plan awaiting approval: title and steps, plus "Open in Allternit" when it is saved to the account. */
export function PlanSummary({ plan, keysActive }: { plan: Plan; keysActive: boolean }) {
  const [note, setNote] = useState<string | null>(null);
  const artifactId = plan.artifact?.persisted ? plan.artifact.id : undefined;
  useInput((input) => {
    if (!artifactId || input !== 'o') return;
    openPlan(artifactId).then(r => setNote(r === 'failed' ? 'Could not open the plan' : r === 'app' ? 'Opened in Allternit' : 'Opened in your browser'));
  }, { isActive: keysActive && !!artifactId });
  return (
    <Box flexDirection="column" marginBottom={1}>
      {plan.title ? <Text bold>{plan.title}</Text> : null}
      {plan.steps.map((step, i) => (
        <Box key={i} flexDirection="column">
          <Text>{`${i + 1}. ${step.title}`}</Text>
          {step.detail ? <Text dimColor>{`   ${step.detail}`}</Text> : null}
        </Box>
      ))}
      {artifactId ? <Text dimColor>{note ?? 'Press o to open in Allternit'}</Text> : null}
    </Box>
  );
}
