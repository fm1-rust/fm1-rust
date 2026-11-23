// A dedicated full-speed host for an FM-1 running the RXS1 sacrificial image.
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include "pico/stdlib.h"
#include "hardware/clocks.h"
#include "hardware/sync.h"
#include "pio_usb.h"
#include "pio_usb_ll.h"
#include "tusb.h"

struct vector {
    uint16_t length;
    bool bad_crc;
    uint16_t encoded_len;
    const uint8_t *bytes;
};
#include "vectors.h"

enum { ARENA = 8192, BOUND = 2048, WINDOW_MS = 500 };
static uint8_t tx[10000] __attribute__((aligned(4)));
static uint8_t snap[ARENA];
static uint8_t address;
static unsigned generation, verified_generation;
static struct {
    bool busy, ok;
    uint32_t actual;
} ctl;
static tusb_control_request_t setup;
static uint8_t control_buffer[64];
static const uint8_t caps[12] = {'R', 'X', 'S', '1', 3, 0, 0, 32, 0, 8, 0xf4, 1};

static void pump(void) {
    tud_task_ext(0, false);
    tuh_task_ext(0, false);
}
void tuh_mount_cb(uint8_t addr) {
    uint16_t vid = 0, pid = 0;
    if (tuh_vid_pid_get(addr, &vid, &pid) && vid == 0x1209 && pid == 1) {
        address = addr;
        generation++;
    }
}
void tuh_umount_cb(uint8_t addr) {
    if (address == addr) {
        address = 0;
        generation++;
    }
}

static void print_line(const char *fmt, ...) {
    char msg[240];
    va_list args;
    va_start(args, fmt);
    int n = vsnprintf(msg, sizeof(msg), fmt, args);
    va_end(args);
    if (n < 0 || (size_t)n >= sizeof(msg))
        return;
    absolute_time_t deadline = make_timeout_time_ms(1500);
    for (int i = 0; i < n && tud_cdc_connected() && !time_reached(deadline);) {
        i += (int)tud_cdc_write(msg + i, (uint32_t)(n - i));
        tud_cdc_write_flush();
        pump();
    }
}

static void finish(tuh_xfer_t *x) {
    ctl.ok = x->result == XFER_RESULT_SUCCESS;
    ctl.actual = x->actual_len;
    ctl.busy = false;
}

// The request and its buffer outlive even an abandoned transfer. Fail closed
// after timeout. The caller never reuses the buffer while the host stack owns it.
static bool ctrl(uint8_t type, uint8_t req, uint16_t val, uint16_t idx, uint16_t len) {
    if (ctl.busy || !address || len > sizeof(control_buffer))
        return false;
    unsigned current = generation;
    setup = (tusb_control_request_t){
        .bmRequestType = type, .bRequest = req, .wValue = val, .wIndex = idx, .wLength = len};
    tuh_xfer_t x = {.daddr = address,
                    .ep_addr = 0,
                    .setup = &setup,
                    .buffer = len ? control_buffer : NULL,
                    .complete_cb = finish};
    ctl.busy = true;
    ctl.ok = false;
    if (!tuh_control_xfer(&x)) {
        ctl.busy = false;
        return false;
    }
    absolute_time_t deadline = make_timeout_time_ms(1700);
    while (ctl.busy && current == generation && !time_reached(deadline))
        pump();
    return current == generation && !ctl.busy && ctl.ok && ctl.actual == len;
}

static uint16_t le(const uint8_t *b) { return (uint16_t)(b[0] | (b[1] << 8)); }
static bool select_alt(unsigned alt) { return ctrl(1, 11, 0, 0, 0) && ctrl(1, 11, alt, 0, 0); }
static void __not_in_flash_func(send_raw)(uint8_t addr, uint16_t size) {
    pio_port_t *port = PIO_USB_PIO_PORT(0);
    // The PIO host and its SOF timer both run on this core. One continuous
    // DATA packet can occupy multiple frame times. Resume SOFs after its EOP.
    uint32_t irq = save_and_disable_interrupts();
    pio_usb_bus_send_token(port, USB_PID_OUT, addr, 1);
    pio_usb_bus_usb_transfer(port, tx, size);
    restore_interrupts(irq);
}

static void dump(unsigned token) {
    static const char digits[] = "0123456789abcdef";
    for (unsigned off = 0; off < ARENA; off += 64) {
        char line[129];
        for (unsigned j = 0; j < 64; j++) {
            uint8_t b = snap[off + j];
            line[2 * j] = digits[b >> 4];
            line[2 * j + 1] = digits[b & 15];
        }
        line[128] = 0;
        print_line("PAGE %u %u %s\r\n", token, off, line);
    }
}

enum verdict { OBSERVED, GUARD, INVALID, UNSEEN, BROKEN };
static const char *label[] = {"observed", "guard-changed", "inconclusive", "no-payload-witness",
                              "recovery-failed"};

static enum verdict trial(const struct vector *v, unsigned code, uint8_t fill, bool recovery) {
    static const uint8_t maxima[] = {0xff, 0x40, 0x08, 0x09};
    if (!address || generation != verified_generation)
        return INVALID;
    unsigned n = v ? v->length : 0;
    if (v) {
        if (n > 4096 || v->encoded_len > sizeof(tx))
            return INVALID;
        memcpy(tx, v->bytes, v->encoded_len);
    }
    unsigned alt = recovery ? 1 : 2;
    if (!select_alt(alt) || !ctrl(0xc0, 0x54, 0, 0, 2))
        return INVALID;
    uint16_t token = le(control_buffer);
    if (!token || !ctrl(0x40, 0x60 + 2 * code, n | (fill == 0x5a ? 0x8000 : 0), token, 0) ||
        !ctrl(0xc0, 0x52, 0, token, 24) || le(control_buffer) != token ||
        control_buffer[2] != 1 || control_buffer[3] != fill ||
        control_buffer[4] != maxima[code] || control_buffer[7] != alt)
        return INVALID;
    if (generation != verified_generation)
        return INVALID;
    if (v)
        send_raw(address, v->encoded_len);
    absolute_time_t earliest = make_timeout_time_ms(WINDOW_MS + 40);
    while (!time_reached(earliest))
        pump();
    if (generation != verified_generation || !ctrl(0xc0, 0x52, 0, token, 24) ||
        le(control_buffer) != token || control_buffer[2] != 3 || control_buffer[3] != fill ||
        control_buffer[4] != maxima[code] || control_buffer[7] != alt)
        return INVALID;
    uint8_t status[24];
    memcpy(status, control_buffer, sizeof(status));
    for (unsigned off = 0; off < ARENA; off += 64) {
        if (!ctrl(0xc0, 0x53, off / 64, token, 64))
            return INVALID;
        memcpy(snap + off, control_buffer, 64);
    }
    int highest = -1, guard = -1;
    for (int i = 0; i < ARENA; i++)
        if (snap[i] != fill) {
            highest = i;
            if (guard < 0 && i >= BOUND)
                guard = i;
        }
    unsigned prefix = 0;
    while (prefix < n && snap[prefix] == reference[prefix])
        prefix++;
    enum verdict result = OBSERVED;
    if (!v && (highest >= 0 || status[6] || status[23]))
        result = INVALID;
    else if (guard >= 0)
        result = GUARD;
    else if (recovery && (prefix != 64 || status[6] != 1 || le(status + 9) != 64))
        result = BROKEN;
    else if (v && ((code == 0 && n == 69 && prefix != 69) || prefix < 16))
        result = UNSEEN;
    print_line("TRIAL %u max=%02x fill=%02x length=%u bad_crc=%u recover=%u ready=%u count=%u "
               "irq=%u high=%d first_guard=%d prefix=%u %s\r\n",
               token, maxima[code], fill, n, v && v->bad_crc, recovery, status[6],
               le(status + 9), status[23], highest, guard, prefix, label[result]);
    dump(token);
    return result;
}

static void run(void) {
    if (!address || !ctrl(0xc0, 0x50, 0, 0, 12) || memcmp(control_buffer, caps, 5) ||
        memcmp(control_buffer + 6, caps + 6, 6)) {
        print_line("END inconclusive: attach the 1209:0001 RXS1 stress image\r\n");
        return;
    }
    verified_generation = generation;
    bool clean = true;
    for (unsigned p = 0; p < 2; p++) {
        uint8_t fill = p ? 0x5a : 0xa5;
        for (unsigned code = 0; code < 4; code++) {
            // A quiet and a positive control qualify each hardware setting.
            if (trial(NULL, code, fill, false) != OBSERVED) {
                print_line("END inconclusive: quiet control\r\n");
                return;
            }
            enum verdict check = trial(&vectors[0], code, fill, true);
            if (check == INVALID) {
                print_line("END inconclusive: control transport\r\n");
                return;
            }
            if (check != OBSERVED)
                clean = false;
            // Qualify the raw generator against the known historical crossing.
            if (code == 0 && trial(&vectors[1], code, fill, false) != OBSERVED) {
                print_line("END inconclusive: historical 69-byte calibration\r\n");
                return;
            }
            for (unsigned i = 2; i < sizeof(vectors) / sizeof(vectors[0]); i++) {
                enum verdict res = trial(&vectors[i], code, fill, false);
                if (res == INVALID) {
                    print_line("END inconclusive: control transport\r\n");
                    return;
                }
                if (res != OBSERVED)
                    clean = false;
            }
            if (trial(&vectors[0], code, fill, true) != OBSERVED)
                clean = false;
        }
    }
    print_line("END %s\r\n", clean ? "observed" : "review required");
}

int main(void) {
    set_sys_clock_khz(120000, true);
    gpio_init(18);
    gpio_set_dir(18, GPIO_OUT);
    gpio_put(18, 1);
    pio_usb_configuration_t cfg = PIO_USB_DEFAULT_CONFIG;
    cfg.pin_dp = 16;
    tuh_configure(1, TUH_CFGID_RPI_PIO_USB_CONFIGURATION, &cfg);
    tud_init(0);
    tuh_init(1);
    while (true) {
        pump();
        if (tud_cdc_available() && tud_cdc_read_char() == 'r') {
            print_line("BEGIN FM1-RXS1-FEATHER 1\r\n");
            run();
        }
    }
}
