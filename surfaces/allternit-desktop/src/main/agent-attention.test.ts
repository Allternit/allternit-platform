import { describe, expect, it } from 'vitest';
import { attentionNotice, parseAttentionInput, parseNeedsYou, resumeTaskMessage } from './agent-attention.js';

describe('agent attention notices', () => {
  it('uses the contract copy and the run deep link', () => {
    expect(attentionNotice({ kind: 'credential', name: 'Gmail', domain: 'mail.google.com', computerId: 'c1', runId: 'r1' }))
      .toMatchObject({ title: 'Fill your Gmail login on mail.google.com?', route: '/computers/c1?run=r1' });
    expect(attentionNotice({ kind: 'human', botName: 'Gizzi' }).title).toBe('Gizzi needs you');
    expect(attentionNotice({ kind: 'blocked', botName: 'Gizzi', domain: 'x.com' }).title).toBe('x.com blocked Gizzi');
  });
  it('drops bad kinds and unsafe ids from the page', () => {
    expect(parseAttentionInput({ kind: 'nope' })).toBeNull();
    expect(parseAttentionInput({ kind: 'human', runId: '../x y' })?.runId).toBeUndefined();
  });
});

describe('extension bridge messages', () => {
  it('turns needs_you into a domain and handoff, and answers with resume_task', () => {
    expect(parseNeedsYou({ type: 'needs_you', payload: { kind: 'credential', url: 'https://a.example.com/login', handoffId: 'h-1' } }))
      .toMatchObject({ kind: 'credential', domain: 'a.example.com', handoffId: 'h-1' });
    expect(parseNeedsYou({ type: 'needs_you', payload: { kind: 'credential' } })).toBeNull();
    expect(resumeTaskMessage('h-1', true)).toEqual({ type: 'resume_task', payload: { handoffId: 'h-1', approved: true } });
    expect(resumeTaskMessage('h-1', 'yes')).toBeNull();
  });
});
