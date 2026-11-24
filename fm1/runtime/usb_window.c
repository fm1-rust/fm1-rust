/* WND3, AC791N only. Register layout and bit 2 fault decoding from JieLi
   WL82 asm/csfr.h and cpu/wl82/debug.c. The SDK does not say whether the
   fault suppresses a write. This image measures the ordinary SRAM bytes.
   Only fm1_window_run owns USB and the slot. All paths end in soft reset. */
#include <stdint.h>
#include <stddef.h>

typedef uint32_t u32;
typedef uint8_t u8;
#define REG(a) (*(volatile u32 *)(uintptr_t)(a))
#define X(i) REG(0x01eee000u + (i)*4u)
#define DBG_ACCESS X(0x90)
#define MSG X(0x91)
#define MSG_CLR X(0x92)
#define SLOT 7u
#define BIT (1u << SLOT)
#define LIMIT_H X(0xa0 + SLOT)
#define LIMIT_L X(0xb0 + SLOT)
#define ID X(0xc0 + SLOT)
#define C0_LIM X(0xd2)
#define C1_LIM X(0xd3)
#define LIM X(0xd4)
#define ALLOW X(0xd5)
#define OUTERR X(0xd6)
#define FUSB 0x81u
#define RANGE_FAULT 4u
#define VECTOR REG(0x01c7fe04)
#define IRQ_WORD(i) REG(0x01eef100u + 4u*(i))
#define TIMER5_ENABLE (1u << 28)   /* source 63, register word 7 */
#define TICKS REG(0x10804)
#define MS 24000u
#define CON0 REG(0x11800)
#define SIE REG(0x11804)
#define EP0_ADR REG(0x11818)
#define EP1_RX REG(0x11820)
#define MAGIC 0x33444e57u /* WND3 */
#define NONE UINT32_MAX
#define ARENA 8192u

/* Both destinations and the complete scans stay in the same reserved arena.
   Moving the requested base by 64 bytes tests for a stale destination. */
static u32 prefix(u32 test) { return test & 8u ? 128u : 64u; }

/* Stable little-endian wire header, 2 pages, followed by the complete arena.
   LAST and FIRST are offsets relative to the DMA destination. Host also checks
   all bytes before that destination. No pointer is read from retained RAM. */
enum field {
    TAG, TOKEN, CASE, RESULT, DEST, CUT, FILL, FIRST, AT_CUT, LAST,
    CAUSE, SLOTS, MASTER, WID, TRAPPED, EMU, LOW, HIGH, DEVICE,
    LIMIT_MASK, ALLOW_MASK, OUT_MASK, DBG_ENABLE, INITIAL_MSG, BUS, COUNT,
    ARENA_BASE, CSR0, RXCSR, STARTED_GO, ELAPSED, HASH
};
struct record { u32 h[32]; u8 bytes[ARENA]; };
_Static_assert(sizeof(struct record) == 8320, "retained record extent");
#define R ((volatile struct record *)(uintptr_t)0x01c7c000u)
static volatile u8 arena[ARENA] __attribute__((aligned(64)));
/* A failed setup readback has no stimulus or endpoint completion. Its COUNT,
   CSR0 and RXCSR fields carry the checked address, requested word and observed
   word. This preserves the existing 130-page capture and raw fault evidence. */
enum check {
    CHECK_VECTOR = 0x10, CHECK_TIMER5,
    CHECK_LOW = 0x15, CHECK_HIGH, CHECK_ID, CHECK_ALLOW, CHECK_OUTERR, CHECK_LIMIT
};
static volatile u32 start, phase, have_vector, have_irqs, changed_slot, initial_access;
static u32 saved_vector, old_hi, old_lo, old_id, old_irq0, old_irq7;
extern void fm1_window_exception(void);
static inline void sync(void) { __asm__ volatile("csync" ::: "memory"); }
static inline void acquire(void) { __asm__ volatile("ssync" ::: "memory"); }

static void access(u32 enabled) {
    if ((DBG_ACCESS & 1u) != enabled) DBG_ACCESS = 0xe7;
    sync();
}

static void vector(u32 address) {
    /* The bootloader can protect the top-of-RAM vector table with CPU0 slot 2. */
    u32 old = C0_LIM;
    C0_LIM = old & ~4u;
    sync();
    VECTOR = address;
    sync();
    C0_LIM = old;
    sync();
}

/* Keep back-to-back indexed accesses out of line as in the BSP transaction
   helper. Felucca issue #14 reproduced a TX stall with inlined SIE access.
   Bound every transaction even if USB or TIMER4 stops. */
__attribute__((noinline)) static u32 sie(u32 command) {
    SIE = command;
    sync();
    u32 since = TICKS;
    for (u32 i = 0; i < 50000; ++i) {
        u32 v = SIE;
        if (v & 0x8000) return v & 255;
        if (TICKS - since >= MS / 10) break;
    }
    return NONE;
}
static u32 rd(u32 reg) { return sie(0x4000 | (reg << 8)); }
static u32 wr(u32 reg, u32 v) { return sie((reg << 8) | v); }
static int until(u32 from, u32 ticks) {
    for (u32 i = 0; i < 90000000u; ++i)
        if (TICKS - from >= ticks) return 1;
    return 0;
}

void fm1_window_page(u32 page, u8 *dst) {
    /* Rust checks the 64-byte destination. This can be called only after a
       fresh boot, before GO. Uncommitted/stale data need no interpretation. */
    const volatile u8 *record = (const volatile u8 *)R;
    for (u32 i = 0; i < 64; ++i)
        dst[i] = (page < sizeof(*R)/64 && R->h[TAG] == MAGIC)
                    ? record[page*64 + i] : 0;
}

static void capture_cause(void) {
    R->h[CAUSE] = MSG;
    R->h[SLOTS] = X(0xda);
    R->h[MASTER] = X(0xdb);
    R->h[WID] = X(0xde);
    R->h[EMU] = REG(0x01eef0d4);
}
static void window_config(void) {
    R->h[LOW] = LIMIT_L;
    R->h[HIGH] = LIMIT_H;
    R->h[DEVICE] = ID;
    R->h[LIMIT_MASK] = LIM;
    R->h[ALLOW_MASK] = ALLOW;
    R->h[OUT_MASK] = OUTERR;
    R->h[DBG_ENABLE] = X(0xd0);
}
static void commit(void) {
    u32 hash = 2166136261u;
    const volatile u8 *h = (const volatile u8 *)R;
    for (u32 i = 4; i < offsetof(struct record, h[HASH]); ++i)
        hash = (hash ^ h[i]) * 16777619u;
    for (u32 i = 0; i < ARENA; ++i)
        hash = (hash ^ R->bytes[i]) * 16777619u;
    R->h[HASH] = hash;
    sync();
    R->h[TAG] = MAGIC;
    sync();
}
__attribute__((noreturn)) static void reboot(void) {
    /* Keep USB detached across the soft reset. Boot the installed application
       normally so it can show the retained evidence before attempting USB. */
    for (u32 i = 0; i < 4; ++i) REG(0x01c7fd80 + 4*i) = 0;
    sync();
    REG(0x10000) |= 0x10;
    sync();
    for (;;) __asm__ volatile("nop");
}
__attribute__((noreturn)) static void finish(u32 result) {
    /* A fault must not itself gate EP1 or withdraw protection halfway through
       the packet. Leave the receiver untouched until the common deadline. */
    if (phase == 2 && !until(start, 1000u * MS)) result = 5;
    __asm__ volatile("cli\n\tcsync" ::: "memory");
    phase = 3; /* any new exception during capture/cleanup makes the shot inconclusive */
    R->h[ELAPSED] = TICKS - start;
    /* The deadline path samples the actual fault registers even if vector 1
       never ran. With a vector entry, keep the values captured at entry. */
    if (!R->h[TRAPPED]) capture_cause();
    window_config();
    R->h[BUS] = rd(6);
    if (result < CHECK_VECTOR) {
        if (wr(14, 0) != NONE) R->h[CSR0] = rd(17);
        if (R->h[CASE] & 4) {
            R->h[COUNT] = rd(22);
        } else if (wr(14, 1) != NONE) {
            R->h[RXCSR] = rd(20);
            u32 low = rd(22), high = rd(23);
            if (low != NONE && high != NONE) R->h[COUNT] = low | (high << 8);
        }
    }

    CON0 |= 0x1fu << 19; /* gate endpoint DMA, then detach all of USB0 */
    sync();
    CON0 = 0;
    SIE = 0;
    REG(0x51000) = 0x0c;
    sync();
    /* A failed drain cannot justify reading the arena. Record only invalid
       bytes in that case. Bench contract covers this interval independently. */
    int drained = until(TICKS, 10u * MS);
    if (!drained) result = 5;
    acquire();
    u32 cut = R->h[CUT], fill = R->h[FILL], lead = prefix(R->h[CASE]);
    for (u32 i = 0; i < ARENA; ++i) {
        u8 byte = drained ? arena[i] : 0;
        R->bytes[i] = byte;
        if (i < lead) continue;
        u32 off = i - lead;
        if (off == cut) R->h[AT_CUT] = byte;
        if (byte == fill) continue;
        R->h[LAST] = off;
        if (off >= cut && R->h[FIRST] == NONE) R->h[FIRST] = off;
    }
    R->h[RESULT] = result;
    commit();

    if (have_vector) access(1);
    if (changed_slot) {
        LIM &= ~BIT;
        sync();
        ALLOW &= ~BIT;
        OUTERR &= ~BIT;
        LIMIT_H = old_hi;
        LIMIT_L = old_lo;
        ID = old_id;
        if (MSG & RANGE_FAULT) MSG_CLR = RANGE_FAULT;
        sync();
    }
    if (have_irqs) {
        IRQ_WORD(0) = old_irq0;
        IRQ_WORD(7) = old_irq7;
        sync();
    }
    if (have_vector) {
        vector(saved_vector);
        access(initial_access);
    }
    reboot();
}

static void checked(enum check which, volatile u32 *reg, u32 expected, u32 mask) {
    u32 actual = *reg;
    if ((actual ^ expected) & mask) {
        R->h[COUNT] = (u32)(uintptr_t)reg;
        R->h[CSR0] = expected;
        R->h[RXCSR] = actual;
        finish(which);
    }
}

static void install_exception(void) {
    access(1);
    saved_vector = VECTOR;
    have_vector = 1;
    u32 handler = (u32)(uintptr_t)fm1_window_exception;
    vector(handler);
    checked(CHECK_VECTOR, &VECTOR, handler, NONE);
    /* Install the terminal handler before rearming the fault latches. A CPU
       guard on silicon required this rearm even after DBG_MSG was cleared. */
    X(0xd1) |= (1u << 17) | (1u << 18);
    sync();

    /* TIMER5 is the only regular source on this stand-alone board. Verify
       it is masked before reopening global IRQs. Request vector 1 at prio 7
       as in Felucca. On the black FM-1 its nibble read zero after writing
       0xf0 on all eight R3 guarded attempts. Capture completes on TIMER4's
       deadline, and fault attribution uses DBG_MSG and the slot/master IDs. */
    old_irq0 = IRQ_WORD(0);
    old_irq7 = IRQ_WORD(7);
    have_irqs = 1;
    u32 tick = old_irq7 & ~TIMER5_ENABLE;
    u32 exc = old_irq0 | 0xf0u; /* source 1, priority 7 */
    IRQ_WORD(7) = tick;
    IRQ_WORD(0) = exc;
    sync();
    checked(CHECK_TIMER5, &IRQ_WORD(7), tick, TIMER5_ENABLE);
}

__attribute__((noreturn)) void fm1_window_fault(void) {
    /* Terminal vector, CPU0 only. A nested exception is ambiguous. Leave
       TAG invalid instead of publishing a partly overwritten record. */
    if (R->h[TRAPPED]) { R->h[TAG] = 0; sync(); reboot(); }
    R->h[TRAPPED] = 1;
    capture_cause();
    finish(phase == 2 ? 1 : 4);
}

__attribute__((noreturn)) void fm1_window_run(u32 test, u32 token) {
    __asm__ volatile("cli\n\tcsync" ::: "memory");
    for (u32 i = 0; i < 32; ++i) R->h[i] = 0;
    u32 ep0 = test & 4u, mode = test & 3u;
    u32 cut = mode == 1 ? 128u : (ep0 ? 4u : 32u) + (mode == 3 ? 1u : 0u);
    u32 dest = (u32)(uintptr_t)&arena[prefix(test)];
    u32 fill = test & 8 ? 0x5a : 0xa5;
    R->h[TOKEN] = token; R->h[CASE] = test; R->h[DEST] = dest;
    R->h[CUT] = cut; R->h[FILL] = fill;
    R->h[ARENA_BASE] = (u32)(uintptr_t)&arena[0];
    R->h[FIRST] = R->h[LAST] = R->h[BUS] = R->h[COUNT] =
        R->h[CSR0] = R->h[RXCSR] = NONE;
    initial_access = DBG_ACCESS & 1u;
    R->h[INITIAL_MSG] = MSG;
    start = TICKS;
    /* Preserve every inherited slot. Slot 7 must be unused even in the
       comparator-off control. Never steal a CPU, peripheral or pending slot. */
    if ((C0_LIM | C1_LIM | LIM | ALLOW | OUTERR) & BIT) finish(2);
    if (MSG & RANGE_FAULT) finish(6);
    if ((X(1) & 10u) == 8u) finish(7);
    CON0 |= 1u << 20; /* gate EP1 before installing a new destination */
    sync();
    for (u32 i = 0; i < ARENA; ++i) arena[i] = fill;
    sync();
    /* Comparator-off controls run with CPU0 IRQs masked. They need neither
       a writable exception vector nor any interrupt-bank readback. */
    if (mode) install_exception();
    /* EP0's eight-byte GO SETUP is copied and unacknowledged. It has no OUT
       data stage. Its status IN is a ZLP. A quiet EP0 trial checks this. */
    volatile u32 *adr = ep0 ? &EP0_ADR : &EP1_RX;
    /* Both address registers are ___WO in the WL82 SDK's JL_USB_TypeDef.
       Delivery controls verify the programmed destination from frozen SRAM. */
    *adr = dest;
    sync();
    if (mode) {
        old_hi = LIMIT_H; old_lo = LIMIT_L; old_id = ID;
        changed_slot = 1;
        LIMIT_H = dest + cut - 1; /* hypothesis: inclusive byte address */
        LIMIT_L = dest;
        ID = FUSB;
        u32 allowed = ALLOW | BIT;
        u32 errors = OUTERR | BIT;
        ALLOW = allowed;
        OUTERR = errors;
        sync();
        u32 limits = LIM | BIT;
        LIM = limits;
        sync();
        /* Preserve evidence of within-beat rounding. The host requires
           an exact readback for every claimed crossing or observed stop. */
        checked(CHECK_LOW, &LIMIT_L, dest, NONE);
        checked(CHECK_HIGH, &LIMIT_H, dest + cut - 1, ~7u);
        checked(CHECK_ID, &ID, FUSB, NONE);
        checked(CHECK_ALLOW, &ALLOW, allowed, BIT);
        checked(CHECK_OUTERR, &OUTERR, errors, BIT);
        checked(CHECK_LIMIT, &LIM, limits, BIT);
    }
    if (!ep0) CON0 &= ~(1u << 20);
    sync();
    phase = 2;
    start = TICKS;
    if (mode) {
        u32 flags = 0x300;
        __asm__ volatile("icfg = %0\n\tcsync" :: "r"(flags) : "memory");
    }
    /* Completion of this status IN releases the host to send exactly one
       transfer. No further CPU USB service runs during acquisition. */
    R->h[STARTED_GO] = 1;
    if (wr(17, 0x48) == NONE) finish(4);
    finish(1);
}
