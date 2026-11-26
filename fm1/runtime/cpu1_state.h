/* CPU0 owns this word. The checked bootstrap publishes its verified persistent
 * peer's reply and clears it only after hardware confirms hold. Startup and
 * shutdown run with CPU0 interrupts masked. The CRT initializes it to zero. */
#pragma once
#include <stdatomic.h>
extern _Atomic unsigned fm1_cpu1_live_reply;

/* CPU0 only, with interrupts masked. Neither operation depends on CRT RAM. */
void fm1_cpu1_debug_access(unsigned enabled);
int fm1_cpu1_try_hold(void);
