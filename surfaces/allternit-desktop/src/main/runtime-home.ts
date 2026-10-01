/**
 * The app's runtime package store (see runtime-package.ts). Resolve
 * allternit-api, gizzi-code and the platform screens through
 * runtimeResource() so an updated runtime package wins over the bundled copy.
 */
import { app } from 'electron';
import * as path from 'path';
import log from 'electron-log';
import { RuntimePackages } from './runtime-package.js';

let instance: RuntimePackages | null = null;

export function runtimePackages(): RuntimePackages {
  instance ??= new RuntimePackages({
    root: path.join(app.getPath('userData'), 'runtime'),
    resourcesPath: process.resourcesPath ?? '',
    log,
  });
  return instance;
}

/** A runtime file: the active runtime package's copy when it has one, else the bundled resource. */
export function runtimeResource(...segments: string[]): string {
  return runtimePackages().file(...segments);
}
