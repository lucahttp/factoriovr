import struct, sys, zlib
src, dst = sys.argv[1], sys.argv[2]
d = open(src, 'rb').read()
pos, idat, w, h = 8, b'', 0, 0
while pos + 8 <= len(d):
    l, t = struct.unpack('>I4s', d[pos:pos+8])
    if t == b'IHDR': w, h = struct.unpack('>II', d[pos+8:pos+16])
    if t == b'IDAT': idat += d[pos+8:pos+8+l]
    pos += 12 + l
    if t == b'IEND': break
raw = zlib.decompress(idat)
F = 4                                   # reducir x4
nw, nh = w // F, h // F
out = bytearray()
for y in range(nh):
    out.append(0)
    for x in range(nw):
        r = g = b = a = 0
        for dy in range(F):
            base = (y*F+dy) * (w*4+1) + 1
            for dx in range(F):
                o = base + (x*F+dx)*4
                r += raw[o]; g += raw[o+1]; b += raw[o+2]; a += raw[o+3]
        n = F*F
        out += bytes((r//n, g//n, b//n, a//n))
def bloque(tipo, datos):
    return struct.pack('>I', len(datos)) + tipo + datos + struct.pack('>I', zlib.crc32(tipo+datos) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n'
png += bloque(b'IHDR', struct.pack('>IIBBBBB', nw, nh, 8, 6, 0, 0, 0))
z = b'\x78\x01'
raw2 = bytes(out)
i = 0
while i < len(raw2):
    n = min(65535, len(raw2)-i)
    fin = 1 if i+n >= len(raw2) else 0
    z += bytes([fin]) + struct.pack('<HH', n, (~n) & 0xffff) + raw2[i:i+n]
    i += n
a, b = 1, 0
for byte in raw2:
    a = (a + byte) % 65521; b = (b + a) % 65521
z += struct.pack('>I', (b << 16) | a)
png += bloque(b'IDAT', z) + bloque(b'IEND', b'')
open(dst, 'wb').write(png)
print("reducida a %dx%d, %d bytes" % (nw, nh, len(png)))
