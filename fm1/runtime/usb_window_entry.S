/* CPU0's window probe owns vector 1. Never return to the interrupted
   Rust or SIE frame. A CPU1 entry violates the bench setup and parks. */
    .section .text.fm1_window_exception,"ax",@progbits
    .globl fm1_window_exception
fm1_window_exception:
    cli
    r0 = cnum
    if (r0 == 0) goto fm1_window_fault
1:  goto 1b
