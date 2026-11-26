/* SPDX-License-Identifier: MPL-2.0
 * Register setup adapted from Felucca's fm1_uart.h.
 * Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments */
/* UART1 owns this fixed board SRAM ring for the entire transfer. Rust only
 * receives copies. The UART start/end addresses are independent of caller
 * buffers, and this storage stays reserved even if a handle is forgotten. */
#include <stdint.h>

static volatile uint8_t ring[UINT8_MAX + 1u] __attribute__((aligned(16)));
#define CON (*(volatile uint32_t *)0x12100u)
#define RX_START (*(volatile uint32_t *)0x1211cu)
#define RX_END (*(volatile uint32_t *)0x12120u)
#define COUNT (*(volatile uint32_t *)0x12128u)
#define RX_PENDING (1u << 14)
#define TIMEOUT_PENDING (1u << 11)
#define RX_ON 0x41u

/* Rewriting RXSADR resets the DMA cursor. RXEADR is exclusive. */
void fm1_trs_rx_set_ring(void) {
    RX_START = (uint32_t)(uintptr_t)ring;
    RX_END = (uint32_t)(uintptr_t)(ring + sizeof ring);
}

/* JieLi's WL82 spec_uart.c (uartx_isr) tests pending before RDC, orders
 * the latch, clears the flags, then reads the low 16 count bits. Polling
 * RDC unconditionally has left receivers a byte behind on the FM-1.
 * Protect just the handshake from interruption between latch and ack. */
uint32_t fm1_trs_rx_capture(void) {
    if (!(CON & (RX_PENDING | TIMEOUT_PENDING)))
        return 0;
    uint32_t saved, flags, result = 0;
    __asm__ volatile("%0 = icfg\n\tcli\n\tcsync" : "=r"(saved) :: "memory");
    flags = CON;
    if (flags & (RX_PENDING | TIMEOUT_PENDING)) {
        CON = RX_ON | 0x80u;
        __asm__ volatile("csync" ::: "memory");
        CON = RX_ON | 0x1400u;
        result = (COUNT & 0xffffu) | ((flags & RX_PENDING) << 2);
    }
    __asm__ volatile("csync\n\ticfg = %0\n\tcsync" :: "r"(saved) : "memory");
    return result;
}

void fm1_trs_rx_copy(uint8_t start, uint8_t *out, unsigned len) {
    __asm__ volatile("ssync" ::: "memory");
    for (unsigned i = 0; i < len; ++i)
        out[i] = ring[(uint8_t)(start + i)];
    __asm__ volatile("ssync" ::: "memory");
}
