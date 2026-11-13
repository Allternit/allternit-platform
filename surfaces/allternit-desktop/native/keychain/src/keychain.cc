// keychain.node: reads and creates macOS login-keychain generic passwords off
// the JavaScript thread.
//
// Why this exists: Electron's safeStorage reads the "<app> Safe Storage" item
// synchronously on the main thread. When macOS shows its "allow access"
// prompt, the whole main process (UI, timers, Quit) freezes until the user
// answers. Here every Keychain call runs on its own detached std::thread and
// the result comes back through a Node-API threadsafe function, so an open
// prompt never blocks the event loop, a libuv pool thread, or process exit.
//
// Contract (see README.md):
//   getPassword(service, account) -> Promise<
//       {status:'ok', password: Buffer}
//     | {status:'not_found'}
//     | {status:'denied'|'error', code: number}>
//   addPassword(service, account, password: Buffer|string) -> Promise<
//       {status:'ok'|'duplicate'} | {status:'error', code: number}>
// The promises never reject for Keychain outcomes; they reject only for bad
// arguments (TypeError) or if a native thread cannot be started.
//
// Items are the classic generic-password class (kSecClassGenericPassword with
// kSecAttrService + kSecAttrAccount) in the file-based login keychain: the
// same item Chromium's OSCrypt / Electron safeStorage uses.

// memset_s (C11 Annex K) is only declared when this is set before <string.h>.
#define __STDC_WANT_LIB_EXT1__ 1
#include <string.h>

#define NAPI_VERSION 8
#include "node_api.h"

#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>

#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <system_error>
#include <thread>
#include <vector>

namespace {

// OSStatus values the JS side cares about. Spelled out so the mapping is
// visible next to the contract.
constexpr OSStatus kUserCanceled = -128;     // errSecUserCanceled
constexpr OSStatus kAuthFailed = -25293;     // errSecAuthFailed
constexpr OSStatus kItemNotFound = -25300;   // errSecItemNotFound
constexpr OSStatus kDuplicateItem = -25299;  // errSecDuplicateItem
static_assert(kUserCanceled == errSecUserCanceled, "errSecUserCanceled");
static_assert(kAuthFailed == errSecAuthFailed, "errSecAuthFailed");
static_assert(kItemNotFound == errSecItemNotFound, "errSecItemNotFound");
static_assert(kDuplicateItem == errSecDuplicateItem, "errSecDuplicateItem");

void WipeBytes(std::vector<uint8_t>& bytes) {
  if (!bytes.empty()) memset_s(bytes.data(), bytes.size(), 0, bytes.size());
  bytes.clear();
}

// Shared between the worker thread and the threadsafe function's finalizer.
// Node deletes a threadsafe function when its environment is torn down even
// if a thread still holds it, so the worker checks `alive` (under `mu`, held
// across the call) before touching the function. The finalizer flips it.
struct Lifeline {
  std::mutex mu;
  bool alive = true;
};

enum class Kind { kGet, kAdd };

struct Request {
  Kind kind;
  std::string service;
  std::string account;
  std::vector<uint8_t> password;  // input for add, output for get
  OSStatus status = errSecSuccess;
  napi_deferred deferred = nullptr;
  napi_threadsafe_function tsfn = nullptr;
  std::shared_ptr<Lifeline> lifeline;

  ~Request() { WipeBytes(password); }
};

// ---------------------------------------------------------------------------
// Keychain work (runs on the detached thread; no Node-API calls here)
// ---------------------------------------------------------------------------

CFStringRef MakeCFString(const std::string& s) {
  return CFStringCreateWithBytes(kCFAllocatorDefault,
                                 reinterpret_cast<const UInt8*>(s.data()),
                                 static_cast<CFIndex>(s.size()),
                                 kCFStringEncodingUTF8, false);
}

CFMutableDictionaryRef MakeBaseQuery(CFStringRef service, CFStringRef account) {
  CFMutableDictionaryRef q = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 0, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  if (!q) return nullptr;
  CFDictionarySetValue(q, kSecClass, kSecClassGenericPassword);
  CFDictionarySetValue(q, kSecAttrService, service);
  CFDictionarySetValue(q, kSecAttrAccount, account);
  return q;
}

void RunGet(Request* req) {
  CFStringRef service = MakeCFString(req->service);
  CFStringRef account = MakeCFString(req->account);
  CFMutableDictionaryRef query =
      (service && account) ? MakeBaseQuery(service, account) : nullptr;
  if (!query) {
    req->status = errSecAllocate;
  } else {
    CFDictionarySetValue(query, kSecReturnData, kCFBooleanTrue);
    CFDictionarySetValue(query, kSecMatchLimit, kSecMatchLimitOne);
    CFTypeRef result = nullptr;
    // May block for as long as the macOS access prompt is open.
    req->status = SecItemCopyMatching(query, &result);
    if (req->status == errSecSuccess) {
      if (result && CFGetTypeID(result) == CFDataGetTypeID()) {
        CFDataRef data = static_cast<CFDataRef>(result);
        const UInt8* bytes = CFDataGetBytePtr(data);
        req->password.assign(bytes, bytes + CFDataGetLength(data));
      } else {
        req->status = errSecDecode;
      }
    }
    if (result) CFRelease(result);
  }
  if (query) CFRelease(query);
  if (account) CFRelease(account);
  if (service) CFRelease(service);
}

void RunAdd(Request* req) {
  CFStringRef service = MakeCFString(req->service);
  CFStringRef account = MakeCFString(req->account);
  CFDataRef data = CFDataCreate(kCFAllocatorDefault, req->password.data(),
                                static_cast<CFIndex>(req->password.size()));
  CFMutableDictionaryRef attrs =
      (service && account && data) ? MakeBaseQuery(service, account) : nullptr;
  if (!attrs) {
    req->status = errSecAllocate;
  } else {
    // Chromium's item carries label == service; match it so Keychain Access
    // shows the same name either way.
    CFDictionarySetValue(attrs, kSecAttrLabel, service);
    CFDictionarySetValue(attrs, kSecValueData, data);
    req->status = SecItemAdd(attrs, nullptr);
  }
  WipeBytes(req->password);
  if (attrs) CFRelease(attrs);
  if (data) CFRelease(data);
  if (account) CFRelease(account);
  if (service) CFRelease(service);
}

void Work(Request* req) {
  if (req->kind == Kind::kGet) {
    RunGet(req);
  } else {
    RunAdd(req);
  }

  // Hand the result back to the JS thread.
  std::shared_ptr<Lifeline> lifeline = req->lifeline;
  std::lock_guard<std::mutex> lock(lifeline->mu);
  if (!lifeline->alive) {
    // Environment already torn down (app quitting). Nobody is waiting.
    delete req;
    return;
  }
  // Copy first: once queued, the JS thread may delete req at any moment.
  napi_threadsafe_function tsfn = req->tsfn;
  napi_status s = napi_call_threadsafe_function(tsfn, req, napi_tsfn_nonblocking);
  if (s == napi_closing) {
    // Function is closing and has already dropped this thread's reference;
    // it must not be touched again (no release).
    delete req;
    return;
  }
  if (s != napi_ok) delete req;
  napi_release_threadsafe_function(tsfn, napi_tsfn_release);
}

// ---------------------------------------------------------------------------
// JS-thread side
// ---------------------------------------------------------------------------

napi_value MakeString(napi_env env, const char* s) {
  napi_value v = nullptr;
  napi_create_string_utf8(env, s, NAPI_AUTO_LENGTH, &v);
  return v;
}

void SetStatus(napi_env env, napi_value obj, const char* status) {
  napi_set_named_property(env, obj, "status", MakeString(env, status));
}

void SetCode(napi_env env, napi_value obj, OSStatus code) {
  napi_value v = nullptr;
  napi_create_int32(env, static_cast<int32_t>(code), &v);
  napi_set_named_property(env, obj, "code", v);
}

napi_value BuildResult(napi_env env, Request* req) {
  napi_value obj = nullptr;
  napi_create_object(env, &obj);
  if (req->kind == Kind::kGet) {
    if (req->status == errSecSuccess) {
      SetStatus(env, obj, "ok");
      napi_value buf = nullptr;
      void* unused = nullptr;
      // Copy, not an external buffer: Electron's V8 sandbox forbids
      // external backing stores.
      static const uint8_t kEmpty = 0;
      napi_create_buffer_copy(env, req->password.size(),
                              req->password.empty() ? &kEmpty : req->password.data(),
                              &unused, &buf);
      napi_set_named_property(env, obj, "password", buf);
    } else if (req->status == kItemNotFound) {
      SetStatus(env, obj, "not_found");
    } else if (req->status == kUserCanceled || req->status == kAuthFailed) {
      SetStatus(env, obj, "denied");
      SetCode(env, obj, req->status);
    } else {
      SetStatus(env, obj, "error");
      SetCode(env, obj, req->status);
    }
  } else {
    if (req->status == errSecSuccess) {
      SetStatus(env, obj, "ok");
    } else if (req->status == kDuplicateItem) {
      SetStatus(env, obj, "duplicate");
    } else {
      SetStatus(env, obj, "error");
      SetCode(env, obj, req->status);
    }
  }
  return obj;
}

void CallJs(napi_env env, napi_value /*js_cb*/, void* /*context*/, void* data) {
  Request* req = static_cast<Request*>(data);
  // env == nullptr: Node is draining the queue during teardown. Just free.
  if (env != nullptr && req->deferred != nullptr) {
    napi_resolve_deferred(env, req->deferred, BuildResult(env, req));
    req->deferred = nullptr;
  }
  delete req;
}

void FinalizeTsfn(napi_env /*env*/, void* finalize_data, void* /*hint*/) {
  auto* holder = static_cast<std::shared_ptr<Lifeline>*>(finalize_data);
  {
    std::lock_guard<std::mutex> lock((*holder)->mu);
    (*holder)->alive = false;
  }
  delete holder;
}

void RejectTypeError(napi_env env, napi_deferred deferred, const char* message) {
  napi_value msg = MakeString(env, message);
  napi_value err = nullptr;
  napi_create_type_error(env, nullptr, msg, &err);
  napi_reject_deferred(env, deferred, err);
}

void RejectError(napi_env env, napi_deferred deferred, const char* message) {
  napi_value msg = MakeString(env, message);
  napi_value err = nullptr;
  napi_create_error(env, nullptr, msg, &err);
  napi_reject_deferred(env, deferred, err);
}

// Reads a non-empty JS string into `out`.
bool ReadString(napi_env env, napi_value value, std::string* out) {
  napi_valuetype type;
  if (napi_typeof(env, value, &type) != napi_ok || type != napi_string) return false;
  size_t len = 0;
  if (napi_get_value_string_utf8(env, value, nullptr, 0, &len) != napi_ok) return false;
  if (len == 0) return false;
  std::string s(len + 1, '\0');
  size_t written = 0;
  if (napi_get_value_string_utf8(env, value, &s[0], s.size(), &written) != napi_ok) {
    return false;
  }
  s.resize(written);
  *out = std::move(s);
  return true;
}

// Reads a non-empty Buffer / Uint8Array / string into `out`.
bool ReadSecret(napi_env env, napi_value value, std::vector<uint8_t>* out) {
  bool is_buffer = false;
  napi_is_buffer(env, value, &is_buffer);
  if (is_buffer) {
    void* data = nullptr;
    size_t len = 0;
    if (napi_get_buffer_info(env, value, &data, &len) != napi_ok || len == 0) return false;
    const uint8_t* p = static_cast<const uint8_t*>(data);
    out->assign(p, p + len);
    return true;
  }
  bool is_typed = false;
  napi_is_typedarray(env, value, &is_typed);
  if (is_typed) {
    napi_typedarray_type t;
    size_t len = 0;
    void* data = nullptr;
    if (napi_get_typedarray_info(env, value, &t, &len, &data, nullptr, nullptr) != napi_ok ||
        t != napi_uint8_array || len == 0) {
      return false;
    }
    const uint8_t* p = static_cast<const uint8_t*>(data);
    out->assign(p, p + len);
    return true;
  }
  std::string s;
  if (!ReadString(env, value, &s)) return false;
  out->assign(s.begin(), s.end());
  memset_s(&s[0], s.size(), 0, s.size());
  return true;
}

napi_value Start(napi_env env, napi_callback_info info, Kind kind) {
  size_t argc = 3;
  napi_value argv[3] = {nullptr, nullptr, nullptr};
  napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr);

  napi_deferred deferred = nullptr;
  napi_value promise = nullptr;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return nullptr;

  std::unique_ptr<Request> req(new Request());
  req->kind = kind;
  if (argc < 2 || !ReadString(env, argv[0], &req->service) ||
      !ReadString(env, argv[1], &req->account)) {
    RejectTypeError(env, deferred, "service and account must be non-empty strings");
    return promise;
  }
  if (kind == Kind::kAdd && (argc < 3 || !ReadSecret(env, argv[2], &req->password))) {
    RejectTypeError(env, deferred, "password must be a non-empty Buffer or string");
    return promise;
  }

  auto lifeline = std::make_shared<Lifeline>();
  auto* holder = new std::shared_ptr<Lifeline>(lifeline);
  napi_threadsafe_function tsfn = nullptr;
  napi_status s = napi_create_threadsafe_function(
      env, nullptr, nullptr,
      MakeString(env, kind == Kind::kGet ? "keychain.getPassword" : "keychain.addPassword"),
      0 /* unlimited queue */, 1 /* this worker */, holder, FinalizeTsfn,
      nullptr, CallJs, &tsfn);
  if (s != napi_ok) {
    delete holder;
    RejectError(env, deferred, "keychain: could not create threadsafe function");
    return promise;
  }
  // A pending Keychain call must never keep the process alive.
  napi_unref_threadsafe_function(env, tsfn);

  req->deferred = deferred;
  req->tsfn = tsfn;
  req->lifeline = lifeline;
  Request* raw = req.release();
  try {
    std::thread(Work, raw).detach();
  } catch (const std::system_error&) {
    raw->deferred = nullptr;
    delete raw;
    napi_release_threadsafe_function(tsfn, napi_tsfn_release);
    RejectError(env, deferred, "keychain: could not start worker thread");
  }
  return promise;
}

napi_value GetPassword(napi_env env, napi_callback_info info) {
  return Start(env, info, Kind::kGet);
}

napi_value AddPassword(napi_env env, napi_callback_info info) {
  return Start(env, info, Kind::kAdd);
}

}  // namespace

NAPI_MODULE_INIT() {
  napi_property_descriptor props[] = {
      {"getPassword", nullptr, GetPassword, nullptr, nullptr, nullptr, napi_enumerable, nullptr},
      {"addPassword", nullptr, AddPassword, nullptr, nullptr, nullptr, napi_enumerable, nullptr},
  };
  napi_define_properties(env, exports, sizeof(props) / sizeof(props[0]), props);
  return exports;
}
