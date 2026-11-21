/* SPDX-FileCopyrightText: 2026 Julius Vitkauskas */
/* SPDX-License-Identifier: MIT-0 */
/* The patched Rust compiler lacks pi32v2 inline assembly and an interrupt ABI.
   Provide the native TIMER5 entry and CPU instructions for the BSP. */
typedef unsigned u32;

void rust_timer5_tick(void);

__attribute__((interrupt)) void fm1_timer5_isr(void) { rust_timer5_tick(); }

u32 fm1_core_id(void) {
    u32 core;
    __asm__ volatile("%0 = cnum" : "=r"(core));
    return core;
}

static inline u32 irq_save(void) {
    u32 state;
    __asm__ volatile("%0 = icfg\n\tcli\n\tcsync" : "=r"(state) :: "memory");
    return state;
}

void fm1_cli(void) { __asm__ volatile("cli" ::: "memory"); }
void fm1_csync(void) { __asm__ volatile("csync" ::: "memory"); }
void fm1_ssync(void) { __asm__ volatile("ssync" ::: "memory"); }
void fm1_set_icfg(u32 v) { __asm__ volatile("csync\n\ticfg = %0\n\tcsync" :: "r"(v) : "memory"); }

/* Disjoint pins can share a register across interrupt priorities. */
void fm1_modify(volatile u32 *reg, u32 clear, u32 set) {
    u32 saved = irq_save();
    *reg = (*reg & ~clear) | set;
    fm1_set_icfg(saved);
}

/* Publish the staging buffer to SPI1 DMA. Passing its address through this
   external call also tells the Rust compiler that the DMA reads the bytes. */
void fm1_spi1_dma(const unsigned char *bytes, u32 len) {
    fm1_csync();
    *(volatile u32 *)0x11d0c = (u32)bytes;
    *(volatile u32 *)0x11d10 = len;
}
