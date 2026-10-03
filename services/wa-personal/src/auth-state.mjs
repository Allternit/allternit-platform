// Baileys auth state kept in one encrypted file (instead of useMultiFileAuthState's plaintext JSON files).
import { readSealed, writeSealed } from './crypto.mjs';

export function useEncryptedAuthState(baileys, file, key) {
  const { BufferJSON, initAuthCreds, proto } = baileys;
  const saved = readSealed(file, key);
  const data = saved ? JSON.parse(saved, BufferJSON.reviver) : { creds: initAuthCreds(), keys: {} };
  const persist = () => writeSealed(file, key, JSON.stringify(data, BufferJSON.replacer));
  return {
    state: {
      creds: data.creds,
      keys: {
        get: async (type, ids) => {
          const out = {};
          for (const id of ids) {
            let v = data.keys[type]?.[id];
            if (type === 'app-state-sync-key' && v) v = proto.Message.AppStateSyncKeyData.fromObject(v);
            if (v !== undefined) out[id] = v;
          }
          return out;
        },
        set: async (batch) => {
          for (const [type, entries] of Object.entries(batch)) {
            data.keys[type] ??= {};
            for (const [id, v] of Object.entries(entries)) {
              if (v) data.keys[type][id] = v;
              else delete data.keys[type][id];
            }
          }
          persist();
        },
      },
    },
    saveCreds: async () => persist(),
  };
}
