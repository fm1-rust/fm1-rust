/* SPDX-FileCopyrightText: 2026 Julius Vitkauskas */
/* SPDX-License-Identifier: MIT-0 */
/* AC791N ROM entry and stack switch adapted from the FM-1 app4 hardware test.
   See the cpu1-probe section of the BSP README for the source and protocol. */

/* m-bootloader enters here only in the probe image. Hold the peer before
   the ordinary CRT initializes RAM. */
	.section .entry.cpu1_probe,"ax",@progbits
	.globl fm1_cpu1_probe_reset
fm1_cpu1_probe_reset:
	goto 1f
	rts
1:	cli
	ssp = __sstack_top
	sp = __stack_top
	call fm1_cpu1_hold
	goto _start

/* CPU0 uses these shims to seed and poll the assembly-owned mailbox.
   During the exchange only CPU1 writes the reply. Both lines last until reset. */
	.section .text.fm1_cpu1_mail,"ax",@progbits
	.globl fm1_cpu1_seed
fm1_cpu1_seed:
	r1 = fm1_cpu1_request
	[r1+0] = r0
	r2 = 0
	[r1+32] = r2
	[r1+36] = r2
	[r1+40] = r2
	[r1+44] = r2
	[r1+48] = r2
	csync
	rts
	.globl fm1_cpu1_reply
fm1_cpu1_reply:
	csync
	r1 = fm1_cpu1_response
	r0 = [r1+0]
	rts

/* The complete peer path is copied into SRAM by crt0. No indirect calls,
   CPU0 interrupt handlers, Rust references or borrowed stack arguments. */
	.section .cpu1.text.entry,"ax",@progbits
	.globl fm1_cpu1_entry
fm1_cpu1_entry:
	cli
	r0 = 0
	icfg = r0
	usp = fm1_cpu1_ustack_top
	sp = fm1_cpu1_sstack_top
	ssp = fm1_cpu1_sstack_top
	reti = .Luser
	rti
.Luser:
	cli
	r0 = 0
	icfg = r0
	r1 = fm1_cpu1_response
	r2 = cnum
	[r1+4] = r2
	r3 = usp
	[r1+8] = r3
	r4 = sp
	[r1+12] = r4
	r5 = ssp
	[r1+16] = r5
	r0 = 1
	if (r2 != r0) goto .Lbad_context
	r0 = fm1_cpu1_ustack_top
	if (r3 != r0) goto .Lbad_context
	if (r4 != r0) goto .Lbad_context
	r0 = fm1_cpu1_sstack_top
	if (r5 != r0) goto .Lbad_context
	r1 = fm1_cpu1_request
	r0 = [r1+0]
	call fm1_cpu1_probe_word
	goto .Lpublish
.Lbad_context:
	r0 = 2
.Lpublish:
	csync
	r1 = fm1_cpu1_response
	[r1+0] = r0
	csync
.Lpark:
	goto .Lpark

/* Vector 1 is shared. A CPU1 exception can arrive before its stack is
   usable. A CPU0 exception during bring-up must never resume through CPU1. */
	.globl fm1_cpu1_probe_exception
fm1_cpu1_probe_exception:
	cli
	r0 = cnum
	if (r0 == 0) goto .Lpark
	r0 = 4
	goto .Lpublish

	/* All peer writes use this private storage. Keep it outside Rust's .bss
	   so the board still enters with its ordinary statics freshly initialized. */
	.section .cpu1.scratch,"aw",@nobits
	.p2align 5
fm1_cpu1_request:
	.space 32
	.globl fm1_cpu1_response
fm1_cpu1_response:
	.space 32
	.space 1024
	.globl fm1_cpu1_ustack_top
fm1_cpu1_ustack_top:
	.space 1024
	.globl fm1_cpu1_sstack_top
fm1_cpu1_sstack_top:
