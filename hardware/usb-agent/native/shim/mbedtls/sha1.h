// Native board: mbedtls/sha1.h (one-shot only) over OpenSSL 3.
#pragma once

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

int mbedtls_sha1(const unsigned char *input,
                 size_t ilen,
                 unsigned char output[20]);

#ifdef __cplusplus
}
#endif
