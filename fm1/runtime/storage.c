/* AC791N raw SPI NOR, on CPU0. Only functions in RAM run between disabling
 * and restoring SFC. See the Flash storage section of the BSP README. */
#include <stdint.h>

_Static_assert(sizeof(unsigned) == 4 && sizeof(uintptr_t) == 4, "pi32v2 storage ABI");

#define REG(a) (*(volatile uint32_t *)(uintptr_t)(a))
#define SFC        REG(0x40200)
#define SFC_BASE   REG(0x4020c)
#define CACHE      REG(0x1eee008)
#define CPU1       REG(0x1eee004)
#define PIN_MUX    REG(0x5101c)
#define PD_OUT     REG(0x500c0)
#define PD_DIR     REG(0x500c8)
#define SPI        REG(0x11c00)
#define BAUD       REG(0x11c04)
#define BUF        REG(0x11c08)
#define TICKS      REG(0x10804)
#define RAM __attribute__((section(".fm1_storage_ram"), noinline))
#define INLINE static inline __attribute__((always_inline))

enum { OK, BUSY, UNSUPPORTED, IO, RANGE };
enum { PROBE, READ, PROGRAM, ERASE };
enum { PAGE = 256, SECTOR = 4096, STORE_BYTES = 4 * SECTOR };
extern const unsigned char __fm1_storage_start[]; /* absolute linker symbol */

INLINE void sync_bus(void) { __asm__ volatile("csync" ::: "memory"); }

static RAM void select_chip(unsigned selected) {
    if (selected) PD_OUT &= ~1u;
    else PD_OUT |= 1u;
    sync_bus();
    for (volatile unsigned delay = 10; delay; --delay) {}
}

/* Failure can leave the last byte's command framing ambiguous. After a bus
 * failure, never fetch another instruction from flash. A chip reset returns
 * control to the mask ROM. Give an in-flight sector erase time to finish. */
static RAM __attribute__((noreturn)) void reset_from_ram(void) {
    select_chip(0);
    uint32_t start = TICKS;
    for (volatile unsigned n = 0; n < 100000000u && TICKS - start < 48000000u; ++n) {}
    REG(0x10000) |= 1u << 4;
    sync_bus();
    for (;;) {}
}

static RAM unsigned byte_io(unsigned value, unsigned receive) {
    SPI = (SPI & ~0x1000u) | 8u | (receive << 12);
    BUF = value;
    sync_bus();
    for (unsigned n = 0; !(SPI & 0x8000u); ++n)
        if (n == 100000u) reset_from_ram();
    unsigned byte = receive ? BUF & 255u : 0;
    SPI = (SPI | 0x4000u) & ~8u;
    return byte;
}

static RAM void command(unsigned opcode) {
    select_chip(1);
    byte_io(opcode, 0);
}

static RAM unsigned status(void) {
    command(0x05);
    unsigned sr = byte_io(255, 1);
    select_chip(0);
    return sr;
}

static RAM int transfer(unsigned op, unsigned address, uint8_t *page, unsigned len) {
    uint32_t icfg, core;
    __asm__ volatile("%0 = icfg\n\t%1 = cnum" : "=r"(icfg), "=r"(core) :: "memory");
    uint32_t masked = icfg & ~0x300u;
    __asm__ volatile("icfg = %0\n\tcsync" :: "r"(masked) : "memory");
    int result = BUSY;
    if (core || (CPU1 & 0x0au) == 8u) goto unmask;
    unsigned sfc = SFC;
    unsigned mode = (sfc >> 8) & 15u;
    /* Accept the stock single/dual/quad output-read modes, excluding
     * continuous read which would interpret a command as an address. */
    if (!(sfc & 1u) || mode > 3u || SFC_BASE != 0x4000u) {
        result = UNSUPPORTED;
        goto unmask;
    }
    for (unsigned n = 0; !(CACHE & 0x4000u) || (SFC & 0x80000000u); ++n)
        if (n == 100000u) goto unmask;

    unsigned mux = PIN_MUX, spi = SPI, baud = BAUD;
    unsigned pd_out = PD_OUT, pd_dir = PD_DIR;
    PD_OUT |= 1u;
    PD_DIR &= ~1u;
    sync_bus();
    SFC = 0;
    PIN_MUX = mux & ~0x20u;
    SPI = 0;
    BAUD = 3; /* 15 MHz at the bootloader's 60 MHz peripheral clock */
    SPI = 0x5029;
    sync_bus();
    result = OK;
    if (op == PROBE) {
        command(0x9f);
        unsigned id = byte_io(255, 1) << 16;
        id |= byte_io(255, 1) << 8;
        id |= byte_io(255, 1);
        select_chip(0);
        if (id != 0x856014 && id != 0xb36014) result = UNSUPPORTED;
    } else {
        if (op != READ) {
            command(0x06);
            select_chip(0);
            if ((status() & 3u) != 2u) result = IO;
        }
        if (result == OK) {
            command(op == READ ? 0x03 : (op == ERASE ? 0x20 : 0x02));
            byte_io(address >> 16, 0);
            byte_io((address >> 8) & 255u, 0);
            byte_io(address & 255u, 0);
            for (unsigned i = 0; i < len; ++i) {
                if (op == READ) page[i] = byte_io(255, 1);
                else byte_io(page[i], 0);
            }
            select_chip(0);
        }
        if (op != READ) {
            /* A failed WREN may itself have observed BUSY. Restore XIP only
             * after a successful status transaction reports the part idle. */
            uint32_t start = TICKS;
            for (unsigned polls = 0; status() & 1u; ++polls)
                if (TICKS - start >= 48000000u || polls == 2000000u) reset_from_ram();
            command(0x04); /* Clear the write-enable latch after rejected or protected operations. */
            select_chip(0);
        }
    }
    SPI = 0;
    BAUD = baud;
    PIN_MUX = mux;
    SPI = spi;
    sync_bus();
    SFC = sfc & ~0x80000000u;
    sync_bus();
    PD_OUT = (PD_OUT & ~1u) | (pd_out & 1u);
    PD_DIR = (PD_DIR & ~1u) | (pd_dir & 1u);
    sync_bus();
unmask:
    __asm__ volatile("icfg = %0\n\tcsync" :: "r"(icfg) : "memory");
    return result;
}

/* XIP side. Only the local page is ever passed across the XIP-off boundary.
 * All data I/O uses raw SPI, never the SFC's decrypted/cached view. The
 * reserved sectors contain no XIP objects, hence no cache line is consumed
 * for their contents. No DMA, deferred pointer, or write-protection override. */
static int run(unsigned op, unsigned off, const uint8_t *src, uint8_t *dst, unsigned n) {
    uintptr_t base = (uintptr_t)__fm1_storage_start;
    if (base != 0xd8000u || off >= STORE_BYTES || n > STORE_BYTES - off || n > PAGE)
        return RANGE;
    if (op == ERASE && (off & (SECTOR - 1))) return RANGE;
    if (op == PROGRAM && n > PAGE - (off & (PAGE - 1))) return RANGE;
    uint8_t page[PAGE];
    if ((uintptr_t)page < 0x01c00000u || (uintptr_t)page > 0x01c80000u - PAGE ||
        (uintptr_t)transfer < 0x01c00000u || (uintptr_t)transfer >= 0x01c80000u)
        return UNSUPPORTED;
    for (unsigned i = 0; src && i < n; ++i) page[i] = src[i];
    int (*volatile in_ram)(unsigned, unsigned, uint8_t *, unsigned) = transfer;
    int result = in_ram(op, (unsigned)base + off, page, n);
    if (result == OK && dst)
        for (unsigned i = 0; i < n; ++i) dst[i] = page[i];
    return result;
}

int fm1_storage_probe(void) {
    int rc = run(PROBE, 0, 0, 0, 0);
    if (rc) return rc;
    /* The installed app_area_head describes the CRC-covered image, including
     * factory assets from older packagers. Never modify a sector it covers. */
    const uintptr_t xip = 0x02000000u;
    uint32_t size = REG(xip + 8);
    if (REG(xip + 4) != xip + 0x120 || (REG(xip + 12) & 255u) != 0x83 ||
        REG(xip + 16) != 0x5f707061u || REG(xip + 20) != 0x61657261u ||
        REG(xip + 24) != 0x6165685fu || (REG(xip + 28) & 0xffffu) != 'd' ||
        size <= 0x120 || size > (uintptr_t)__fm1_storage_start - 0x4000u)
        return UNSUPPORTED;
    return OK;
}
int fm1_storage_read(unsigned off, uint8_t *dst, unsigned n) { return run(READ, off, 0, dst, n); }
int fm1_storage_program(unsigned off, const uint8_t *src, unsigned n) { return run(PROGRAM, off, src, 0, n); }
int fm1_storage_erase(unsigned off) { return run(ERASE, off, 0, 0, 0); }
