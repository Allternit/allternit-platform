import * as fs from "node:fs";
import * as path from "node:path";
import { app } from "electron";
import { decryptString, encryptString, isEncryptionAvailable, KeychainUnavailableError } from "./os-crypt.js";
import { keychainUnavailable } from "./secure-store.js";

type SecretFile = Record<string, Record<string, string>>;

function filePath(): string {
  return path.join(app.getPath("userData"), "mini-app-secrets.json");
}
function read(): SecretFile {
  try {
    return JSON.parse(fs.readFileSync(filePath(), "utf8")) as SecretFile;
  } catch {
    return {};
  }
}
function write(data: SecretFile): void {
  fs.mkdirSync(path.dirname(filePath()), { recursive: true });
  fs.writeFileSync(filePath(), JSON.stringify(data), { mode: 0o600 });
}
function valid(value: string): boolean {
  return /^[A-Za-z0-9._:-]{1,200}$/.test(value);
}

export function setMiniAppSecret(
  appId: string,
  name: string,
  value: string,
): { success: boolean; error?: string } {
  if (!valid(appId) || !/^[A-Z][A-Z0-9_]{1,127}$/.test(name))
    return { success: false, error: "Invalid app or secret name" };
  if (!isEncryptionAvailable()) {
    const blocked = keychainUnavailable();
    return {
      success: false,
      error: blocked?.reason === "pending"
        ? "Waiting for macOS Keychain permission"
        : "Operating-system encryption is unavailable",
    };
  }
  const data = read();
  data[appId] ||= {};
  data[appId][name] = encryptString(value).toString("base64");
  write(data);
  return { success: true };
}

export function listMiniAppSecrets(appId: string): string[] {
  return Object.keys(read()[appId] || {});
}
/**
 * Main-process only: values are never exposed through preload or IPC.
 * Throws KeychainUnavailableError when a stored value can't be opened because
 * the macOS Keychain is locked, so an app never starts silently without it.
 */
export function getMiniAppSecretEnvironment(appId: string, allowedNames: string[]): Record<string, string> {
  const encrypted = read()[appId] || {};
  const environment: Record<string, string> = {};
  for (const name of allowedNames) {
    const value = encrypted[name];
    if (!value) continue;
    const blocked = keychainUnavailable();
    if (blocked) throw blocked;
    try {
      environment[name] = decryptString(Buffer.from(value, "base64"));
    } catch (error) {
      if (error instanceof KeychainUnavailableError) throw error;
      /* Ignore damaged values. */
    }
  }
  return environment;
}
export function deleteAllMiniAppSecrets(appId: string): void {
  const data = read();
  if (data[appId]) { delete data[appId]; write(data); }
}
export function deleteMiniAppSecret(
  appId: string,
  name: string,
): { success: boolean } {
  const data = read();
  if (data[appId]) {
    delete data[appId][name];
    write(data);
  }
  return { success: true };
}
