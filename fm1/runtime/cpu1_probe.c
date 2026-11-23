/* CPU0 side of the one-shot AC791N probe. No CPU1 pointer crosses into Rust.
   Register layout: AC79 SDK wl82/csfr.h. Sequence: pi32v2-isa docs/CPU1.md. */
#include <stdint.h>

typedef uint32_t u32;
#define REG(addr) (*(volatile u32 *)(uintptr_t)(addr))
#define SYS_DIV       REG(0x00010008)
#define T4_CON        REG(0x00010800)
#define T4_CNT        REG(0x00010804)
#define T4_PRD        REG(0x00010808)
#define C1_CON        REG(0x01eee004)
#define DBG_WR_EN     REG(0x01eee240)
#define DBG_MSG       REG(0x01eee244)
#define DBG_MSG_CLR   REG(0x01eee248)
#define DBG_EN        REG(0x01eee340)
#define C0_WR_LIMIT   REG(0x01eee348)
#define PC_LIMIT(i)   REG(0x01eee380 + 4u * (i)) /* high, low, high, low */
#define C1_IRQ_CFG(i) REG(0x01eef300 + 4u * (i))
#define C1_IRQ_CLEAR  REG(0x01eef3a4)
#define EXCEPTION     REG(0x01c7fe04) /* vector 1 */
#define BOOT_PC       REG(0x01c7fff8) /* also CPU0 SOFT6, vector 126 */
#define HOLD          (1u << 1)
#define RUN           (1u << 3)

extern void fm1_cpu1_entry(void), fm1_cpu1_probe_exception(void);
extern void fm1_cpu1_seed(u32 challenge);
extern u32 fm1_cpu1_reply(void);
/* cpu1_entry.S owns the storage. CPU0 reads this snapshot only after hold. */
extern const volatile struct {
    u32 reply, core, usp, sp, ssp;
} fm1_cpu1_response;
extern const unsigned char fm1_cpu1_ustack_top[], fm1_cpu1_sstack_top[];

static inline void sync(void) { __asm__ volatile("csync" ::: "memory"); }

/* Writing the key toggles write access. Preserve the incoming state. */
static void debug_access(u32 enabled) {
    if ((DBG_WR_EN & 1u) != enabled)
        DBG_WR_EN = 0xe7;
    sync();
}

/* Also called before .data/.bss exist. Only immediate MMIO addresses and
   the private CPU0 stack are usable. A hold may raise a delayed bus fault. */
void fm1_cpu1_hold(void) {
    const u32 bus_causes = 0x003f0030;
    u32 access = DBG_WR_EN & 1u;
    debug_access(1);
    u32 debug = DBG_EN;
    DBG_EN = debug & ~bus_causes;
    sync();
    C1_CON |= HOLD;
    C1_CON &= ~RUN;
    sync();
    for (unsigned n = 0; n < 256; ++n)
        __asm__ volatile("nop");
    DBG_MSG_CLR = bus_causes;
    sync();
    DBG_EN = debug;
    sync();
    debug_access(access);
    /* Fail closed before RAM initialization or return to the BSP. */
    if ((C1_CON & (HOLD | RUN)) != HOLD)
        for (;;)
            __asm__ volatile("nop");
}

/* Called exactly once, on CPU0 with IRQs masked, after crt0 has copied
   SRAM and before Board enables timers and interrupts. Returns a scalar
   only after reasserting hold on every outcome. Never restart in place. */
u32 fm1_cpu1_probe_run(void) {
    for (unsigned i = 0; i < 32; ++i)
        C1_IRQ_CFG(i) = 0;
    C1_IRQ_CLEAR = 0xff;
    T4_CON = 0x4000;
    T4_CNT = 0;
    T4_PRD = UINT32_MAX;
    T4_CON = 0x4009; /* 24 MHz crystal */

    u32 access = DBG_WR_EN & 1u;
    debug_access(1);
    u32 limits[4];
    for (unsigned i = 0; i < 4; ++i)
        limits[i] = PC_LIMIT(i);
    for (unsigned i = 0; i < 4; i += 2) {
        PC_LIMIT(i) = UINT32_MAX;
        PC_LIMIT(i + 1) = 0;
    }
    u32 protection = C0_WR_LIMIT;
    C0_WR_LIMIT = protection & ~4u;
    sync();
    u32 vector = EXCEPTION, entry = BOOT_PC, divider = SYS_DIV;
    u32 previous_causes = DBG_MSG;
    EXCEPTION = (u32)(uintptr_t)fm1_cpu1_probe_exception;
    BOOT_PC = (u32)(uintptr_t)fm1_cpu1_entry;
    /* Rotating bit 24 into bit 31 keeps valid replies disjoint from all
       status codes. Every challenge bit participates in the comparison. */
    u32 challenge = (T4_CNT ^ 0x79100000u) | (1u << 24);
    u32 expected = ((challenge << 7) | (challenge >> 25)) ^ 0xac791u;
    fm1_cpu1_seed(challenge);
    SYS_DIV = divider | 8u;
    sync();
    C1_CON |= RUN;
    C1_CON &= ~HOLD;
    sync();

    u32 start = T4_CNT, reply = 0;
    /* The poll limit also terminates if TIMER4 fails to tick. */
    for (unsigned polls = 0; polls < 1000000 && T4_CNT - start < 480000; ++polls) {
        reply = fm1_cpu1_reply();
        if (reply != 0)
            break;
    }
    u32 faults = DBG_MSG & ~previous_causes;
    fm1_cpu1_hold();
    /* The ROM ran with access to the debug box. Reopen it before restoring
       protected state, even if the boot path changed the write-enable bit. */
    debug_access(1);
    C0_WR_LIMIT = protection & ~4u;
    sync();
    faults |= DBG_MSG & ~previous_causes;
    DBG_MSG_CLR = faults;
    SYS_DIV = divider;
    BOOT_PC = entry;
    EXCEPTION = vector;
    sync();
    C0_WR_LIMIT = protection;
    for (unsigned i = 0; i < 4; ++i)
        PC_LIMIT(i) = limits[i];
    sync();
    debug_access(access);
    T4_CON = 0x4000;
    if (faults != 0)
        return 4;
    if (reply == expected) {
        u32 user = (u32)(uintptr_t)fm1_cpu1_ustack_top;
        if (fm1_cpu1_response.reply != reply)
            return 3;
        if (fm1_cpu1_response.core != 1 || fm1_cpu1_response.usp != user ||
            fm1_cpu1_response.sp != user ||
            fm1_cpu1_response.ssp != (u32)(uintptr_t)fm1_cpu1_sstack_top)
            return 2;
        return 1;
    }
    if (reply == 0 || reply == 2 || reply == 4)
        return reply; /* timeout, bad execution context, or CPU1 exception */
    return 3; /* wrong computed word */
}
