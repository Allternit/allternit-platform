import * as React from 'react';
import type { LocalJSXCommandCall } from '../../types/command';
import { StatusPanel } from './StatusPanel';
import type { StatusTab } from './tabs';

/** The call for a hidden alias that opens one /status tab. */
export function openStatusTab(tab: StatusTab): LocalJSXCommandCall {
  return async (onDone, context) => <StatusPanel onClose={onDone} context={context} defaultTab={tab} />;
}

export const statusTabCall = openStatusTab('Status');
export const usageTabCall = openStatusTab('Usage');
export const costTabCall = openStatusTab('Cost');
export const statsTabCall = openStatusTab('Stats');
