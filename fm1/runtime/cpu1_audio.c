/* Private to rticx-cpu1-audio. CPU0 and CPU1 exchange integers in fixed
   storage. Neither core receives a pointer, buffer length or callback from
   the other. CPU0 initializes all words under hold; each line then has one writer. */
#include <stdatomic.h>
#include <stdint.h>

typedef uint32_t u32;
typedef _Atomic(u32) word;
_Static_assert(sizeof(word) == 4 && ATOMIC_INT_LOCK_FREE == 2, "native SRAM words");
enum { FRAMES = 128 };
static struct {
    _Alignas(32) word job;
    word gate, check, release;
    _Alignas(32) word done;
    word held, checksum, ticks;
    _Alignas(32) word pcm[FRAMES];
} box __attribute__((section(".cpu1.scratch")));

#define RAM __attribute__((section(".cpu1.text.audio")))
#define TICKS (*(const volatile u32 *)(uintptr_t)0x10804)
extern void fm1_cpu1_audio_ready(u32 reply);
extern u32 fm1_cpu1_pcm_sample(u32 job, u32 index); /* fixed Rust arithmetic */

static inline u32 load(const word *p) { return atomic_load_explicit(p, memory_order_relaxed); }
static inline void store(word *p, u32 v) { atomic_store_explicit(p, v, memory_order_relaxed); }
/* The compiler's acquire/release word accesses alone emit no csync on this
   target. Keep both the language ordering and the chip's publication fence. */
static inline void publish(word *p, u32 v) {
    __asm__ volatile("csync" ::: "memory");
    atomic_store_explicit(p, v, memory_order_release);
    __asm__ volatile("csync" ::: "memory");
}
static inline u32 observe(const word *p) {
    u32 v = atomic_load_explicit(p, memory_order_acquire);
    __asm__ volatile("csync" ::: "memory");
    return v;
}

/* Called by CPU0 with CPU1 held, before the first release. .cpu1.scratch
   is NOLOAD and can contain replies and samples from the preceding boot. */
void fm1_cpu1_audio_clear(void) {
    atomic_init(&box.job, 0);
    atomic_init(&box.gate, 0);
    atomic_init(&box.check, 0);
    atomic_init(&box.release, 0);
    atomic_init(&box.done, 0);
    atomic_init(&box.held, 0);
    atomic_init(&box.checksum, 0);
    atomic_init(&box.ticks, 0);
    for (unsigned i = 0; i < FRAMES; ++i)
        atomic_init(&box.pcm[i], 0);
    __asm__ volatile("csync" ::: "memory");
}

void fm1_cpu1_audio_submit(u32 job, u32 gate) {
    store(&box.gate, gate);
    store(&box.check, ~job ^ gate);
    publish(&box.job, job);
}
u32 fm1_cpu1_audio_done(void) { return observe(&box.done); }
u32 fm1_cpu1_audio_held(void) { return observe(&box.held); }
void fm1_cpu1_audio_release(u32 job) { publish(&box.release, job); }
u32 fm1_cpu1_audio_ticks(void) { return load(&box.ticks); }

/* CPU0's exclusive local OutputBlock has exactly FRAMES pairs. The caller
   has observed this job's completion and will not submit until after copy. */
u32 fm1_cpu1_audio_copy(int32_t out[FRAMES][2]) {
    for (unsigned i = 0; i < FRAMES; ++i) {
        int32_t s = (int32_t)load(&box.pcm[i]);
        out[i][0] = out[i][1] = s;
    }
    return load(&box.checksum);
}

/* The checked ROM entry in cpu1_entry.S calls this symbol. The one-shot
   image supplies its own definition and never extracts this archive member. */
RAM u32 fm1_cpu1_probe_word(u32 challenge) {
    u32 last = 0;
    fm1_cpu1_audio_ready(((challenge << 7) | (challenge >> 25)) ^ 0xac791u);
    for (;;) {
        u32 job;
        do { job = observe(&box.job); } while (job == last);
        u32 gate = load(&box.gate);
        if (job != last + 1 || load(&box.check) != (~job ^ gate)) {
            fm1_cpu1_audio_ready(3);
            for (;;) __asm__ volatile("nop");
        }
        u32 start = TICKS;
        u32 digest = job;
        for (unsigned i = 0; i < FRAMES; ++i) {
            if (i == FRAMES / 2 && gate == job) {
                publish(&box.held, job);
                while (observe(&box.release) != job) {}
            }
            u32 s = fm1_cpu1_pcm_sample(job, i);
            store(&box.pcm[i], s);
            digest = ((digest << 5) | (digest >> 27)) + s;
        }
        store(&box.checksum, digest);
        store(&box.ticks, TICKS - start);
        publish(&box.done, job);
        last = job;
    }
}
