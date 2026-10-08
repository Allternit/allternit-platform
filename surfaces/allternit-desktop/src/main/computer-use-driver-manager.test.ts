import { describe, expect, it } from 'vitest';
import * as path from 'node:path';
import { cuaCursorMotionStyle, cuaDriverBinaryName, defaultCuaSocketPath } from './computer-use-driver-manager.js';

describe('computer-use driver paths', () => {
  it('uses cua-driver.exe on Windows and cua-driver elsewhere', () => {
    expect(cuaDriverBinaryName('win32')).toBe('cua-driver.exe');
    expect(cuaDriverBinaryName('darwin')).toBe('cua-driver');
    expect(cuaDriverBinaryName('linux')).toBe('cua-driver');
  });

  it('uses a named pipe on Windows and a unix socket on unix', () => {
    expect(defaultCuaSocketPath('/tmp/runtime', 'win32')).toBe(String.raw`\\.\pipe\allternit-cua-driver`);
    expect(defaultCuaSocketPath('/tmp/runtime', 'darwin')).toBe(path.join('/tmp/runtime', 'cua-driver.sock'));
    expect(defaultCuaSocketPath('/tmp/runtime', 'linux')).toBe(path.join('/tmp/runtime', 'cua-driver.sock'));
  });
});

describe('computer-use cursor motion', () => {
  it('defaults to signature_arc and accepts the six driver styles', () => {
    expect(cuaCursorMotionStyle('')).toBe('signature_arc');
    expect(cuaCursorMotionStyle(undefined)).toBe('signature_arc');
    expect(cuaCursorMotionStyle(' Comet_Swoop ')).toBe('comet_swoop');
    expect(cuaCursorMotionStyle('magnetic')).toBe('magnetic');
    expect(cuaCursorMotionStyle('teleport')).toBe('signature_arc');
  });
});
