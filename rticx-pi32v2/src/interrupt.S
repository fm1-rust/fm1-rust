/* CPU0 vector entries. Saving RETI allows a higher-priority interrupt to
 * nest. The C ABI preserves r4-r15; save the caller's registers here.
 * See pi32v2-isa/docs/ABI.md, sections 5 and 7. */
.macro vector name, handler
    .section .text.__rticx_irq_\name,"ax",@progbits
    .global __rticx_irq_\name
    .weak \handler
__rticx_irq_\name:
    [--sp] = {psr, rets, reti}
    [--sp] = {r3-r0}
    /* Hardware clears the nesting gate on entry. Open both gates after
     * saving RETI, then restore the entry ICFG before returning. */
    r0 = icfg
    [--sp] = {r0}
    r0 = 0x300
    icfg = r0
    csync
    call \handler
    {r0} = [sp++]
    icfg = r0
    csync
    {r3-r0} = [sp++]
    {psr, rets, reti} = [sp++]
    csync
    rti
.endm

.irp irq,USB0,ALNK0,SPI1,UART1,SARADC,SPI2,TIMER5,SOFT0,SOFT1,SOFT2,SOFT3,SOFT4,SOFT5,SOFT6,SOFT7
    vector \irq, \irq
.endr

/* The board clock owns this vector independently of the application task names. */
vector TIMER2, __fm1_rticx_deadline_irq

.text
.global __rticx_enable
__rticx_enable:
    r0 = 0x300
.global __rticx_irq_restore
__rticx_irq_restore:
    csync
    icfg = r0
    csync
    rts

.global __rticx_idle
__rticx_idle:
    idle
    rts

.global __rticx_irq_save
__rticx_irq_save:
    r0 = icfg
    cli
    csync
    rts
