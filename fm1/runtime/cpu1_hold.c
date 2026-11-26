/* CPU0 can hold CPU1 before CRT initialization or a software reset. */
#include <stdint.h>
#include "cpu1_state.h"

typedef uint32_t u32;
#define REG(addr) (*(volatile u32 *)(uintptr_t)(addr))
#define C1_CON        REG(0x01eee004)
#define DBG_WR_EN     REG(0x01eee240)
#define DBG_MSG_CLR   REG(0x01eee248)
#define DBG_EN        REG(0x01eee340)
#define HOLD          (1u << 1)
#define RUN           (1u << 3)

static inline void sync(void) { __asm__ volatile("csync" ::: "memory"); }

/* Writing the key toggles write access. Preserve the incoming state. */
void fm1_cpu1_debug_access(u32 enabled) {
    if ((DBG_WR_EN & 1u) != enabled)
        DBG_WR_EN = 0xe7;
    sync();
}

/* Also called before .data/.bss exist. Only immediate MMIO addresses and
   the private CPU0 stack are usable. A hold may raise a delayed bus fault. */
int fm1_cpu1_try_hold(void) {
    const u32 bus_causes = 0x003f0030;
    u32 access = DBG_WR_EN & 1u;
    fm1_cpu1_debug_access(1);
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
    fm1_cpu1_debug_access(access);
    return (C1_CON & (HOLD | RUN)) == HOLD;
}
