#ifndef RING_ADAPTER_H
#define RING_ADAPTER_H

#include <stdint.h>
#include <stdatomic.h>
#include <stddef.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Must match user/src/ring.rs RingHeader exactly */

typedef struct {
    atomic_uint_least64_t head;      /* offset 0 */
    atomic_uint_least64_t tail;      /* offset 8 */
    atomic_uint_least32_t flags;     /* offset 16 */
    uint32_t               capacity; /* offset 20 (write-once) */
    /* data starts at offset 24 */
} __attribute__((packed, aligned(8))) ring_header_t;

static inline uint64_t ring_head(ring_header_t *h) {
    return atomic_load_explicit(&h->head, memory_order_acquire);
}
static inline void ring_set_head(ring_header_t *h, uint64_t v) {
    atomic_store_explicit(&h->head, v, memory_order_release);
}
static inline uint64_t ring_tail(ring_header_t *h) {
    return atomic_load_explicit(&h->tail, memory_order_acquire);
}
static inline void ring_set_tail(ring_header_t *h, uint64_t v) {
    atomic_store_explicit(&h->tail, v, memory_order_release);
}
static inline uint8_t *ring_data(ring_header_t *h) {
    return (uint8_t *)(h + 1);
}

/* Non-blocking push. Returns 0 on success, -1 on full. */
int ring_try_push(ring_header_t *hdr, const uint8_t *data, size_t len);

/* Non-blocking pop. Returns bytes read on success, -1 on empty. */
ssize_t ring_try_pop(ring_header_t *hdr, uint8_t *buf, size_t max_len);

/* Busy-poll helpers */
void ring_push_spin(ring_header_t *hdr, const uint8_t *data, size_t len);
size_t ring_pop_spin(ring_header_t *hdr, uint8_t *buf, size_t max_len);

#ifdef __cplusplus
}
#endif

#endif /* RING_ADAPTER_H */
