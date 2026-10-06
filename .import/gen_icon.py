import math, struct, zlib, sys
N = 1024
S = 4  # supersampling per axis is costly; use analytic coverage instead
bg = (17, 22, 28)
line_c = (163, 230, 53)
grid_c = (40, 50, 60)
R = 200  # corner radius
M = 40   # margin
def wave(x):
    u = (x - M) / (N - 2 * M)
    return N / 2 - 230 * math.sin(u * 2 * math.pi * 1.5) * math.exp(-1.6 * (u - 0.5) ** 2)
pts = [(x, wave(x)) for x in range(M + 60, N - M - 60, 2)]
def seg_dist(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)
rows = []
half = 26
for y in range(N):
    row = bytearray([0])
    # candidate segments near this row
    cand = [(pts[i], pts[i + 1]) for i in range(len(pts) - 1)
            if min(pts[i][1], pts[i + 1][1]) - half - 2 <= y <= max(pts[i][1], pts[i + 1][1]) + half + 2]
    for x in range(N):
        cx = min(max(x + 0.5, M + R), N - M - R)
        cy = min(max(y + 0.5, M + R), N - M - R)
        d = math.hypot(x + 0.5 - cx, y + 0.5 - cy) - R
        a = max(0.0, min(1.0, 0.5 - d))
        if a <= 0:
            row += b"\0\0\0\0"
            continue
        r, g, b = bg
        if (y - M) % 128 == 0 or (x - M) % 128 == 0:
            r, g, b = grid_c
        if cand and M + 40 < x < N - M - 40:
            dist = min(seg_dist(x + 0.5, y + 0.5, *p, *q) for p, q in cand)
            k = max(0.0, min(1.0, half - dist + 0.5))
            if k > 0:
                r = round(r + (line_c[0] - r) * k)
                g = round(g + (line_c[1] - g) * k)
                b = round(b + (line_c[2] - b) * k)
        row += bytes((r, g, b, round(255 * a)))
    rows.append(bytes(row))
def chunk(t, data):
    return struct.pack(">I", len(data)) + t + data + struct.pack(">I", zlib.crc32(t + data))
png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0))
       + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b""))
open(sys.argv[1], "wb").write(png)
