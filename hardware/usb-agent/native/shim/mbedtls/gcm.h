// Native board: mbedtls/gcm.h over OpenSSL 3 (decision 4). Written from the
// mbedTLS signatures in esp32s3-libs 3.3.12. AES keys of 128, 192 or 256
// bits; auth_decrypt zeroes the output when the tag does not match, as
// mbedTLS does.
#pragma once

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MBEDTLS_GCM_ENCRYPT     1
#define MBEDTLS_GCM_DECRYPT     0
#define MBEDTLS_ERR_GCM_AUTH_FAILED -0x0012
#define MBEDTLS_ERR_GCM_BAD_INPUT -0x0014

typedef enum {
    MBEDTLS_CIPHER_ID_NONE = 0,
    MBEDTLS_CIPHER_ID_NULL,
    MBEDTLS_CIPHER_ID_AES,
    MBEDTLS_CIPHER_ID_DES,
    MBEDTLS_CIPHER_ID_3DES,
    MBEDTLS_CIPHER_ID_CAMELLIA,
    MBEDTLS_CIPHER_ID_ARIA,
    MBEDTLS_CIPHER_ID_CHACHA20,
} mbedtls_cipher_id_t;

typedef struct mbedtls_gcm_context {
    unsigned char key[32];
    unsigned int keybits;  // 0 = no key set
} mbedtls_gcm_context;

void mbedtls_gcm_init(mbedtls_gcm_context *ctx);
int mbedtls_gcm_setkey(mbedtls_gcm_context *ctx,
                       mbedtls_cipher_id_t cipher,
                       const unsigned char *key,
                       unsigned int keybits);
int mbedtls_gcm_auth_decrypt(mbedtls_gcm_context *ctx,
                             size_t length,
                             const unsigned char *iv,
                             size_t iv_len,
                             const unsigned char *add,
                             size_t add_len,
                             const unsigned char *tag,
                             size_t tag_len,
                             const unsigned char *input,
                             unsigned char *output);
void mbedtls_gcm_free(mbedtls_gcm_context *ctx);

#ifdef __cplusplus
}
#endif
