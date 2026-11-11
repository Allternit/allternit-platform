// Native board: mbedtls/md.h over OpenSSL 3 (decision 4). Written from the
// signatures of the mbedTLS headers in esp32s3-libs 3.3.12, not from the
// firmware's calls. HMAC and plain digests for SHA-1 and SHA-256; other
// types have no info (mbedtls_md_info_from_type returns NULL).
#pragma once

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MBEDTLS_ERR_MD_FEATURE_UNAVAILABLE -0x5080
#define MBEDTLS_ERR_MD_BAD_INPUT_DATA -0x5100
#define MBEDTLS_ERR_MD_ALLOC_FAILED -0x5180

typedef enum {
    MBEDTLS_MD_NONE=0,
    MBEDTLS_MD_MD5=0x03,
    MBEDTLS_MD_RIPEMD160=0x04,
    MBEDTLS_MD_SHA1=0x05,
    MBEDTLS_MD_SHA224=0x08,
    MBEDTLS_MD_SHA256=0x09,
    MBEDTLS_MD_SHA384=0x0a,
    MBEDTLS_MD_SHA512=0x0b,
} mbedtls_md_type_t;

typedef struct mbedtls_md_info_t mbedtls_md_info_t;

typedef struct mbedtls_md_context_t {
    const mbedtls_md_info_t *md_info;
    void *md_ctx;    // OpenSSL EVP_MD_CTX
    void *hmac_ctx;  // OpenSSL EVP_MAC_CTX
} mbedtls_md_context_t;

const mbedtls_md_info_t *mbedtls_md_info_from_type(mbedtls_md_type_t md_type);
void mbedtls_md_init(mbedtls_md_context_t *ctx);
void mbedtls_md_free(mbedtls_md_context_t *ctx);
int mbedtls_md_setup(mbedtls_md_context_t *ctx, const mbedtls_md_info_t *md_info, int hmac);
int mbedtls_md_hmac_starts(mbedtls_md_context_t *ctx, const unsigned char *key,
                           size_t keylen);
int mbedtls_md_hmac_update(mbedtls_md_context_t *ctx, const unsigned char *input,
                           size_t ilen);
int mbedtls_md_hmac_finish(mbedtls_md_context_t *ctx, unsigned char *output);

#ifdef __cplusplus
}
#endif
