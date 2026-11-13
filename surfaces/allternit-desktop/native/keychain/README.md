# keychain.node: macOS Keychain reads off the main thread

Electron's `safeStorage` reads the `<app name> Safe Storage` password from the
macOS login keychain synchronously, on the main thread. When macOS shows its
"allow access" prompt, the whole main process stops (window, timers, Quit)
until someone answers. This addon does the Keychain call on its own detached
native thread and hands the result back as a Promise. The main thread keeps
running while the prompt is open.

`src/main/os-crypt.ts` is the only caller. It turns the password into the
Chromium OSCrypt key and does the encryption in JS (see the plan's Decision 4).
macOS only: Windows and Linux keep using `safeStorage`.

## API

```ts
getPassword(service: string, account: string): Promise<
  | { status: 'ok'; password: Buffer }
  | { status: 'not_found' }
  | { status: 'denied' | 'error'; code: number }>

addPassword(service: string, account: string, password: Buffer | string): Promise<
  | { status: 'ok' | 'duplicate' }
  | { status: 'error'; code: number }>
```

- Items are classic generic passwords in the file-based login keychain
  (`kSecClassGenericPassword`, `kSecAttrService`, `kSecAttrAccount`). That is
  the same item Chromium / `safeStorage` reads and writes, so existing data
  stays readable. `addPassword` also sets the label to the service name, as
  Chromium does.
- `code` is the macOS `OSStatus`. `denied` means `errSecUserCanceled` (-128)
  or `errSecAuthFailed` (-25293). `not_found` means `errSecItemNotFound`
  (-25300). `duplicate` means `errSecDuplicateItem` (-25299).
- Keychain results never reject. The promise rejects only for bad arguments
  (`TypeError`: empty or non-string service/account, empty password) or if a
  thread can't be started.
- `getPassword` may stay pending for as long as the macOS prompt is open.
  Callers that need a timeout race it themselves.

## Threading and exit

- Each call starts one `std::thread`, detaches it, and returns the result
  through a Node-API threadsafe function. It does not use `napi_async_work`, so
  an open prompt never holds one of libuv's four pool threads.
- The threadsafe function is unref'd, so a pending call never keeps the
  process alive. In Electron the app loop keeps running anyway. **In plain
  `node`, keep the event loop alive yourself while you wait** (for example a
  `setInterval`), or node exits with the promise still pending.
- If the app quits while a prompt is open, the thread is simply abandoned. A
  mutex-guarded flag, flipped by the threadsafe function's finalizer, stops the
  thread from touching the function after Node tears the environment down.
- Password bytes are copied into the Buffer (`napi_create_buffer_copy`).
  Electron's V8 sandbox does not allow external buffers. The native copies are
  wiped with `memset_s` after use.

## Build

`npm run prepare:keychain-addon` (`scripts/prepare-keychain-addon.cjs`) runs
one `xcrun clang++` call: `-std=c++17 -arch arm64 -arch x86_64 -bundle
-undefined dynamic_lookup -framework Security -framework CoreFoundation`, with
no node-gyp. Node-API is ABI-stable (built for `NAPI_VERSION 8`), so the same
universal file loads in plain node and in every Electron version without a
rebuild. Output: `build/keychain.node` (gitignored).

- The script does nothing off macOS, and skips the compile when the output is
  newer than every source.
- `dev`, `build:electron`, `build:electron:dmg`, `pack` and `dist` run it.
- The mac `extraResources` entry in `package.json` ships it to
  `Contents/Resources/native/keychain/keychain.node`.
  `scripts/verify-packaged-resources.cjs` fails a mac build if the file, either
  arch, or that entry is missing.

## Smoke test (plain node)

```sh
security add-generic-password -s allternit-addon-smoke -a smoke -w hello -A
node -e '
  const kc = require("./native/keychain/build/keychain.node");
  const keep = setInterval(() => {}, 1000);
  kc.getPassword("allternit-addon-smoke", "smoke")
    .then((r) => console.log(r.status, r.password && r.password.toString()))
    .finally(() => clearInterval(keep));'
security delete-generic-password -s allternit-addon-smoke
```

Homebrew `node` is not an Apple-signed tool. Even with `-A`, macOS may show a
keychain prompt for it, and the call then returns `denied` if the prompt is
dismissed. Never point a smoke test at the real `@allternit/desktop Safe
Storage` item.

## Vendored headers

`include/node-api/` holds the four Node-API headers, copied unchanged from
Node.js v26.13.1, under Node's MIT licence (`include/node-api/LICENSE`).
