// Native board: mbedtls/base64.h (encode only), written from the mbedTLS
// signature and documented behaviour: on a short buffer it sets *olen to
// the size needed (with the terminating NUL) and returns
// MBEDTLS_ERR_BASE64_BUFFER_TOO_SMALL; on success it writes a NUL after the
// text and *olen excludes it.
#pragma once

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MBEDTLS_ERR_BASE64_BUFFER_TOO_SMALL               -0x002A

int mbedtls_base64_encode(unsigned char *dst, size_t dlen, size_t *olen,
                          const unsigned char *src, size_t slen);

#ifdef __cplusplus
}
#endif
