/* AC791N C ABI. r0 is the stereo i32 destination and r2 is the number of frames.
 * r1 is the source. r3 carries the packed Q2.14 gains (left low, right high)
 * or the right-hand Q23 plane. Pair loads/stores need four-byte alignment.
 *
 * The FM-1 excprobe runs (groups 4, 5, 12) showed that register counts above
 * 31 do not give the requested repeat count, and interrupts can wait for a rep
 * to finish. Use at most 16 frames per repeat. No body reads ahead.
 */

    .macro scale right
    r7_r6 = r6.\right,r6.l *|* r3.h,r3.l (ssat)
    r6 = r6 >>> 6
    r7 = r7 >>> 6
    .endm

    .macro mono_i16
    r6 = h[r1++=2] (u)
    scale l
    .endm

    .macro stereo_i16
    /* Even a stereo frame can start at 2 mod 4. Load both halfwords there. */
    r6 = h[r1++=2] (u)
    r7 = h[r1++=2] (u)
    r6.h = r7.l
    scale h
    .endm

    .macro stereo_q23
    r7_r6 = d[r1++=8]
    .endm

    .macro planar_q23
    r6 = [r1++=4]
    r7 = [r3++=4]
    .endm

    .macro stereo_word
    r6 = [r1++=4]
    scale h
    .endm

    .macro frames load
.Lnext\@:
    r4 = 16
    r8 = umin(r4, r2)
    r2 -= r8
    rep r8 {
        \load
        r5_r4 = d[r0+0]
        r7_r6 = r7,r6 +|+ r5,r4 (ssat)
        d[r0++=8] = r7_r6
    }
    if (r2 != 0) goto .Lnext\@
    .endm

    .macro mix name
    .section .text.fm1_audio_mix_v1_\name,"ax",@progbits
    .p2align 1
    .global fm1_audio_mix_v1_\name
    .hidden fm1_audio_mix_v1_\name
    .type fm1_audio_mix_v1_\name,@function
fm1_audio_mix_v1_\name:
    if (r2 == 0) goto .Ldone\@
    [--sp] = {r8-r4}
    .ifc \name,stereo_i16
        /* Rust guarantees halfword alignment. Test the other address bit once.
         * A native-endian stereo frame fills the aligned word (left low). */
        r4 = r1 & 2
        if (r4 != 0) goto .Lhalfwords\@
        frames stereo_word
        goto .Lpop\@
.Lhalfwords\@:
    .endif
    frames \name
.Lpop\@:
    {r8-r4} = [sp++]
.Ldone\@:
    rts
    .size fm1_audio_mix_v1_\name, .-fm1_audio_mix_v1_\name
    .endm

    mix mono_i16
    mix stereo_i16
    mix stereo_q23
    mix planar_q23
