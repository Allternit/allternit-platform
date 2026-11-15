// JNI layer between com.allternit.ondevice.NativeLlama and llama.cpp.
//
// One loaded model at a time per handle (EngineService keeps at most one). Every call on a
// handle comes from the engine's single worker thread, except stop(), which only flips an
// atomic flag read by the generate loop and the decode abort callback.
//
// Chat is stateless at the API: the caller sends the whole conversation each time. The KV
// cache is reused for the longest token prefix shared with the previous request, so a
// follow-up turn only decodes the new tokens.
#include <android/log.h>
#include <jni.h>

#include <algorithm>
#include <atomic>
#include <cstring>
#include <string>
#include <thread>
#include <unistd.h>
#include <vector>

#include "chat.h"
#include "common.h"
#include "ggml-backend.h"
#include "llama.h"
#include "sampling.h"

#define TAG "AllternitEngine"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

namespace {

struct Engine {
    llama_model * model = nullptr;
    llama_context * ctx = nullptr;
    common_chat_templates_ptr templates;
    std::vector<llama_token> cached;  // tokens currently in the KV cache (seq 0)
    std::atomic<bool> stop{false};
    std::string backend;              // "OpenCL" or "CPU"
    int n_ctx = 0;
};

void log_callback(ggml_log_level level, const char * text, void *) {
    int prio = ANDROID_LOG_DEBUG;
    if (level == GGML_LOG_LEVEL_ERROR) prio = ANDROID_LOG_ERROR;
    else if (level == GGML_LOG_LEVEL_WARN) prio = ANDROID_LOG_WARN;
    else if (level == GGML_LOG_LEVEL_INFO) prio = ANDROID_LOG_INFO;
    __android_log_write(prio, "llama.cpp", text);
}

std::string jstr(JNIEnv * env, jstring s) {
    if (!s) return {};
    const char * c = env->GetStringUTFChars(s, nullptr);
    std::string out(c);
    env->ReleaseStringUTFChars(s, c);
    return out;
}

std::string json_escape(const std::string & s) {
    std::string o;
    o.reserve(s.size() + 8);
    for (char c : s) {
        switch (c) {
            case '"': o += "\\\""; break;
            case '\\': o += "\\\\"; break;
            case '\n': o += "\\n"; break;
            case '\r': o += "\\r"; break;
            case '\t': o += "\\t"; break;
            default:
                if ((unsigned char) c < 0x20) {
                    char buf[8];
                    snprintf(buf, sizeof buf, "\\u%04x", c);
                    o += buf;
                } else {
                    o += c;
                }
        }
    }
    return o;
}

// Length of the longest prefix of `s` that ends on a UTF-8 character boundary. Token pieces
// can split a multi-byte character; the rest waits for the next token.
size_t utf8_complete_prefix(const std::string & s) {
    size_t n = s.size();
    if (n == 0) return 0;
    // Walk back over at most 3 continuation bytes to the lead byte of the last character.
    size_t i = n;
    int back = 0;
    while (i > 0 && back < 4) {
        unsigned char c = (unsigned char) s[i - 1];
        if ((c & 0xC0) != 0x80) {
            size_t need = (c & 0x80) == 0 ? 1 : (c & 0xE0) == 0xC0 ? 2 : (c & 0xF0) == 0xE0 ? 3 : (c & 0xF8) == 0xF0 ? 4 : 1;
            return (n - (i - 1) >= need) ? n : i - 1;
        }
        --i;
        ++back;
    }
    return n;
}

bool abort_cb(void * data) {
    return static_cast<Engine *>(data)->stop.load();
}

int default_threads() {
    // Big cores only where we can tell; 4 is right for the common 4+4 phone layouts.
    long cpus = sysconf(_SC_NPROCESSORS_ONLN);
    return (int) std::max(2L, std::min(4L, cpus > 4 ? cpus / 2 : cpus));
}

ggml_backend_dev_t find_gpu() {
    for (size_t i = 0; i < ggml_backend_dev_count(); i++) {
        ggml_backend_dev_t d = ggml_backend_dev_get(i);
        auto t = ggml_backend_dev_type(d);
        if (t == GGML_BACKEND_DEVICE_TYPE_GPU || t == GGML_BACKEND_DEVICE_TYPE_IGPU) return d;
    }
    return nullptr;
}

}  // namespace

extern "C" {

// Loads every backend .so from the app's native lib dir and returns a JSON array of the
// devices that came up, e.g. [{"name":"GPUOpenCL","desc":"QUALCOMM Adreno(TM) 610","type":"gpu"},
// {"name":"CPU","desc":"...","type":"cpu"}].
JNIEXPORT jstring JNICALL
Java_com_allternit_ondevice_NativeLlama_nativeInit(JNIEnv * env, jclass, jstring jlibdir) {
    llama_log_set(log_callback, nullptr);
    std::string dir = jstr(env, jlibdir);
    ggml_backend_load_all_from_path(dir.c_str());
    llama_backend_init();
    std::string out = "[";
    for (size_t i = 0; i < ggml_backend_dev_count(); i++) {
        ggml_backend_dev_t d = ggml_backend_dev_get(i);
        auto t = ggml_backend_dev_type(d);
        const char * ts = t == GGML_BACKEND_DEVICE_TYPE_CPU ? "cpu"
                        : t == GGML_BACKEND_DEVICE_TYPE_GPU ? "gpu"
                        : t == GGML_BACKEND_DEVICE_TYPE_IGPU ? "igpu" : "accel";
        if (i) out += ",";
        out += "{\"name\":\"" + json_escape(ggml_backend_dev_name(d)) + "\",\"desc\":\"" +
               json_escape(ggml_backend_dev_description(d)) + "\",\"type\":\"" + ts + "\"}";
    }
    out += "]";
    LOGI("backends: %s", out.c_str());
    return env->NewStringUTF(out.c_str());
}

// Returns a handle (0 on failure). useGpu: offload every layer to the first GPU device
// (OpenCL on Adreno). The caller retries with useGpu=false when a GPU load fails.
JNIEXPORT jlong JNICALL
Java_com_allternit_ondevice_NativeLlama_nativeLoad(JNIEnv * env, jclass, jstring jpath, jint n_ctx, jint n_threads,
                                                    jboolean use_gpu) {
    std::string path = jstr(env, jpath);
    auto * e = new Engine();
    ggml_backend_dev_t gpu = use_gpu ? find_gpu() : nullptr;
    ggml_backend_dev_t devs[2] = {gpu, nullptr};

    llama_model_params mp = llama_model_default_params();
    mp.devices = devs;  // {gpu, NULL} or {NULL}: an empty list keeps everything on the CPU
    mp.n_gpu_layers = gpu ? -1 : 0;
    e->model = llama_model_load_from_file(path.c_str(), mp);
    if (!e->model) {
        LOGE("model load failed (%s, gpu=%d)", path.c_str(), gpu != nullptr);
        delete e;
        return 0;
    }
    int train = llama_model_n_ctx_train(e->model);
    int want = n_ctx > 0 ? n_ctx : 2048;
    if (train > 0) want = std::min(want, train);

    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = want;
    cp.n_batch = 512;
    cp.n_ubatch = 512;
    int threads = n_threads > 0 ? n_threads : default_threads();
    cp.n_threads = threads;
    cp.n_threads_batch = threads;
    cp.abort_callback = abort_cb;
    cp.abort_callback_data = e;
    cp.offload_kqv = gpu != nullptr;
    cp.no_perf = false;
    e->ctx = llama_init_from_model(e->model, cp);
    if (!e->ctx) {
        LOGE("context init failed (n_ctx=%d, gpu=%d)", want, gpu != nullptr);
        llama_model_free(e->model);
        delete e;
        return 0;
    }
    e->n_ctx = (int) llama_n_ctx(e->ctx);
    e->templates = common_chat_templates_init(e->model, "");
    e->backend = gpu ? ggml_backend_dev_name(gpu) : "CPU";
    LOGI("loaded %s: backend=%s n_ctx=%d threads=%d", path.c_str(), e->backend.c_str(), e->n_ctx, threads);
    return reinterpret_cast<jlong>(e);
}

// {"backend":"GPUOpenCL","n_ctx":2048,"model_bytes":...,"desc":"qwen3 0.6B Q4_K - Medium"}
JNIEXPORT jstring JNICALL
Java_com_allternit_ondevice_NativeLlama_nativeInfo(JNIEnv * env, jclass, jlong h) {
    auto * e = reinterpret_cast<Engine *>(h);
    if (!e) return env->NewStringUTF("{}");
    char desc[256] = {0};
    llama_model_desc(e->model, desc, sizeof desc);
    std::string out = "{\"backend\":\"" + json_escape(e->backend) + "\",\"n_ctx\":" + std::to_string(e->n_ctx) +
                      ",\"model_bytes\":" + std::to_string(llama_model_size(e->model)) + ",\"desc\":\"" +
                      json_escape(desc) + "\"}";
    return env->NewStringUTF(out.c_str());
}

// Streams one assistant reply. sink.onToken(byte[] utf8) returns false to stop. Returns a
// JSON object: {"finish":"stop|length|cancel|error","prompt_tokens":n,"completion_tokens":n,
// "reused_tokens":n,"prompt_ms":x,"gen_ms":x,"tok_per_sec":x,"error":"..."}.
JNIEXPORT jstring JNICALL
Java_com_allternit_ondevice_NativeLlama_nativeChat(JNIEnv * env, jclass, jlong h, jobjectArray jroles,
                                                    jobjectArray jcontents, jint max_tokens, jfloat temperature,
                                                    jint seed, jboolean thinking, jobject sink) {
    auto * e = reinterpret_cast<Engine *>(h);
    auto fail = [&](const std::string & msg) {
        std::string out = "{\"finish\":\"error\",\"error\":\"" + json_escape(msg) + "\"}";
        return env->NewStringUTF(out.c_str());
    };
    if (!e) return fail("no model loaded");
    e->stop.store(false);

    jclass sink_cls = env->GetObjectClass(sink);
    jmethodID on_token = env->GetMethodID(sink_cls, "onToken", "([B)Z");
    if (!on_token) return fail("sink has no onToken([B)Z");

    common_chat_templates_inputs inputs;
    jsize n = env->GetArrayLength(jroles);
    for (jsize i = 0; i < n; i++) {
        common_chat_msg m;
        auto r = (jstring) env->GetObjectArrayElement(jroles, i);
        auto c = (jstring) env->GetObjectArrayElement(jcontents, i);
        m.role = jstr(env, r);
        m.content = jstr(env, c);
        env->DeleteLocalRef(r);
        env->DeleteLocalRef(c);
        inputs.messages.push_back(std::move(m));
    }
    inputs.add_generation_prompt = true;
    inputs.use_jinja = true;
    inputs.enable_thinking = thinking;
    std::string prompt;
    try {
        prompt = common_chat_templates_apply(e->templates.get(), inputs).prompt.str();
    } catch (const std::exception & ex) {
        LOGW("jinja template failed (%s), trying the legacy template", ex.what());
        try {
            inputs.use_jinja = false;
            prompt = common_chat_templates_apply(e->templates.get(), inputs).prompt.str();
        } catch (const std::exception & ex2) {
            return fail(std::string("chat template: ") + ex2.what());
        }
    }

    std::vector<llama_token> tokens = common_tokenize(e->ctx, prompt, true, true);
    if (tokens.empty()) return fail("empty prompt");
    const int budget_tokens = std::max(1, (int) max_tokens);
    if ((int) tokens.size() + 8 >= e->n_ctx) return fail("conversation is longer than the context (" + std::to_string(e->n_ctx) + " tokens)");

    // Reuse the KV cache for the shared prefix; always decode at least the last token.
    size_t keep = 0;
    while (keep < e->cached.size() && keep < tokens.size() && e->cached[keep] == tokens[keep]) keep++;
    if (keep >= tokens.size()) keep = tokens.size() - 1;
    llama_memory_t mem = llama_get_memory(e->ctx);
    if (!llama_memory_seq_rm(mem, 0, (llama_pos) keep, -1)) {
        llama_memory_clear(mem, true);
        keep = 0;
    }
    e->cached.resize(keep);

    const int64_t t0 = ggml_time_us();
    for (size_t i = keep; i < tokens.size(); i += 512) {
        int32_t len = (int32_t) std::min<size_t>(512, tokens.size() - i);
        llama_batch b = llama_batch_get_one(tokens.data() + i, len);
        int rc = llama_decode(e->ctx, b);
        if (rc != 0) {
            llama_memory_clear(mem, true);
            e->cached.clear();
            if (e->stop.load()) {
                return env->NewStringUTF("{\"finish\":\"cancel\",\"completion_tokens\":0}");
            }
            return fail("prompt decode failed (" + std::to_string(rc) + ")");
        }
        e->cached.insert(e->cached.end(), tokens.begin() + i, tokens.begin() + i + len);
    }
    const int64_t t1 = ggml_time_us();

    common_params_sampling sp;
    sp.temp = temperature;
    if (seed >= 0) sp.seed = (uint32_t) seed;
    common_sampler * smpl = common_sampler_init(e->model, sp);
    const llama_vocab * vocab = llama_model_get_vocab(e->model);

    std::string pending;
    int generated = 0;
    std::string finish = "length";
    while (generated < budget_tokens) {
        if (e->stop.load()) { finish = "cancel"; break; }
        if ((int) e->cached.size() + 1 >= e->n_ctx) { finish = "length"; break; }
        llama_token tok = common_sampler_sample(smpl, e->ctx, -1);
        common_sampler_accept(smpl, tok, true);
        if (llama_vocab_is_eog(vocab, tok)) { finish = "stop"; break; }
        generated++;
        pending += common_token_to_piece(e->ctx, tok, false);
        size_t ready = utf8_complete_prefix(pending);
        bool keep_going = true;
        if (ready > 0) {
            jbyteArray arr = env->NewByteArray((jsize) ready);
            env->SetByteArrayRegion(arr, 0, (jsize) ready, reinterpret_cast<const jbyte *>(pending.data()));
            keep_going = env->CallBooleanMethod(sink, on_token, arr);
            env->DeleteLocalRef(arr);
            if (env->ExceptionCheck()) { env->ExceptionClear(); keep_going = false; }
            pending.erase(0, ready);
        }
        llama_batch b = llama_batch_get_one(&tok, 1);
        if (llama_decode(e->ctx, b) != 0) {
            finish = e->stop.load() ? "cancel" : "error";
            llama_memory_clear(mem, true);
            e->cached.clear();
            break;
        }
        e->cached.push_back(tok);
        if (!keep_going) { finish = "cancel"; break; }
    }
    common_sampler_free(smpl);
    const int64_t t2 = ggml_time_us();
    double gen_ms = (t2 - t1) / 1000.0;
    double tps = gen_ms > 0 ? generated * 1000.0 / gen_ms : 0;
    char buf[512];
    snprintf(buf, sizeof buf,
             "{\"finish\":\"%s\",\"prompt_tokens\":%d,\"completion_tokens\":%d,\"reused_tokens\":%d,"
             "\"prompt_ms\":%.1f,\"gen_ms\":%.1f,\"tok_per_sec\":%.2f,\"ctx_used\":%d,\"ctx_max\":%d}",
             finish.c_str(), (int) tokens.size(), generated, (int) keep, (t1 - t0) / 1000.0, gen_ms, tps,
             (int) e->cached.size(), e->n_ctx);
    LOGI("chat done %s", buf);
    return env->NewStringUTF(buf);
}

JNIEXPORT void JNICALL
Java_com_allternit_ondevice_NativeLlama_nativeStop(JNIEnv *, jclass, jlong h) {
    auto * e = reinterpret_cast<Engine *>(h);
    if (e) e->stop.store(true);
}

JNIEXPORT void JNICALL
Java_com_allternit_ondevice_NativeLlama_nativeFree(JNIEnv *, jclass, jlong h) {
    auto * e = reinterpret_cast<Engine *>(h);
    if (!e) return;
    e->templates.reset();
    if (e->ctx) llama_free(e->ctx);
    if (e->model) llama_model_free(e->model);
    delete e;
}

}  // extern "C"
