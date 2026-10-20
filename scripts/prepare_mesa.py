#!/usr/bin/env python3
"""prepare_mesa.py — Payload de Mesa (con Zink) + glibc de Debian para ARM64.

El cliente grafico de Factorio pide OpenGL 3.3 core. En Android no hay OpenGL
de escritorio, pero Zink (Mesa) da OpenGL de escritorio sobre Vulkan, de modo
que el cliente puede correr sobre el driver Vulkan de Adreno con aceleracion
por hardware.

Este script deja en build/mesa/ un subarbol de Debian arm64 con:
  - glibc (libc6, libstdc++6, libgcc-s1) para el cliente grafico
  - Mesa: libGL.so.1 (glvnd), zink_dri.so, libEGL.so.1, libvulkan.so.1
  - lavapipe (Vulkan por software) para poder validar sin GPU
  - SDL2 por si hace falta

Despues se empaqueta en build/mesa.tar.gz para desplegarlo en el visor.

Uso:
    python3 scripts/prepare_mesa.py [--out build/mesa] [--no-download]
"""

import argparse
import gzip
import io
import os
import re
import shutil
import sys
import tarfile
import urllib.request

MIRROR = "https://deb.debian.org/debian"
DIST = "bookworm"
ARCH = "arm64"

# Paquetes: runtime de Mesa con Zink, loader de Vulkan, lavapipe y glibc.
PAQUETES = [
    # glibc y acompanantes
    "libc6", "libgcc-s1", "libstdc++6", "zlib1g",
    # Mesa
    "libgl1", "libglx-mesa0", "libgl1-mesa-dri", "libgl1-mesa-glx",
    "libglapi-mesa", "libegl1", "libgbm1", "libdrm2", "libdrm-common",
    "libvulkan1", "mesa-vulkan-drivers", "mesa-zink", "libllvm15", "libsensors5",
    "libxcb1", "libx11-6", "libxau6", "libxdmcp6", "libxext6", "libbsd0",
    "libmd0",
    # SDL2 (el cliente lo lleva estatico, pero lo dejamos por si acaso)
    "libsdl2-2.0-0",
]

# Lo que hay que comprobar al final.
ESPERADOS = [
    # El driver de Zink va en el paquete propio mesa-zink.
    "usr/lib/aarch64-linux-gnu/dri/zink_dri.so",
    "usr/lib/aarch64-linux-gnu/libGL.so.1",
    "usr/lib/aarch64-linux-gnu/libEGL.so.1",
    "usr/lib/aarch64-linux-gnu/libvulkan.so.1",
    # lavapipe: Vulkan por software, para validar sin GPU.
    "usr/lib/aarch64-linux-gnu/libvulkan_lvp.so",
    "usr/share/vulkan/icd.d/lvp_icd.aarch64.json",
    # El linker de glibc (Debian lo deja en /lib).
    "lib/ld-linux-aarch64.so.1",
    "lib/aarch64-linux-gnu/libc.so.6",
]


def log(msg):
    print(msg, flush=True)


def descargar(url, destino=None):
    with urllib.request.urlopen(url, timeout=180) as r:
        datos = r.read()
    if destino:
        with open(destino, "wb") as fh:
            fh.write(datos)
    return datos


def indice_dependencias():
    """Mapa paquete -> (version, ruta en el pool) para arm64."""
    url = f"{MIRROR}/dists/{DIST}/main/binary-{ARCH}/Packages.gz"
    log(f"Descargando el indice de paquetes ({DIST}/{ARCH})...")
    txt = gzip.decompress(descargar(url)).decode("utf-8", "replace")
    out = {}
    for bloque in txt.split("\n\n"):
        m = re.search(r"^Package: (\S+)$", bloque, re.M)
        if not m:
            continue
        f = re.search(r"^Filename: (\S+)$", bloque, re.M)
        v = re.search(r"^Version: (\S+)$", bloque, re.M)
        if f:
            out[m.group(1)] = (v.group(1) if v else "?", f.group(1))
    return out


def extraer_deb(ruta_deb, destino):
    """Extrae un .deb (ar + data.tar.*) en destino."""
    data = open(ruta_deb, "rb").read()
    if data[:8] != b"!<arch>\n":
        raise RuntimeError(f"{ruta_deb} no es un .deb valido")
    off = 8
    payload = None
    while off < len(data):
        hdr = data[off:off + 60]
        if len(hdr) < 60:
            break
        name = hdr[0:16].decode().strip().rstrip("/")
        size = int(hdr[48:58].decode().strip())
        body = data[off + 60:off + 60 + size]
        if name.startswith("data.tar"):
            payload = body
        off += 60 + size + (size % 2)
    if payload is None:
        raise RuntimeError(f"{ruta_deb} no contiene data.tar")
    enlaces = []
    with tarfile.open(fileobj=io.BytesIO(payload)) as tf:
        for m in tf.getmembers():
            rel = m.name.lstrip("./")
            if not rel or rel.startswith("/"):
                continue
            target = os.path.join(destino, rel)
            if m.isdir():
                os.makedirs(target, exist_ok=True)
            elif m.isfile():
                os.makedirs(os.path.dirname(target), exist_ok=True)
                src = tf.extractfile(m)
                if src:
                    with open(target, "wb") as dst:
                        shutil.copyfileobj(src, dst)
                os.chmod(target, (m.mode & 0o777) or 0o644)
            elif m.issym():
                os.makedirs(os.path.dirname(target), exist_ok=True)
                if os.path.lexists(target):
                    os.remove(target)
                os.symlink(m.linkname, target)
            elif m.islnk():
                # Los drivers de DRI vienen como hardlinks a un unico
                # fichero (link farm del .deb): sin resolverlos, el driver
                # de Zink no aparece; si se copian, el payload se multiplica
                # por 43. Se resuelven en una segunda pasada, cuando el
                # fichero original ya esta en su sitio.
                enlaces.append((target, m.linkname.lstrip("./")))
    for target, origen_rel in enlaces:
        os.makedirs(os.path.dirname(target), exist_ok=True)
        origen = os.path.join(destino, origen_rel)
        if not os.path.exists(origen):
            continue
        if os.path.lexists(target):
            os.remove(target)
        try:
            os.link(origen, target)
        except OSError:
            shutil.copyfile(origen, target)
            os.chmod(target, 0o755)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="build/mesa")
    ap.add_argument("--cache", default="build/debs")
    ap.add_argument("--no-download", action="store_true")
    args = ap.parse_args()

    raiz = os.path.abspath(os.path.dirname(os.path.dirname(__file__)))
    out = os.path.join(raiz, args.out) if not os.path.isabs(args.out) else args.out
    cache = os.path.join(raiz, args.cache) if not os.path.isabs(args.cache) else args.cache
    os.makedirs(cache, exist_ok=True)

    if not args.no_download:
        idx = indice_dependencias()
        faltan = [p for p in PAQUETES if p not in idx]
        if faltan:
            log(f"Paquetes no encontrados en el indice: {', '.join(faltan)}")
        for p in PAQUETES:
            if p not in idx:
                continue
            ver, ruta = idx[p]
            deb = os.path.join(cache, os.path.basename(ruta))
            if not os.path.exists(deb):
                log(f"  descargando {p} {ver}...")
                descargar(f"{MIRROR}/{ruta}", deb)
    else:
        idx = {}

    log("Extrayendo los paquetes...")
    os.makedirs(out, exist_ok=True)
    for deb in sorted(os.listdir(cache)):
        if not deb.endswith(".deb"):
            continue
        try:
            extraer_deb(os.path.join(cache, deb), out)
        except Exception as exc:  # noqa: BLE001
            log(f"  aviso: no se pudo extraer {deb}: {exc}")

    log("")
    log("Comprobando los ficheros clave:")
    faltan = []
    for rel in ESPERADOS:
        p = os.path.join(out, rel)
        ok = os.path.exists(p)
        tam = os.path.getsize(p) if ok else 0
        log(f"  {'OK     ' if ok else 'AUSENTE'} {rel}  ({tam/1e6:.1f} MB)")
        if not ok:
            faltan.append(rel)

    # Se cuentan inodes unicos: los drivers de DRI son hardlinks del mismo
    # fichero y sumarlos todos multiplicaria el tamano por 43.
    vistos = set()
    total = 0
    for r, _, fs in os.walk(out):
        for f in fs:
            p = os.path.join(r, f)
            if os.path.islink(p):
                continue
            try:
                st = os.stat(p)
            except OSError:
                continue
            if st.st_ino in vistos:
                continue
            vistos.add(st.st_ino)
            total += st.st_size
    log("")
    log(f"Payload: {out}  ({total/1e6:.1f} MB)")
    if faltan:
        log("Faltan: " + ", ".join(faltan))
        return 1
    log("El payload tiene Mesa con Zink, el loader de Vulkan y glibc.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
