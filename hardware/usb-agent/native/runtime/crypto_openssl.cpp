// Native board: the mbedTLS calls the firmware makes, on OpenSSL 3
// (Apache-2.0, linked, not copied; decision 4). Written from the mbedTLS
// signatures and documented return codes in esp32s3-libs 3.3.12.
#include <mbedtls/base64.h>
#include <mbedtls/gcm.h>
#include <mbedtls/md.h>
#include <mbedtls/sha1.h>
#include <openssl/core_names.h>
#include <openssl/crypto.h>
#include <openssl/evp.h>
#include <string.h>

struct mbedtls_md_info_t {
  mbedtls_md_type_t type;
  const char *name;  // OpenSSL digest name
  size_t size;
};

static const mbedtls_md_info_t INFO_SHA1 = {MBEDTLS_MD_SHA1, "SHA1", 20};
static const mbedtls_md_info_t INFO_SHA256 = {MBEDTLS_MD_SHA256, "SHA256", 32};

extern "C" {

// ── md ──────────────────────────────────────────────────────────────────────

const mbedtls_md_info_t *mbedtls_md_info_from_type(mbedtls_md_type_t md_type) {
  switch (md_type) {
    case MBEDTLS_MD_SHA1: return &INFO_SHA1;
    case MBEDTLS_MD_SHA256: return &INFO_SHA256;
    default: return nullptr;
  }
}

void mbedtls_md_init(mbedtls_md_context_t *ctx) {
  memset(ctx, 0, sizeof *ctx);
}

void mbedtls_md_free(mbedtls_md_context_t *ctx) {
  if (!ctx) return;
  if (ctx->md_ctx) EVP_MD_CTX_free((EVP_MD_CTX *)ctx->md_ctx);
  if (ctx->hmac_ctx) EVP_MAC_CTX_free((EVP_MAC_CTX *)ctx->hmac_ctx);
  memset(ctx, 0, sizeof *ctx);
}

int mbedtls_md_setup(mbedtls_md_context_t *ctx, const mbedtls_md_info_t *md_info, int hmac) {
  if (!ctx || !md_info) return MBEDTLS_ERR_MD_BAD_INPUT_DATA;
  ctx->md_info = md_info;
  ctx->md_ctx = EVP_MD_CTX_new();
  if (!ctx->md_ctx) return MBEDTLS_ERR_MD_ALLOC_FAILED;
  if (hmac) {
    EVP_MAC *mac = EVP_MAC_fetch(nullptr, "HMAC", nullptr);
    if (!mac) return MBEDTLS_ERR_MD_ALLOC_FAILED;
    ctx->hmac_ctx = EVP_MAC_CTX_new(mac);
    EVP_MAC_free(mac);
    if (!ctx->hmac_ctx) return MBEDTLS_ERR_MD_ALLOC_FAILED;
  }
  return 0;
}

int mbedtls_md_hmac_starts(mbedtls_md_context_t *ctx, const unsigned char *key, size_t keylen) {
  if (!ctx || !ctx->md_info || !ctx->hmac_ctx) return MBEDTLS_ERR_MD_BAD_INPUT_DATA;
  OSSL_PARAM params[2];
  params[0] = OSSL_PARAM_construct_utf8_string(OSSL_MAC_PARAM_DIGEST, (char *)ctx->md_info->name, 0);
  params[1] = OSSL_PARAM_construct_end();
  static const unsigned char empty = 0;
  if (!EVP_MAC_init((EVP_MAC_CTX *)ctx->hmac_ctx, keylen ? key : &empty, keylen, params)) return MBEDTLS_ERR_MD_BAD_INPUT_DATA;
  return 0;
}

int mbedtls_md_hmac_update(mbedtls_md_context_t *ctx, const unsigned char *input, size_t ilen) {
  if (!ctx || !ctx->md_info || !ctx->hmac_ctx) return MBEDTLS_ERR_MD_BAD_INPUT_DATA;
  if (ilen && !EVP_MAC_update((EVP_MAC_CTX *)ctx->hmac_ctx, input, ilen)) return MBEDTLS_ERR_MD_BAD_INPUT_DATA;
  return 0;
}

int mbedtls_md_hmac_finish(mbedtls_md_context_t *ctx, unsigned char *output) {
  if (!ctx || !ctx->md_info || !ctx->hmac_ctx) return MBEDTLS_ERR_MD_BAD_INPUT_DATA;
  size_t outl = 0;
  if (!EVP_MAC_final((EVP_MAC_CTX *)ctx->hmac_ctx, output, &outl, ctx->md_info->size)) return MBEDTLS_ERR_MD_BAD_INPUT_DATA;
  return 0;
}

// ── sha1 ────────────────────────────────────────────────────────────────────

int mbedtls_sha1(const unsigned char *input, size_t ilen, unsigned char output[20]) {
  unsigned int n = 0;
  return EVP_Digest(input, ilen, output, &n, EVP_sha1(), nullptr) ? 0 : MBEDTLS_ERR_MD_BAD_INPUT_DATA;
}

// ── base64 ──────────────────────────────────────────────────────────────────

int mbedtls_base64_encode(unsigned char *dst, size_t dlen, size_t *olen, const unsigned char *src, size_t slen) {
  static const char *B = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  if (slen == 0) {
    *olen = 0;
    return 0;
  }
  size_t n = (slen + 2) / 3 * 4;
  if (dlen < n + 1 || !dst) {
    *olen = n + 1;
    return MBEDTLS_ERR_BASE64_BUFFER_TOO_SMALL;
  }
  size_t o = 0;
  for (size_t i = 0; i < slen; i += 3) {
    uint32_t v = (uint32_t)src[i] << 16;
    if (i + 1 < slen) v |= (uint32_t)src[i + 1] << 8;
    if (i + 2 < slen) v |= src[i + 2];
    dst[o++] = B[(v >> 18) & 63];
    dst[o++] = B[(v >> 12) & 63];
    dst[o++] = i + 1 < slen ? B[(v >> 6) & 63] : '=';
    dst[o++] = i + 2 < slen ? B[v & 63] : '=';
  }
  dst[o] = 0;
  *olen = o;
  return 0;
}

// ── gcm ─────────────────────────────────────────────────────────────────────

void mbedtls_gcm_init(mbedtls_gcm_context *ctx) {
  memset(ctx, 0, sizeof *ctx);
}

void mbedtls_gcm_free(mbedtls_gcm_context *ctx) {
  if (ctx) OPENSSL_cleanse(ctx, sizeof *ctx);
}

int mbedtls_gcm_setkey(mbedtls_gcm_context *ctx, mbedtls_cipher_id_t cipher, const unsigned char *key, unsigned int keybits) {
  if (!ctx || !key) return MBEDTLS_ERR_GCM_BAD_INPUT;
  if (cipher != MBEDTLS_CIPHER_ID_AES) return MBEDTLS_ERR_GCM_BAD_INPUT;
  if (keybits != 128 && keybits != 192 && keybits != 256) return MBEDTLS_ERR_GCM_BAD_INPUT;
  memcpy(ctx->key, key, keybits / 8);
  ctx->keybits = keybits;
  return 0;
}

int mbedtls_gcm_auth_decrypt(mbedtls_gcm_context *ctx, size_t length, const unsigned char *iv, size_t iv_len,
                             const unsigned char *add, size_t add_len, const unsigned char *tag, size_t tag_len,
                             const unsigned char *input, unsigned char *output) {
  if (!ctx || !ctx->keybits || iv_len == 0 || tag_len < 4 || tag_len > 16) return MBEDTLS_ERR_GCM_BAD_INPUT;
  const EVP_CIPHER *c = ctx->keybits == 128 ? EVP_aes_128_gcm() : ctx->keybits == 192 ? EVP_aes_192_gcm() : EVP_aes_256_gcm();
  EVP_CIPHER_CTX *e = EVP_CIPHER_CTX_new();
  if (!e) return MBEDTLS_ERR_GCM_BAD_INPUT;
  int ok = EVP_DecryptInit_ex(e, c, nullptr, nullptr, nullptr) &&
           EVP_CIPHER_CTX_ctrl(e, EVP_CTRL_GCM_SET_IVLEN, (int)iv_len, nullptr) &&
           EVP_DecryptInit_ex(e, nullptr, nullptr, ctx->key, iv);
  int outl = 0;
  if (ok && add_len) ok = EVP_DecryptUpdate(e, nullptr, &outl, add, (int)add_len);
  if (ok && length) ok = EVP_DecryptUpdate(e, output, &outl, input, (int)length);
  if (ok) ok = EVP_CIPHER_CTX_ctrl(e, EVP_CTRL_GCM_SET_TAG, (int)tag_len, (void *)tag);
  unsigned char fin[16];
  if (ok) ok = EVP_DecryptFinal_ex(e, fin, &outl);
  EVP_CIPHER_CTX_free(e);
  if (!ok) {
    if (length) OPENSSL_cleanse(output, length);  // mbedTLS zeroes the output on a bad tag
    return MBEDTLS_ERR_GCM_AUTH_FAILED;
  }
  return 0;
}

}  // extern "C"
