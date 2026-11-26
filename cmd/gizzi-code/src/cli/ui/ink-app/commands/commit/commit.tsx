// @ts-nocheck
import * as React from 'react';
import { useState, useEffect } from 'react';
import { execSync } from 'child_process';
import { Box, Text } from '../../ink.js';
import { Dialog } from '../../components/design-system/Dialog.js';
import { SelectMulti } from '../../components/CustomSelect/SelectMulti.js';
import { getAttributionTexts } from '../../utils/attribution.js';
import type { LocalJSXCommandCall } from '../../types/command.js';
import { EmptyLine } from '../../components/EmptyLine'

type Props = {
  onDone(): void;
};

export function CommitDialog({ onDone }: Props) {
  const [files, setFiles] = useState<string[]>([]);
  const [staged, setStaged] = useState<string[]>([]);
  const [step, setStep] = useState<'stage' | 'done'>('stage');
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    try {
      const output = execSync('git status --porcelain', { encoding: 'utf8' });
      const parsed = output
        .split('\n')
        .map(line => line.trim())
        .filter(line => line.length > 0)
        .map(line => {
          // git status --porcelain output prefix is 2 characters plus a space
          return line.slice(3).trim();
        });
      setFiles(parsed);
      setStaged(parsed); // Default to staging all modified files
    } catch (e) {
      setErrorMsg('Failed to query git status');
    }
  }, []);

  const handleStageSubmit = (selected: string[]) => {
    if (selected.length === 0) {
      setErrorMsg('No files selected for staging.');
      return;
    }
    try {
      // Stage the selected files
      execSync('git reset', { encoding: 'utf8' }); // Unstage all first
      for (const file of selected) {
        execSync(`git add "${file}"`, { encoding: 'utf8' });
      }

      // Draft commit message (LLM or interactive)
      const { commit: commitAttribution } = getAttributionTexts();
      const attrib = commitAttribution ? `\n\n${commitAttribution}` : '';
      const msg = `refactor: update files\n\nStaged changes for:${selected.map(f => `\n- ${f}`).join('')}${attrib}`;
      
      execSync(`git commit -m "${msg}"`, { encoding: 'utf8' });
      setStep('done');
      onDone();
    } catch (e: any) {
      setErrorMsg(e.message || 'Failed to create commit');
    }
  };

  if (errorMsg) {
    return (
      <Dialog title="Git Commit Error" color="error" onCancel={onDone}>
        <Text color="red">{errorMsg}</Text>
      </Dialog>
    );
  }

  if (files.length === 0) {
    return (
      <Dialog title="Git Commit" color="info" onCancel={onDone}>
        <EmptyLine>No modified files to commit.</EmptyLine>
      </Dialog>
    );
  }

  return (
    <Dialog title="Interactive Git Commit" subtitle="Select files to stage and commit (Space to toggle, Enter to confirm)" color="info" onCancel={onDone}>
      <SelectMulti
        options={files.map(f => ({ label: f, value: f }))}
        defaultValue={staged}
        onSubmit={handleStageSubmit}
        onCancel={onDone}
        hideIndexes={true}
      />
    </Dialog>
  );
}

export const call: LocalJSXCommandCall = async (onDone) => {
  return <CommitDialog onDone={onDone} />;
};
