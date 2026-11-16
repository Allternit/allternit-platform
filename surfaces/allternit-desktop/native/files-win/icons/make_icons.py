"""Writes the colour badge icons for Explorer (design pick 6A). Stdlib only.

    python3 make_icons.py

Each .ico holds 16, 24 and 32 px images: a filled circle in the badge colour with
a white mark. Colours match the Finder badge set in the approved designs.
"""
import struct

BADGES = {
    "badge-agent.ico": ((0x7A, 0x5A, 0xF8), "dot"),      # Agent output: violet
    "badge-shared.ico": ((0x2E, 0x90, 0xFA), "ring"),    # Shared: blue
    "badge-conflict.ico": ((0xF0, 0x44, 0x38), "bang"),  # Conflict: red
}


def pixel(x, y, n, rgb, mark):
    c = (n - 1) / 2.0
    dx, dy = x - c, y - c
    r = n / 2.0
    d2 = dx * dx + dy * dy
    if d2 > r * r:
        return (0, 0, 0, 0)
    edge = r - (d2 ** 0.5)
    a = 255 if edge >= 1 else int(255 * max(edge, 0))
    white = False
    if mark == "dot":
        white = d2 <= (r * 0.32) ** 2
    elif mark == "ring":
        white = (r * 0.30) ** 2 <= d2 <= (r * 0.52) ** 2
    elif mark == "bang":
        w = max(1.0, n * 0.09)
        white = abs(dx) <= w and (-r * 0.55 <= dy <= r * 0.15 or r * 0.32 <= dy <= r * 0.52)
    if white:
        return (255, 255, 255, a)
    return (rgb[0], rgb[1], rgb[2], a)


def dib(n, rgb, mark):
    header = struct.pack("<IiiHHIIiiII", 40, n, n * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    rows = []
    for y in range(n - 1, -1, -1):  # bottom-up
        row = b"".join(struct.pack("BBBB", p[2], p[1], p[0], p[3]) for p in (pixel(x, y, n, rgb, mark) for x in range(n)))
        rows.append(row)
    mask_row = b"\x00" * (((n + 31) // 32) * 4)
    return header + b"".join(rows) + mask_row * n


def ico(rgb, mark, sizes=(16, 24, 32)):
    images = [dib(n, rgb, mark) for n in sizes]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for n, img in zip(sizes, images):
        out += struct.pack("<BBBBHHII", n, n, 0, 0, 1, 32, len(img), offset)
        offset += len(img)
    return out + b"".join(images)


if __name__ == "__main__":
    for name, (rgb, mark) in BADGES.items():
        with open(name, "wb") as f:
            f.write(ico(rgb, mark))
        print("wrote", name)
