"""An already-authorized ALSA capture endpoint. Never reopen it between takes."""

from contextlib import contextmanager
import ctypes as c
import errno
import time


def library():
    alsa = c.CDLL("libasound.so.2")
    ptr = c.c_void_p
    signatures = {
        "open": (c.c_int, c.POINTER(ptr), c.c_char_p, c.c_int, c.c_int),
        "close": (c.c_int, ptr),
        "hw_params_malloc": (c.c_int, c.POINTER(ptr)),
        "hw_params_free": (None, ptr),
        "hw_params_any": (c.c_int, ptr, ptr),
        "hw_params_set_access": (c.c_int, ptr, ptr, c.c_int),
        "hw_params_set_format": (c.c_int, ptr, ptr, c.c_int),
        "hw_params_set_channels": (c.c_int, ptr, ptr, c.c_uint),
        "hw_params_set_rate": (c.c_int, ptr, ptr, c.c_uint, c.c_int),
        "hw_params": (c.c_int, ptr, ptr),
        "prepare": (c.c_int, ptr),
        "readi": (c.c_long, ptr, ptr, c.c_ulong),
        "wait": (c.c_int, ptr, c.c_int),
        "drop": (c.c_int, ptr),
    }
    for name, (result, *args) in signatures.items():
        fn = getattr(alsa, "snd_pcm_" + name)
        fn.argtypes, fn.restype = args, result
    return alsa


def check(result):
    if result < 0:
        raise OSError(-result, "ALSA capture failed")
    return result


@contextmanager
def endpoint(device, rate):
    alsa = library()
    pcm = c.c_void_p()
    check(alsa.snd_pcm_open(c.byref(pcm), device.encode("ascii"), 1, 1))  # CAPTURE, NONBLOCK
    try:
        params = c.c_void_p()
        check(alsa.snd_pcm_hw_params_malloc(c.byref(params)))
        try:
            check(alsa.snd_pcm_hw_params_any(pcm, params))
            check(alsa.snd_pcm_hw_params_set_access(pcm, params, 3))  # RW_INTERLEAVED
            check(alsa.snd_pcm_hw_params_set_format(pcm, params, 2))  # S16_LE
            check(alsa.snd_pcm_hw_params_set_channels(pcm, params, 2))
            # Exact, not set_rate_near. Reject an incompatible device before recording.
            check(alsa.snd_pcm_hw_params_set_rate(pcm, params, rate, 0))
            check(alsa.snd_pcm_hw_params(pcm, params))
        finally:
            alsa.snd_pcm_hw_params_free(params)

        def take(seconds, write):
            check(alsa.snd_pcm_prepare(pcm))
            try:
                remaining = seconds * rate
                deadline = time.monotonic() + seconds + 1
                buf = c.create_string_buffer(4 * 1024)
                while remaining:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("PCM capture stalled")
                    requested = min(1024, remaining)
                    n = alsa.snd_pcm_readi(pcm, buf, requested)
                    if n == -errno.EAGAIN:
                        check(alsa.snd_pcm_wait(pcm, 100))
                        continue
                    if not 0 < check(n) <= requested:
                        raise OSError("invalid PCM read size")
                    write(buf.raw[:4 * n])
                    remaining -= n
            finally:
                # No recovery, draining, or automatic restart after loss or overrun.
                check(alsa.snd_pcm_drop(pcm))

        yield take
    finally:
        check(alsa.snd_pcm_close(pcm))
