#!/usr/bin/env python3
"""Valida la captura del cubo 3D: que el PNG sea legible y tenga el cubo.

Se ejecuta DENTRO del contenedor, donde esta la imagen: copiarla al host por
el socket de podman se corta (la imagen son 3,6 MB) y el banco fallaba por
la copia en vez de por el cubo.

Uso: python3 validar_png_cubo.py <fichero.png>
Sale con 0 si el PNG es valido y tiene pixeles del cubo, con 1 si no.
"""

import struct
import sys
import zlib


def main() -> int:
    if len(sys.argv) < 2:
        print("uso: validar_png_cubo.py <fichero.png>")
        return 2
    ruta = sys.argv[1]
    with open(ruta, "rb") as fh:
        d = fh.read()

    assert d[:8] == b"\x89PNG\r\n\x1a\n", "no es un PNG"
    pos, idat, w, h = 8, b"", 0, 0
    while pos + 8 <= len(d):
        ln = struct.unpack(">I", d[pos:pos + 4])[0]
        typ = d[pos + 4:pos + 8]
        data = d[pos + 8:pos + 8 + ln]
        crc = struct.unpack(">I", d[pos + 8 + ln:pos + 12 + ln])[0]
        assert zlib.crc32(typ + data) & 0xFFFFFFFF == crc, \
            "CRC malo en el bloque %s" % typ.decode(errors="replace")
        if typ == b"IHDR":
            w, h = struct.unpack(">II", data[:8])
        elif typ == b"IDAT":
            idat += data
        pos += 12 + ln
        if typ == b"IEND":
            break

    raw = zlib.decompress(idat)
    assert len(raw) == (w * 4 + 1) * h, "tamano de la imagen raro"

    # El cubo se pinta de magenta: mucho rojo y mucho azul, poco verde.
    n = 0
    for y in range(h):
        fila = raw[y * (w * 4 + 1) + 1:(y + 1) * (w * 4 + 1)]
        for x in range(0, len(fila), 4):
            r, g, b = fila[x], fila[x + 1], fila[x + 2]
            if r >= 60 and b >= 60 and (r - g) >= 40 and (b - g) >= 30:
                n += 1
    print("imagen %dx%d, pixeles del cubo: %d" % (w, h, n))
    assert n > 0, "no hay pixeles del cubo en la captura"
    return 0


if __name__ == "__main__":
    sys.exit(main())
