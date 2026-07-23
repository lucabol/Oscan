/*
 * Hosted TLS runtime member.
 *
 * Native hosted archives compile osc_runtime.c in libc mode, where the generic
 * runtime keeps socket support but deliberately leaves TLS to this separate
 * translation unit. Windows uses Schannel with the OS trust store; Linux uses
 * the same bundled BearSSL dependency as freestanding, but with BearSSL's
 * minimal verifier wired to OS trust anchors instead of the freestanding
 * no-verify shim.
 */

#if defined(__linux__) && !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif

#include "osc_runtime.h"

static void osc_tls_host_to_cstr(osc_str host, char *buf, size_t bufsz)
{
    size_t n;
    if (bufsz == 0) return;
    n = host.len < 0 ? 0u : (size_t)host.len;
    if (n >= bufsz) n = bufsz - 1;
    for (size_t i = 0; i < n && host.data != NULL; i++) {
        buf[i] = host.data[i];
    }
    buf[n] = '\0';
}

#if defined(_WIN32)

#ifndef _WINSOCK_DEPRECATED_NO_WARNINGS
#define _WINSOCK_DEPRECATED_NO_WARNINGS
#endif
#define SECURITY_WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <security.h>
#include <schannel.h>
#include <stdio.h>
#include <string.h>

#ifndef SP_PROT_TLS1_3_CLIENT
#define SP_PROT_TLS1_3_CLIENT 0x00002000
#endif

#ifndef UNISP_NAME_A
#define UNISP_NAME_A "Microsoft Unified Security Protocol Provider"
#endif

#define OSC_TLS_MAX_CONNECTIONS 8

typedef struct {
    SOCKET sock;
    CredHandle cred;
    CtxtHandle ctx;
    int in_use;
    int has_cred;
    int has_ctx;
    int closed;
    char decrypted[65536];
    int dec_len;
    int dec_pos;
    char raw_buf[65536];
    int raw_len;
} osc_tls_conn;

static osc_tls_conn osc_tls_conns[OSC_TLS_MAX_CONNECTIONS];
static int osc_tls_initialized;

static void osc_tls_zero(void *ptr, size_t len)
{
    unsigned char *p = (unsigned char *)ptr;
    for (size_t i = 0; i < len; i++) p[i] = 0;
}

static int osc_tls_init(void)
{
    if (osc_tls_initialized) return 0;
    WSADATA wsa;
    if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) return -1;
    osc_tls_zero(osc_tls_conns, sizeof(osc_tls_conns));
    for (int i = 0; i < OSC_TLS_MAX_CONNECTIONS; i++) {
        osc_tls_conns[i].sock = INVALID_SOCKET;
    }
    osc_tls_initialized = 1;
    return 0;
}

static int osc_tls_find_free_slot(void)
{
    for (int i = 0; i < OSC_TLS_MAX_CONNECTIONS; i++) {
        if (!osc_tls_conns[i].in_use) return i;
    }
    return -1;
}

static int osc_tls_send_all(SOCKET sock, const char *buf, int len)
{
    int off = 0;
    while (off < len) {
        int n = send(sock, buf + off, len - off, 0);
        if (n <= 0) return -1;
        off += n;
    }
    return 0;
}

static SOCKET osc_tls_connect_socket(const char *hostname, int32_t port)
{
    struct addrinfo hints;
    struct addrinfo *result = NULL;
    char port_buf[16];
    SOCKET sock = INVALID_SOCKET;

    if (port <= 0 || port > 65535) return INVALID_SOCKET;
    _snprintf(port_buf, sizeof(port_buf), "%d", (int)port);
    port_buf[sizeof(port_buf) - 1] = '\0';
    osc_tls_zero(&hints, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    if (getaddrinfo(hostname, port_buf, &hints, &result) != 0) return INVALID_SOCKET;
    for (struct addrinfo *it = result; it != NULL; it = it->ai_next) {
        DWORD rcv_ms = 15000;
        DWORD snd_ms = 10000;
        sock = socket(it->ai_family, it->ai_socktype, it->ai_protocol);
        if (sock == INVALID_SOCKET) continue;
        setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, (const char *)&rcv_ms, (int)sizeof(rcv_ms));
        setsockopt(sock, SOL_SOCKET, SO_SNDTIMEO, (const char *)&snd_ms, (int)sizeof(snd_ms));
        if (connect(sock, it->ai_addr, (int)it->ai_addrlen) == 0) break;
        closesocket(sock);
        sock = INVALID_SOCKET;
    }
    freeaddrinfo(result);
    return sock;
}

static int osc_tls_do_handshake(int slot, const char *hostname)
{
    osc_tls_conn *c = &osc_tls_conns[slot];
    SECURITY_STATUS ss;
    SCHANNEL_CRED sc_cred;
    SecBuffer out_buf = { 0, SECBUFFER_TOKEN, NULL };
    SecBufferDesc out_desc = { SECBUFFER_VERSION, 1, &out_buf };
    DWORD ctx_flags = ISC_REQ_ALLOCATE_MEMORY | ISC_REQ_CONFIDENTIALITY |
                      ISC_REQ_REPLAY_DETECT | ISC_REQ_SEQUENCE_DETECT |
                      ISC_REQ_STREAM;
    DWORD out_flags = 0;
    char hs_buf[65536];
    int hs_len = 0;

    osc_tls_zero(&sc_cred, sizeof(sc_cred));
    sc_cred.dwVersion = SCHANNEL_CRED_VERSION;
    sc_cred.dwFlags = SCH_CRED_AUTO_CRED_VALIDATION |
                      SCH_CRED_NO_DEFAULT_CREDS |
                      SCH_USE_STRONG_CRYPTO;
    sc_cred.grbitEnabledProtocols = SP_PROT_TLS1_2_CLIENT | SP_PROT_TLS1_3_CLIENT;
    ss = AcquireCredentialsHandleA(NULL, (SEC_CHAR *)UNISP_NAME_A,
                                   SECPKG_CRED_OUTBOUND, NULL, &sc_cred,
                                   NULL, NULL, &c->cred, NULL);
    if (ss != SEC_E_OK) return -1;
    c->has_cred = 1;

    ss = InitializeSecurityContextA(&c->cred, NULL, (SEC_CHAR *)hostname,
                                    ctx_flags, 0, 0, NULL, 0,
                                    &c->ctx, &out_desc, &out_flags, NULL);
    c->has_ctx = 1;
    if (ss != SEC_I_CONTINUE_NEEDED && ss != SEC_E_OK) return -1;
    if (out_buf.cbBuffer > 0 && out_buf.pvBuffer) {
        int send_ok = osc_tls_send_all(c->sock, (char *)out_buf.pvBuffer, (int)out_buf.cbBuffer);
        FreeContextBuffer(out_buf.pvBuffer);
        if (send_ok != 0) return -1;
    }

    while (ss == SEC_I_CONTINUE_NEEDED || ss == SEC_E_INCOMPLETE_MESSAGE) {
        SecBuffer in_bufs[2];
        SecBufferDesc in_desc;
        SecBuffer out_buf2 = { 0, SECBUFFER_TOKEN, NULL };
        SecBufferDesc out_desc2 = { SECBUFFER_VERSION, 1, &out_buf2 };

        if ((size_t)hs_len >= sizeof(hs_buf)) return -1;
        int r = recv(c->sock, hs_buf + hs_len, (int)(sizeof(hs_buf) - (size_t)hs_len), 0);
        if (r <= 0) return -1;
        hs_len += r;

        in_bufs[0].BufferType = SECBUFFER_TOKEN;
        in_bufs[0].cbBuffer = (unsigned long)hs_len;
        in_bufs[0].pvBuffer = hs_buf;
        in_bufs[1].BufferType = SECBUFFER_EMPTY;
        in_bufs[1].cbBuffer = 0;
        in_bufs[1].pvBuffer = NULL;
        in_desc.ulVersion = SECBUFFER_VERSION;
        in_desc.cBuffers = 2;
        in_desc.pBuffers = in_bufs;
        ss = InitializeSecurityContextA(&c->cred, &c->ctx, (SEC_CHAR *)hostname,
                                        ctx_flags, 0, 0, &in_desc, 0,
                                        NULL, &out_desc2, &out_flags, NULL);
        if (ss == SEC_E_OK || ss == SEC_I_CONTINUE_NEEDED) {
            if (out_buf2.cbBuffer > 0 && out_buf2.pvBuffer) {
                int send_ok = osc_tls_send_all(c->sock, (char *)out_buf2.pvBuffer, (int)out_buf2.cbBuffer);
                FreeContextBuffer(out_buf2.pvBuffer);
                if (send_ok != 0) return -1;
            }
            if (in_bufs[1].BufferType == SECBUFFER_EXTRA && in_bufs[1].cbBuffer > 0) {
                memmove(hs_buf, hs_buf + (hs_len - (int)in_bufs[1].cbBuffer), (size_t)in_bufs[1].cbBuffer);
                hs_len = (int)in_bufs[1].cbBuffer;
            } else {
                hs_len = 0;
            }
        } else if (ss != SEC_E_INCOMPLETE_MESSAGE) {
            if (out_buf2.pvBuffer) FreeContextBuffer(out_buf2.pvBuffer);
            return -1;
        }
    }
    if (hs_len > 0) {
        memcpy(c->raw_buf, hs_buf, (size_t)hs_len);
        c->raw_len = hs_len;
    }
    return 0;
}

static int osc_tls_decrypt_data(osc_tls_conn *c)
{
    SecBuffer bufs[4];
    SecBufferDesc desc;
    SECURITY_STATUS ss;
    char *extra = NULL;
    int extra_len = 0;

    if (c->raw_len == 0) return 0;
    bufs[0].BufferType = SECBUFFER_DATA;
    bufs[0].cbBuffer = (unsigned long)c->raw_len;
    bufs[0].pvBuffer = c->raw_buf;
    for (int i = 1; i < 4; i++) {
        bufs[i].BufferType = SECBUFFER_EMPTY;
        bufs[i].cbBuffer = 0;
        bufs[i].pvBuffer = NULL;
    }
    desc.ulVersion = SECBUFFER_VERSION;
    desc.cBuffers = 4;
    desc.pBuffers = bufs;
    ss = DecryptMessage(&c->ctx, &desc, 0, NULL);
    if (ss == SEC_E_INCOMPLETE_MESSAGE) return 0;
    if (ss != SEC_E_OK && ss != SEC_I_CONTEXT_EXPIRED) return -1;
    if (ss == SEC_I_CONTEXT_EXPIRED) {
        c->closed = 1;
        c->raw_len = 0;
        return 1;
    }

    for (int i = 0; i < 4; i++) {
        if (bufs[i].BufferType == SECBUFFER_EXTRA && bufs[i].cbBuffer > 0) {
            extra = (char *)bufs[i].pvBuffer;
            extra_len = (int)bufs[i].cbBuffer;
        }
    }
    for (int i = 0; i < 4; i++) {
        if (bufs[i].BufferType == SECBUFFER_DATA && bufs[i].cbBuffer > 0) {
            char *data = (char *)bufs[i].pvBuffer;
            int copy = (int)bufs[i].cbBuffer;
            int space = (int)sizeof(c->decrypted) - c->dec_len;
            if (extra && data < extra && data + copy > extra) {
                copy = (int)(extra - data);
            }
            if (copy > space) copy = space;
            if (copy > 0) {
                memcpy(c->decrypted + c->dec_len, data, (size_t)copy);
                c->dec_len += copy;
            }
        }
    }
    c->raw_len = 0;
    if (extra && extra_len > 0) {
        memmove(c->raw_buf, extra, (size_t)extra_len);
        c->raw_len = extra_len;
    }
    return 1;
}

osc_result_i32_str osc_tls_connect(osc_str host, int32_t port)
{
    osc_result_i32_str result;
    char hostname[256];
    int slot;
    SOCKET sock;
    osc_tls_conn *c;

    osc_tls_host_to_cstr(host, hostname, sizeof(hostname));
    if (osc_tls_init() != 0 || hostname[0] == '\0') {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: TLS subsystem initialization failed");
        return result;
    }
    slot = osc_tls_find_free_slot();
    if (slot < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: too many open TLS connections");
        return result;
    }
    sock = osc_tls_connect_socket(hostname, port);
    if (sock == INVALID_SOCKET) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: TCP connection failed");
        return result;
    }
    c = &osc_tls_conns[slot];
    osc_tls_zero(c, sizeof(*c));
    c->sock = sock;
    if (osc_tls_do_handshake(slot, hostname) != 0) {
        closesocket(c->sock);
        if (c->has_ctx) DeleteSecurityContext(&c->ctx);
        if (c->has_cred) FreeCredentialsHandle(&c->cred);
        osc_tls_zero(c, sizeof(*c));
        c->sock = INVALID_SOCKET;
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: TLS handshake or certificate verification failed");
        return result;
    }
    c->in_use = 1;
    result.is_ok = 1;
    result.value.ok = (int32_t)slot;
    return result;
}

osc_result_i32_str osc_tls_send(int32_t handle, osc_str data)
{
    osc_result_i32_str result;
    osc_tls_conn *c;
    SecPkgContext_StreamSizes sizes;
    const char *src = data.data;
    int total_sent = 0;

    if (handle < 0 || handle >= OSC_TLS_MAX_CONNECTIONS || !osc_tls_conns[handle].in_use) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_send: invalid TLS handle");
        return result;
    }
    c = &osc_tls_conns[handle];
    if (QueryContextAttributes(&c->ctx, SECPKG_ATTR_STREAM_SIZES, &sizes) != SEC_E_OK) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_send: send failed");
        return result;
    }
    while (total_sent < data.len) {
        int chunk = data.len - total_sent;
        int buf_size;
        char *msg_buf;
        SecBuffer bufs[4];
        SecBufferDesc desc;
        SECURITY_STATUS ss;
        int enc_len;

        if ((DWORD)chunk > sizes.cbMaximumMessage) chunk = (int)sizes.cbMaximumMessage;
        buf_size = (int)(sizes.cbHeader + (DWORD)chunk + sizes.cbTrailer);
        msg_buf = (char *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)buf_size);
        if (!msg_buf) {
            result.is_ok = 0;
            result.value.err = osc_str_from_cstr("tls_send: out of memory");
            return result;
        }
        memcpy(msg_buf + sizes.cbHeader, src + total_sent, (size_t)chunk);
        bufs[0].BufferType = SECBUFFER_STREAM_HEADER;
        bufs[0].cbBuffer = sizes.cbHeader;
        bufs[0].pvBuffer = msg_buf;
        bufs[1].BufferType = SECBUFFER_DATA;
        bufs[1].cbBuffer = (unsigned long)chunk;
        bufs[1].pvBuffer = msg_buf + sizes.cbHeader;
        bufs[2].BufferType = SECBUFFER_STREAM_TRAILER;
        bufs[2].cbBuffer = sizes.cbTrailer;
        bufs[2].pvBuffer = msg_buf + sizes.cbHeader + chunk;
        bufs[3].BufferType = SECBUFFER_EMPTY;
        bufs[3].cbBuffer = 0;
        bufs[3].pvBuffer = NULL;
        desc.ulVersion = SECBUFFER_VERSION;
        desc.cBuffers = 4;
        desc.pBuffers = bufs;
        ss = EncryptMessage(&c->ctx, 0, &desc, 0);
        if (ss != SEC_E_OK) {
            HeapFree(GetProcessHeap(), 0, msg_buf);
            result.is_ok = 0;
            result.value.err = osc_str_from_cstr("tls_send: encrypt failed");
            return result;
        }
        enc_len = (int)(bufs[0].cbBuffer + bufs[1].cbBuffer + bufs[2].cbBuffer);
        if (osc_tls_send_all(c->sock, msg_buf, enc_len) != 0) {
            HeapFree(GetProcessHeap(), 0, msg_buf);
            result.is_ok = 0;
            result.value.err = osc_str_from_cstr("tls_send: send failed");
            return result;
        }
        HeapFree(GetProcessHeap(), 0, msg_buf);
        total_sent += chunk;
    }
    result.is_ok = 1;
    result.value.ok = data.len;
    return result;
}

osc_str osc_tls_recv(osc_arena *arena, int32_t handle, int32_t max_len)
{
    osc_str result;
    char *buf;
    int n;

    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf || handle < 0 || handle >= OSC_TLS_MAX_CONNECTIONS || !osc_tls_conns[handle].in_use) {
        result.data = "";
        result.len = 0;
        return result;
    }
    osc_tls_conn *c = &osc_tls_conns[handle];
    if (c->dec_pos < c->dec_len) {
        int avail = c->dec_len - c->dec_pos;
        int copy = avail < max_len ? avail : max_len;
        memcpy(buf, c->decrypted + c->dec_pos, (size_t)copy);
        c->dec_pos += copy;
        if (c->dec_pos >= c->dec_len) {
            c->dec_pos = 0;
            c->dec_len = 0;
        }
        result.data = buf;
        result.len = (int32_t)copy;
        return result;
    }
    c->dec_pos = 0;
    c->dec_len = 0;
    if (c->closed) {
        result.data = "";
        result.len = 0;
        return result;
    }
    n = osc_tls_decrypt_data(c);
    if (n > 0 && c->dec_len > 0) {
        int copy = c->dec_len < max_len ? c->dec_len : max_len;
        memcpy(buf, c->decrypted, (size_t)copy);
        c->dec_pos = copy;
        if (c->dec_pos >= c->dec_len) {
            c->dec_pos = 0;
            c->dec_len = 0;
        }
        result.data = buf;
        result.len = (int32_t)copy;
        return result;
    }
    if (n < 0 || c->closed) {
        result.data = "";
        result.len = 0;
        return result;
    }
    for (int attempts = 0; attempts < 100; attempts++) {
        int space = (int)sizeof(c->raw_buf) - c->raw_len;
        int r;
        if (space <= 0) break;
        r = recv(c->sock, c->raw_buf + c->raw_len, space, 0);
        if (r <= 0) break;
        c->raw_len += r;
        n = osc_tls_decrypt_data(c);
        if (n > 0 && c->dec_len > 0) {
            int copy = c->dec_len < max_len ? c->dec_len : max_len;
            memcpy(buf, c->decrypted, (size_t)copy);
            c->dec_pos = copy;
            if (c->dec_pos >= c->dec_len) {
                c->dec_pos = 0;
                c->dec_len = 0;
            }
            result.data = buf;
            result.len = (int32_t)copy;
            return result;
        }
        if (n < 0 || c->closed) break;
    }
    result.data = "";
    result.len = 0;
    return result;
}

int32_t osc_tls_recv_byte(int32_t handle)
{
    char one;
    osc_arena *arena = osc_arena_create(16);
    osc_str s;
    if (!arena) return -1;
    s = osc_tls_recv(arena, handle, 1);
    if (s.len == 1) one = s.data[0]; else one = (char)-1;
    osc_arena_destroy(arena);
    return s.len == 1 ? (int32_t)(unsigned char)one : -1;
}

void osc_tls_close(int32_t handle)
{
    osc_tls_conn *c;
    DWORD shutdown_token = SCHANNEL_SHUTDOWN;
    SecBuffer shut_buf = { sizeof(shutdown_token), SECBUFFER_TOKEN, &shutdown_token };
    SecBufferDesc shut_desc = { SECBUFFER_VERSION, 1, &shut_buf };
    SecBuffer out_buf = { 0, SECBUFFER_TOKEN, NULL };
    SecBufferDesc out_desc = { SECBUFFER_VERSION, 1, &out_buf };
    DWORD flags = ISC_REQ_ALLOCATE_MEMORY | ISC_REQ_STREAM;
    DWORD out_flags = 0;

    if (handle < 0 || handle >= OSC_TLS_MAX_CONNECTIONS || !osc_tls_conns[handle].in_use) return;
    c = &osc_tls_conns[handle];
    ApplyControlToken(&c->ctx, &shut_desc);
    InitializeSecurityContextA(&c->cred, &c->ctx, NULL, flags, 0, 0,
                               &shut_desc, 0, NULL, &out_desc, &out_flags, NULL);
    if (out_buf.cbBuffer > 0 && out_buf.pvBuffer) {
        (void)osc_tls_send_all(c->sock, (char *)out_buf.pvBuffer, (int)out_buf.cbBuffer);
        FreeContextBuffer(out_buf.pvBuffer);
    }
    if (c->has_ctx) DeleteSecurityContext(&c->ctx);
    if (c->has_cred) FreeCredentialsHandle(&c->cred);
    closesocket(c->sock);
    osc_tls_zero(c, sizeof(*c));
    c->sock = INVALID_SOCKET;
}

void osc_tls_cleanup(void)
{
    for (int i = 0; i < OSC_TLS_MAX_CONNECTIONS; i++) {
        if (osc_tls_conns[i].in_use) osc_tls_close(i);
    }
    osc_tls_initialized = 0;
    WSACleanup();
}

#elif defined(__linux__)

#include <errno.h>
#include <limits.h>
#include <fcntl.h>
#include <netdb.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#include "bearssl/inc/bearssl.h"

#ifndef MSG_NOSIGNAL
#define MSG_NOSIGNAL 0
#endif

#define OSC_TLS_MAX_CONNECTIONS 8
#define OSC_TLS_MAX_CA_BUNDLE_SIZE (16u * 1024u * 1024u)

typedef struct {
    br_ssl_client_context sc;
    br_x509_minimal_context xc;
    br_sslio_context ioc;
    unsigned char iobuf[BR_SSL_BUFSIZE_BIDI];
    int sock;
    int in_use;
} osc_tls_conn;

typedef struct {
    unsigned char *data;
    size_t len;
    int failed;
} osc_tls_dn_buf;

static osc_tls_conn osc_tls_conns[OSC_TLS_MAX_CONNECTIONS];
static int osc_tls_initialized;
static br_x509_trust_anchor *osc_tls_anchors;
static size_t osc_tls_anchor_count;
static char osc_tls_last_error[192] = "tls_connect: connection failed";

static void osc_tls_set_error(const char *message)
{
    size_t n = strlen(message);
    if (n >= sizeof(osc_tls_last_error)) n = sizeof(osc_tls_last_error) - 1;
    memcpy(osc_tls_last_error, message, n);
    osc_tls_last_error[n] = '\0';
}

static int osc_tls_find_free_slot(void)
{
    for (int i = 0; i < OSC_TLS_MAX_CONNECTIONS; i++) {
        if (!osc_tls_conns[i].in_use) return i;
    }
    return -1;
}

static void osc_tls_free_anchor(br_x509_trust_anchor *ta)
{
    if (!ta) return;
    free(ta->dn.data);
    if (ta->pkey.key_type == BR_KEYTYPE_RSA) {
        free(ta->pkey.key.rsa.n);
        free(ta->pkey.key.rsa.e);
    } else if (ta->pkey.key_type == BR_KEYTYPE_EC) {
        free(ta->pkey.key.ec.q);
    }
    memset(ta, 0, sizeof(*ta));
}

static void osc_tls_free_anchors(void)
{
    for (size_t i = 0; i < osc_tls_anchor_count; i++) {
        osc_tls_free_anchor(&osc_tls_anchors[i]);
    }
    free(osc_tls_anchors);
    osc_tls_anchors = NULL;
    osc_tls_anchor_count = 0;
}

static void osc_tls_dn_append(void *ctx, const void *buf, size_t len)
{
    osc_tls_dn_buf *dn = (osc_tls_dn_buf *)ctx;
    unsigned char *next;

    if (dn->failed) return;
    if (len > SIZE_MAX - dn->len) {
        dn->failed = 1;
        return;
    }
    next = (unsigned char *)realloc(dn->data, dn->len + len);
    if (!next) {
        dn->failed = 1;
        return;
    }
    memcpy(next + dn->len, buf, len);
    dn->data = next;
    dn->len += len;
}

static unsigned char *osc_tls_memdup(const void *src, size_t len)
{
    unsigned char *dst = (unsigned char *)malloc(len == 0 ? 1 : len);
    if (!dst) return NULL;
    if (len > 0) memcpy(dst, src, len);
    return dst;
}

static int osc_tls_anchor_from_der(const unsigned char *der, size_t der_len, br_x509_trust_anchor *out)
{
    br_x509_decoder_context dc;
    osc_tls_dn_buf dn = { 0 };
    br_x509_pkey *pk;

    memset(out, 0, sizeof(*out));
    br_x509_decoder_init(&dc, osc_tls_dn_append, &dn);
    br_x509_decoder_push(&dc, der, der_len);
    pk = br_x509_decoder_get_pkey(&dc);
    if (!pk || dn.failed || !br_x509_decoder_isCA(&dc)) {
        free(dn.data);
        return 0;
    }

    out->dn.data = dn.data;
    out->dn.len = dn.len;
    out->flags = BR_X509_TA_CA;
    out->pkey.key_type = pk->key_type;
    if (pk->key_type == BR_KEYTYPE_RSA) {
        out->pkey.key.rsa.n = osc_tls_memdup(pk->key.rsa.n, pk->key.rsa.nlen);
        out->pkey.key.rsa.e = osc_tls_memdup(pk->key.rsa.e, pk->key.rsa.elen);
        out->pkey.key.rsa.nlen = pk->key.rsa.nlen;
        out->pkey.key.rsa.elen = pk->key.rsa.elen;
        if (!out->pkey.key.rsa.n || !out->pkey.key.rsa.e) {
            osc_tls_free_anchor(out);
            return 0;
        }
    } else if (pk->key_type == BR_KEYTYPE_EC) {
        out->pkey.key.ec.curve = pk->key.ec.curve;
        out->pkey.key.ec.q = osc_tls_memdup(pk->key.ec.q, pk->key.ec.qlen);
        out->pkey.key.ec.qlen = pk->key.ec.qlen;
        if (!out->pkey.key.ec.q) {
            osc_tls_free_anchor(out);
            return 0;
        }
    } else {
        osc_tls_free_anchor(out);
        return 0;
    }
    return 1;
}

static int osc_tls_add_anchor(const unsigned char *der, size_t der_len)
{
    br_x509_trust_anchor ta;
    br_x509_trust_anchor *next;

    if (!osc_tls_anchor_from_der(der, der_len, &ta)) return 0;
    next = (br_x509_trust_anchor *)realloc(
        osc_tls_anchors,
        (osc_tls_anchor_count + 1) * sizeof(*osc_tls_anchors));
    if (!next) {
        osc_tls_free_anchor(&ta);
        return -1;
    }
    osc_tls_anchors = next;
    osc_tls_anchors[osc_tls_anchor_count++] = ta;
    return 1;
}

static int osc_tls_b64_value(unsigned char c)
{
    if (c >= 'A' && c <= 'Z') return (int)(c - 'A');
    if (c >= 'a' && c <= 'z') return (int)(c - 'a') + 26;
    if (c >= '0' && c <= '9') return (int)(c - '0') + 52;
    if (c == '+') return 62;
    if (c == '/') return 63;
    return -1;
}

static unsigned char *osc_tls_decode_base64(const char *src, size_t len, size_t *out_len)
{
    unsigned char *out = (unsigned char *)malloc((len / 4u) * 3u + 3u);
    int val = 0;
    int valb = -8;
    size_t n = 0;

    if (!out) return NULL;
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)src[i];
        int d;
        if (c == '=') break;
        if (c == '\r' || c == '\n' || c == '\t' || c == ' ') continue;
        d = osc_tls_b64_value(c);
        if (d < 0) {
            free(out);
            return NULL;
        }
        val = (val << 6) | d;
        valb += 6;
        if (valb >= 0) {
            out[n++] = (unsigned char)((val >> valb) & 0xFF);
            valb -= 8;
        }
    }
    *out_len = n;
    return out;
}

static unsigned char *osc_tls_read_file(const char *path, size_t *out_len)
{
    FILE *f = fopen(path, "rb");
    unsigned char *buf;
    long len;

    if (!f) return NULL;
    if (fseek(f, 0, SEEK_END) != 0) {
        fclose(f);
        return NULL;
    }
    len = ftell(f);
    if (len < 0 || (unsigned long)len > OSC_TLS_MAX_CA_BUNDLE_SIZE) {
        fclose(f);
        return NULL;
    }
    if (fseek(f, 0, SEEK_SET) != 0) {
        fclose(f);
        return NULL;
    }
    buf = (unsigned char *)malloc((size_t)len + 1u);
    if (!buf) {
        fclose(f);
        return NULL;
    }
    if (fread(buf, 1, (size_t)len, f) != (size_t)len) {
        free(buf);
        fclose(f);
        return NULL;
    }
    fclose(f);
    buf[len] = 0;
    *out_len = (size_t)len;
    return buf;
}

static int osc_tls_load_anchors_from_file(const char *path)
{
    static const char begin_marker[] = "-----BEGIN CERTIFICATE-----";
    static const char end_marker[] = "-----END CERTIFICATE-----";
    size_t len;
    unsigned char *file = osc_tls_read_file(path, &len);
    char *cursor;
    int saw_pem = 0;
    size_t before = osc_tls_anchor_count;

    if (!file) return 0;
    cursor = (char *)file;
    for (;;) {
        char *begin = strstr(cursor, begin_marker);
        char *end;
        unsigned char *der;
        size_t der_len;
        int added;

        if (!begin) break;
        begin += sizeof(begin_marker) - 1;
        end = strstr(begin, end_marker);
        if (!end) break;
        saw_pem = 1;
        der = osc_tls_decode_base64(begin, (size_t)(end - begin), &der_len);
        if (der) {
            added = osc_tls_add_anchor(der, der_len);
            free(der);
            if (added < 0) {
                free(file);
                return -1;
            }
        }
        cursor = end + sizeof(end_marker) - 1;
    }
    if (!saw_pem) {
        int added = osc_tls_add_anchor(file, len);
        if (added < 0) {
            free(file);
            return -1;
        }
    }
    free(file);
    return osc_tls_anchor_count > before;
}

static int osc_tls_load_trust_anchors(void)
{
    static const char *default_paths[] = {
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/ca-bundle.pem",
        "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
        NULL
    };
    const char *override_path = getenv("OSCAN_TLS_CA_BUNDLE");

    if (osc_tls_anchor_count > 0) return 0;
    if (override_path && override_path[0] != '\0') {
        if (osc_tls_load_anchors_from_file(override_path) > 0) return 0;
        osc_tls_set_error("tls_connect: failed to load trust anchors from OSCAN_TLS_CA_BUNDLE");
        return -1;
    }
    for (size_t i = 0; default_paths[i] != NULL; i++) {
        if (osc_tls_load_anchors_from_file(default_paths[i]) > 0) return 0;
    }
    osc_tls_set_error("tls_connect: no system CA bundle found (set OSCAN_TLS_CA_BUNDLE to a PEM CA bundle)");
    return -1;
}

static int osc_tls_read_entropy(void *buf, size_t len)
{
    int fd = open("/dev/urandom", O_RDONLY);
    unsigned char *p = (unsigned char *)buf;
    size_t off = 0;

    if (fd < 0) return -1;
    while (off < len) {
        ssize_t n = read(fd, p + off, len - off);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) {
            close(fd);
            return -1;
        }
        off += (size_t)n;
    }
    close(fd);
    return 0;
}

static int osc_tls_connect_socket(const char *hostname, int32_t port)
{
    struct addrinfo hints;
    struct addrinfo *result = NULL;
    char port_buf[16];
    int fd = -1;

    if (port <= 0 || port > 65535) {
        osc_tls_set_error("tls_connect: invalid port");
        return -1;
    }
    snprintf(port_buf, sizeof(port_buf), "%d", (int)port);
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;

    if (getaddrinfo(hostname, port_buf, &hints, &result) != 0) {
        osc_tls_set_error("tls_connect: cannot resolve host");
        return -1;
    }
    for (struct addrinfo *it = result; it != NULL; it = it->ai_next) {
        struct timeval rcv = { 15, 0 };
        struct timeval snd = { 10, 0 };
        fd = socket(it->ai_family, it->ai_socktype, it->ai_protocol);
        if (fd < 0) continue;
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &rcv, sizeof(rcv));
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &snd, sizeof(snd));
        if (connect(fd, it->ai_addr, it->ai_addrlen) == 0) break;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(result);
    if (fd < 0) osc_tls_set_error("tls_connect: TCP connection failed");
    return fd;
}

static int osc_tls_br_read(void *ctx, unsigned char *buf, size_t len)
{
    int fd = *(int *)ctx;
    for (;;) {
        ssize_t n = recv(fd, buf, len, 0);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return -1;
        if (n > INT_MAX) return INT_MAX;
        return (int)n;
    }
}

static int osc_tls_br_write(void *ctx, const unsigned char *buf, size_t len)
{
    int fd = *(int *)ctx;
    for (;;) {
        ssize_t n = send(fd, buf, len, MSG_NOSIGNAL);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return -1;
        if (n > INT_MAX) return INT_MAX;
        return (int)n;
    }
}

static const char *osc_tls_bearssl_error(int err)
{
    switch (err) {
    case BR_ERR_OK: return "tls_connect: TLS handshake failed";
    case BR_ERR_X509_EXPIRED: return "tls_connect: certificate is expired or not yet valid";
    case BR_ERR_X509_BAD_SERVER_NAME: return "tls_connect: certificate hostname verification failed";
    case BR_ERR_X509_NOT_TRUSTED: return "tls_connect: certificate chain is not trusted";
    case BR_ERR_X509_BAD_SIGNATURE: return "tls_connect: certificate signature verification failed";
    case BR_ERR_X509_TIME_UNKNOWN: return "tls_connect: certificate validation time is unavailable";
    default: return "tls_connect: TLS handshake or certificate verification failed";
    }
}

osc_result_i32_str osc_tls_connect(osc_str host, int32_t port)
{
    osc_result_i32_str result;
    char hostname[256];
    int slot;
    int fd;
    unsigned char seed[32];
    int err;
    osc_tls_conn *c;

    if (!osc_tls_initialized) {
        memset(osc_tls_conns, 0, sizeof(osc_tls_conns));
        for (int i = 0; i < OSC_TLS_MAX_CONNECTIONS; i++) osc_tls_conns[i].sock = -1;
        osc_tls_initialized = 1;
    }
    if (osc_tls_load_trust_anchors() != 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr(osc_tls_last_error);
        return result;
    }

    osc_tls_host_to_cstr(host, hostname, sizeof(hostname));
    if (hostname[0] == '\0') {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: empty host");
        return result;
    }
    slot = osc_tls_find_free_slot();
    if (slot < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: too many open TLS connections");
        return result;
    }
    fd = osc_tls_connect_socket(hostname, port);
    if (fd < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr(osc_tls_last_error);
        return result;
    }
    if (osc_tls_read_entropy(seed, sizeof(seed)) != 0) {
        close(fd);
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: cannot read operating system entropy");
        return result;
    }

    c = &osc_tls_conns[slot];
    memset(c, 0, sizeof(*c));
    c->sock = fd;
    br_ssl_client_init_full(&c->sc, &c->xc, osc_tls_anchors, osc_tls_anchor_count);
    br_ssl_engine_set_versions(&c->sc.eng, BR_TLS12, BR_TLS12);
    br_ssl_engine_set_buffer(&c->sc.eng, c->iobuf, sizeof(c->iobuf), 1);
    br_ssl_engine_inject_entropy(&c->sc.eng, seed, sizeof(seed));
    if (!br_ssl_client_reset(&c->sc, hostname, 0)) {
        close(fd);
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_connect: invalid TLS server name");
        return result;
    }
    br_sslio_init(&c->ioc, &c->sc.eng, osc_tls_br_read, &c->sock, osc_tls_br_write, &c->sock);
    if (br_sslio_flush(&c->ioc) < 0) {
        err = br_ssl_engine_last_error(&c->sc.eng);
        close(fd);
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr(osc_tls_bearssl_error(err));
        return result;
    }

    c->in_use = 1;
    result.is_ok = 1;
    result.value.ok = (int32_t)slot;
    return result;
}

osc_result_i32_str osc_tls_send(int32_t handle, osc_str data)
{
    osc_result_i32_str result;
    osc_tls_conn *c;

    if (handle < 0 || handle >= OSC_TLS_MAX_CONNECTIONS || !osc_tls_conns[handle].in_use) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_send: invalid TLS handle");
        return result;
    }
    c = &osc_tls_conns[handle];
    if (br_sslio_write_all(&c->ioc, data.data, (size_t)data.len) < 0 || br_sslio_flush(&c->ioc) < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("tls_send: send failed");
        return result;
    }
    result.is_ok = 1;
    result.value.ok = data.len;
    return result;
}

osc_str osc_tls_recv(osc_arena *arena, int32_t handle, int32_t max_len)
{
    osc_str result;
    char *buf;
    int n;

    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf || handle < 0 || handle >= OSC_TLS_MAX_CONNECTIONS || !osc_tls_conns[handle].in_use) {
        result.data = "";
        result.len = 0;
        return result;
    }
    n = br_sslio_read(&osc_tls_conns[handle].ioc, buf, (size_t)max_len);
    if (n <= 0) {
        result.data = "";
        result.len = 0;
        return result;
    }
    result.data = buf;
    result.len = (int32_t)n;
    return result;
}

int32_t osc_tls_recv_byte(int32_t handle)
{
    unsigned char b;
    osc_tls_conn *c;
    int n;

    if (handle < 0 || handle >= OSC_TLS_MAX_CONNECTIONS || !osc_tls_conns[handle].in_use) return -1;
    c = &osc_tls_conns[handle];
    n = br_sslio_read(&c->ioc, &b, 1);
    return n == 1 ? (int32_t)b : -1;
}

void osc_tls_close(int32_t handle)
{
    osc_tls_conn *c;

    if (handle < 0 || handle >= OSC_TLS_MAX_CONNECTIONS || !osc_tls_conns[handle].in_use) return;
    c = &osc_tls_conns[handle];
    (void)br_sslio_close(&c->ioc);
    if (c->sock >= 0) close(c->sock);
    memset(c, 0, sizeof(*c));
    c->sock = -1;
}

void osc_tls_cleanup(void)
{
    for (int i = 0; i < OSC_TLS_MAX_CONNECTIONS; i++) {
        if (osc_tls_conns[i].in_use) osc_tls_close(i);
    }
    osc_tls_free_anchors();
    osc_tls_initialized = 0;
}

#else

osc_result_i32_str osc_tls_connect(osc_str host, int32_t port)
{
    osc_result_i32_str result;
    (void)host;
    (void)port;
    result.is_ok = 0;
    result.value.err = osc_str_from_cstr("tls_connect: not supported in hosted mode on this platform");
    return result;
}

osc_result_i32_str osc_tls_send(int32_t handle, osc_str data)
{
    osc_result_i32_str result;
    (void)handle;
    (void)data;
    result.is_ok = 0;
    result.value.err = osc_str_from_cstr("tls_send: not supported in hosted mode on this platform");
    return result;
}

osc_str osc_tls_recv(osc_arena *arena, int32_t handle, int32_t max_len)
{
    osc_str result;
    (void)arena;
    (void)handle;
    (void)max_len;
    result.data = "";
    result.len = 0;
    return result;
}

int32_t osc_tls_recv_byte(int32_t handle)
{
    (void)handle;
    return -1;
}

void osc_tls_close(int32_t handle) { (void)handle; }
void osc_tls_cleanup(void) {}

#endif
