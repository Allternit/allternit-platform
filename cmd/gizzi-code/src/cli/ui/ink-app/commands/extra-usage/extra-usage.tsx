import React from 'react';
import type { LocalJSXCommandContext } from '../../commands';
import type { LocalJSXCommandOnDone } from '../../types/command';
import { Login } from '../login/login';
import { StatusPanel } from '../status/StatusPanel';
import { runExtraUsage } from './extra-usage-core';

/**
 * Hidden alias of /status (overhaul plan D4): runs the extra-usage request as
 * before, then shows its result on the /status Usage tab. A login still opens
 * the login flow.
 */
export async function call(onDone: LocalJSXCommandOnDone, context: LocalJSXCommandContext): Promise<React.ReactNode | null> {
  const result = await runExtraUsage();
  if (result.type === 'message') {
    return <StatusPanel onClose={onDone} context={context} defaultTab="Usage" usageNotice={result.value} />;
  }
  return <Login startingMessage={'Starting new login following /extra-usage. Exit with Ctrl-C to use existing account.'} onDone={success => {
    context.onChangeAPIKey();
    onDone(success ? 'Login successful' : 'Login interrupted');
  }} />;
}
