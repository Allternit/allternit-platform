import { describe, expect, it } from 'vitest';
import { pairingLookupFailureMessage } from './desktop-auth-url.js';
import {
  PairingFailure,
  pairingErrorView,
  pairingFailureKindForStatus,
  pairingStepAt,
} from './pairing-state.js';
import { renderPairingCard } from './startup-page.js';
import { connectionSettingsTarget, isWorkspaceLoadFailure } from './window-chrome.js';

const info = { computerName: 'debra', platformLabel: 'Linux' };

/** The icon each step row shows, in order: done (check), current (spinner), todo (dot). */
function stepIcons(html: string): string[] {
  return [...html.matchAll(/data-step="(done|current|todo)"/g)].map((m) => m[1]);
}

describe('pairing steps', () => {
  it('maps each phase to the step in progress', () => {
    expect(pairingStepAt({ phase: 'welcome' })).toBe(-1);
    expect(pairingStepAt({ phase: 'signing-in' })).toBe(0);
    expect(pairingStepAt({ phase: 'approving' })).toBe(1);
    expect(pairingStepAt({ phase: 'paired' })).toBe(3);
  });

  it('puts an error on the step that failed', () => {
    const signIn = pairingErrorView(new PairingFailure('auth', 'x', 401))!;
    expect(pairingStepAt({ phase: 'error', error: signIn })).toBe(0);
    const server = pairingErrorView(new PairingFailure('server', 'x', 500))!;
    expect(pairingStepAt({ phase: 'error', error: server })).toBe(1);
  });
});

describe('pairing error cards', () => {
  it('blames Allternit, not the person, for a server error and keeps the code', () => {
    const view = pairingErrorView(new PairingFailure('server', 'Pairing approve failed (500): DATABASE_ERROR', 500))!;
    expect(view.title).toBe("Couldn't approve this computer");
    expect(view.body).toBe('Allternit had a problem on its side (error 500). You did nothing wrong. Try again in a minute.');
    expect(view.primary).toBe('retry');
    expect(view.secondary).toBe('status');
  });

  it('offers Sign in again for a rejected sign-in', () => {
    const view = pairingErrorView(new PairingFailure(pairingFailureKindForStatus(401, 'UNAUTHORIZED'), 'x', 401))!;
    expect(view.primary).toBe('sign-in-again');
    expect(view.secondary).toBe('retry');
  });

  it('classifies cloud responses by status and server code', () => {
    expect(pairingFailureKindForStatus(404)).toBe('expired');
    expect(pairingFailureKindForStatus(401, 'TOKEN_EXPIRED')).toBe('expired');
    expect(pairingFailureKindForStatus(403)).toBe('auth');
    expect(pairingFailureKindForStatus(503)).toBe('server');
    expect(pairingFailureKindForStatus(409, 'You reached your plan machine limit')).toBe('limit');
    expect(pairingFailureKindForStatus(400)).toBe('unknown');
  });

  it('never shows raw IPC or HTTP text', () => {
    const raw = [
      new Error("Error invoking remote method 'connection:set-backend': Error: Untrusted IPC sender"),
      new PairingFailure('unknown', 'Pairing exchange failed (400): {"error":"bad_signature"}', 400),
      new Error(pairingLookupFailureMessage(500, 'DATABASE_ERROR')),
    ];
    for (const error of raw) {
      const view = pairingErrorView(error)!;
      const shown = `${view.title} ${view.body}`;
      expect(shown).not.toMatch(/IPC|remote method|bad_signature|DATABASE_ERROR|\{/);
    }
  });

  it('treats a closed sign-in window or Cancel as going back, not an error', () => {
    expect(pairingErrorView(new Error('Sign-in window was closed.'))).toBeNull();
  });

  it('reads fetch network failures and expiry from plain errors', () => {
    expect(pairingErrorView(new TypeError('fetch failed'), 0)).toMatchObject({ title: "Can't reach Allternit", step: 0 });
    expect(pairingErrorView(new Error('Pairing expired. Choose Get started to try again.'), 0)?.title).toBe('This pairing request expired');
    expect(pairingErrorView(new Error('Clerk session timed out'))?.title).toBe('This pairing request expired');
  });
});

describe('pairing card markup', () => {
  it('Welcome: computer name, empty steps, and the button the lab driver clicks', () => {
    const html = renderPairingCard({ phase: 'welcome' }, info);
    expect(html).toContain('Connect this computer');
    expect(html).toContain('debra · Linux · Allternit Desktop');
    expect(stepIcons(html)).toEqual(['todo', 'todo', 'todo']);
    expect(html).toMatch(/id="btn-get-started"[^>]*>Sign in to pair</);
  });

  it('Signing in and Approving: spinner on the current step and a Cancel', () => {
    const signingIn = renderPairingCard({ phase: 'signing-in' }, info);
    expect(stepIcons(signingIn)).toEqual(['current', 'todo', 'todo']);
    expect(signingIn).toContain('Finish signing in in the window that opened.');
    expect(signingIn).toContain('>Cancel<');
    expect(stepIcons(renderPairingCard({ phase: 'approving' }, info))).toEqual(['done', 'current', 'todo']);
  });

  it('Paired: all steps checked, the account, and Open Allternit', () => {
    const html = renderPairingCard({ phase: 'paired', email: 'you@example.com' }, info);
    expect(html).toContain('debra is connected');
    expect(html).toContain('Connected · debra');
    expect(stepIcons(html)).toEqual(['done', 'done', 'done']);
    expect(html).toContain('Signed in as you@example.com.');
    expect(html).toContain('>Open Allternit<');
  });

  it('Error: the card, Try again on the driver button, and the next step', () => {
    const error = pairingErrorView(new PairingFailure('server', 'x', 500))!;
    const html = renderPairingCard({ phase: 'error', error }, info);
    expect(html).toContain('role="alert"');
    expect(html).toContain("Couldn&#39;t approve this computer");
    expect(html).toMatch(/id="btn-get-started"[^>]*>Try again</);
    expect(html).toContain('>Check Allternit status<');
  });

  it('escapes the computer name and email', () => {
    const html = renderPairingCard({ phase: 'paired', email: '<b>x</b>' }, { computerName: '<img src=x>', platformLabel: 'Linux' });
    expect(html).not.toContain('<img src=x>');
    expect(html).not.toContain('<b>x</b>');
  });
});

describe('tray Connection Settings', () => {
  it('opens Settings › Computers once the workspace is loaded', () => {
    expect(connectionSettingsTarget({ platformUiResolved: true, workspaceLoadFailed: false, currentUrl: 'file:///app/platform/index.html' })).toBe('computers-settings');
  });

  it('falls back to the Connect page while the workspace cannot load', () => {
    expect(connectionSettingsTarget({ platformUiResolved: false, workspaceLoadFailed: false, currentUrl: '' })).toBe('connect-page');
    expect(connectionSettingsTarget({ platformUiResolved: true, workspaceLoadFailed: true, currentUrl: '' })).toBe('connect-page');
    expect(connectionSettingsTarget({ platformUiResolved: true, workspaceLoadFailed: false, currentUrl: 'file:///x/static/connect.html' })).toBe('connect-page');
  });

  it('counts only real main-frame failures as the workspace failing to load', () => {
    expect(isWorkspaceLoadFailure(-102, true, 'http://127.0.0.1:3013/')).toBe(true);
    expect(isWorkspaceLoadFailure(-3, true, 'http://127.0.0.1:3013/')).toBe(false);
    expect(isWorkspaceLoadFailure(-102, false, 'http://127.0.0.1:3013/frame')).toBe(false);
    expect(isWorkspaceLoadFailure(-6, true, 'file:///x/static/connect.html')).toBe(false);
  });
});
