"""Restore PNGs stored with the first twenty byte pairs reversed."""

SWAPPED_SIGNATURE = b'P\x89GN\n\r\n\x1a'
SWAPPED_IHDR = b'\x00\x00\x0d\x00HIRD'
HEADER_BYTES = 40


def has_swapped_png_header(data):
    return len(data) >= HEADER_BYTES and data.startswith(SWAPPED_SIGNATURE + SWAPPED_IHDR)


def restore_png_header(data):
    header = bytearray(HEADER_BYTES)
    header[::2] = data[1:HEADER_BYTES:2]
    header[1::2] = data[:HEADER_BYTES:2]
    return bytes(header) + data[HEADER_BYTES:]
