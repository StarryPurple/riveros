#include "ring_adapter.h"
#include <string.h>

#if defined(__x86_64__) || defined(__i386__)
#define cpu_relax() __builtin_ia32_pause()
#else
#define cpu_relax() __asm__ __volatile__("":::"memory")
#endif

int ring_try_push(ring_header_t *hdr, const uint8_t *data, size_t len) {
    if (len == 0 || len > hdr->capacity) return -1;

    uint64_t head = ring_head(hdr);
    uint64_t tail = atomic_load_explicit(&hdr->tail, memory_order_relaxed);

    if (tail - head + len > hdr->capacity) return -1; /* full */

    size_t cap = hdr->capacity;
    size_t pos = (size_t)(tail % cap);
    uint8_t *dst = ring_data(hdr);
    size_t n = (pos + len <= cap) ? len : (cap - pos);
    memcpy(dst + pos, data, n);
    if (n < len) memcpy(dst, data + n, len - n);

    ring_set_tail(hdr, tail + len);
    return 0;
}

ssize_t ring_try_pop(ring_header_t *hdr, uint8_t *buf, size_t max_len) {
    if (max_len == 0) return 0;

    uint64_t tail = ring_tail(hdr);
    uint64_t head = atomic_load_explicit(&hdr->head, memory_order_relaxed);

    uint64_t available = tail - head;
    if (available == 0) return -1; /* empty */

    size_t read_len = (available < max_len) ? (size_t)available : max_len;
    size_t cap = hdr->capacity;
    size_t pos = (size_t)(head % cap);
    const uint8_t *src = ring_data(hdr);
    size_t n = (pos + read_len <= cap) ? read_len : (cap - pos);
    memcpy(buf, src + pos, n);
    if (n < read_len) memcpy(buf + n, src, read_len - n);

    ring_set_head(hdr, head + read_len);
    return (ssize_t)read_len;
}

void ring_push_spin(ring_header_t *hdr, const uint8_t *data, size_t len) {
    while (ring_try_push(hdr, data, len) != 0) {
        cpu_relax();
    }
}

size_t ring_pop_spin(ring_header_t *hdr, uint8_t *buf, size_t max_len) {
    ssize_t n;
    while ((n = ring_try_pop(hdr, buf, max_len)) < 0) {
        cpu_relax();
    }
    return (size_t)n;
}
