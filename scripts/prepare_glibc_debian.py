#!/usr/bin/env python3
"""prepare_glibc_debian.py — Runtime de glibc portable para el visor Pico 4 Ultra.

El motor ARM64 de Factorio es un binario de Linux y Android usa bionic.  Sin
un glibc no arranca, pero hay dos trampas:

1. El glibc de Termux NO es portable: lleva dentro rutas absolutas a
   /data/data/com.termux/files/usr/glibc/..., asi que al copiarlo a otro
   sitio (el sandbox de la app) deja de encontrar sus propias librerias.

2. El cliente GRAFICO de Factorio 2.1.22 pide GLIBC_2.38, y el glibc de
   bookworm (2.36) no llega.  Por eso se usa el de trixie (2.41), que si.

Este script descarga de Debian trixie los paquetes libc6, libgcc-s1 y
libstdc++6 para arm64 y los deja aplanados en build/glibc/ (todo en lib/,
que es donde el linker busca).

Salida: build/glibc.tar.gz (el despliegue lo sube a /data/local/tmp/glibc).
"""

import argparse
import gzip
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.request

MIRROR = "https://deb.debian.org/debian"
DIST = "trixie"
ARCH = "arm64"
PAQUETES = ["libc6", "libgcc-s1", "libstdc++6"]
INDICE = "%s/dists/%s/main/binary-%s/Packages.gz" % (MIRROR, DIST, ARCH)


def log(mensaje):
    print("[glibc] %s" % mensaje)


def indice_paquetes():
    """Devuelve {nombre: (fichero, sha256)} del indice de Debian."""
    url = INDICE
    log("descargando el indice de %s..." % DIST)
    with urllib.request.urlopen(url) as respuesta:
        datos = gzip.decompress(respuesta.read()).decode("utf-8", "replace")
    salida = {}
    for bloque in datos.split("\n\n"):
        nombre = None
        fichero = None
        sha = None
        for linea in bloque.split("\n"):
            if linea.startswith("Package: "):
                nombre = linea[len("Package: "):].strip()
            elif linea.startswith("Filename: "):
                fichero = linea[len("Filename: "):].strip()
            elif linea.startswith("SHA256: "):
                sha = linea[len("SHA256: "):].strip()
        if nombre in PAQUETES and fichero:
            salida[nombre] = (fichero, sha)
    return salida


def descargar(url, destino):
    with urllib.request.urlopen(url) as respuesta, open(destino, "wb") as fh:
        shutil.copyfileobj(respuesta, fh)


def extraer_deb(deb, destino):
    """Extrae un .deb (ar + data.tar.*) en destino.

    Un .deb es un fichero "ar" con dentro data.tar.*, no un tar: hay que
    leer la cabecera ar a mano."""
    import io

    data = open(deb, "rb").read()
    if data[:8] != b"!<arch>\n":
        raise RuntimeError("%s no es un .deb valido" % deb)
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
        raise RuntimeError("%s no contiene data.tar" % deb)

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


def aplanar(origen, destino):
    """Deja todas las librerias juntas en destino/lib."""
    lib = os.path.join(destino, "lib")
    os.makedirs(lib, exist_ok=True)
    movidas = 0
    for base, _dirs, ficheros in os.walk(origen):
        for nombre in ficheros:
            origen_fichero = os.path.join(base, nombre)
            # Los directorios con la version (libc.so.6 -> libc-2.41.so) se
            # resuelven con symlinks, asi que se copian los ficheros reales.
            if os.path.islink(origen_fichero):
                continue
            destino_fichero = os.path.join(lib, nombre)
            if os.path.exists(destino_fichero) or os.path.islink(destino_fichero):
                continue
            shutil.copy2(origen_fichero, destino_fichero)
            movidas += 1
    return movidas


def empaquetar(destino, ruta_tar):
    with tarfile.open(ruta_tar, "w:gz", dereference=True) as tf:
        tf.add(destino, arcname="glibc", recursive=True)
    log("empaquetado %s (%.1f MB)" % (ruta_tar, os.path.getsize(ruta_tar) / 1e6))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="build/glibc")
    ap.add_argument("--cache", default="build/debs")
    ap.add_argument("--tar", default="build/glibc.tar.gz")
    ap.add_argument("--solo-tar", action="store_true")
    args = ap.parse_args()

    raiz = os.path.abspath(os.path.dirname(os.path.dirname(__file__)))
    out = os.path.join(raiz, args.out) if not os.path.isabs(args.out) else args.out
    cache = os.path.join(raiz, args.cache) if not os.path.isabs(args.cache) else args.cache
    ruta_tar = os.path.join(raiz, args.tar) if not os.path.isabs(args.tar) else args.tar
    os.makedirs(cache, exist_ok=True)

    if args.solo_tar:
        log("modo --solo-tar: no se vuelven a descargar los paquetes")
    else:
        indice = indice_paquetes()
        faltan = [p for p in PAQUETES if p not in indice]
        if faltan:
            log("AVISO: paquetes no encontrados: %s" % ", ".join(faltan))
        stage = os.path.join(cache, "glibc_stage")
        if os.path.isdir(stage):
            shutil.rmtree(stage)
        os.makedirs(stage, exist_ok=True)
        for paquete in PAQUETES:
            if paquete not in indice:
                continue
            fichero, _sha = indice[paquete]
            deb = os.path.join(cache, os.path.basename(fichero))
            if not os.path.exists(deb):
                log("descargando %s..." % os.path.basename(fichero))
                descargar("%s/%s" % (MIRROR, fichero), deb)
            log("extrayendo %s" % os.path.basename(fichero))
            extraer_deb(deb, stage)

        # Aplanar: todo en out/lib.
        if os.path.isdir(out):
            shutil.rmtree(out)
        os.makedirs(out, exist_ok=True)
        movidas = aplanar(stage, out)
        log("%d librerias en %s/lib" % (movidas, out))

    loader = os.path.join(out, "lib", "ld-linux-aarch64.so.1")
    if not os.path.exists(loader):
        log("ERROR: no esta el linker en %s" % loader)
        return 1
    libc = os.path.join(out, "lib", "libc.so.6")
    if os.path.exists(libc):
        import subprocess as sp
        version = sp.run(["strings", "-a", libc], capture_output=True, text=True).stdout
        for linea in version.split("\n"):
            if "GNU C Library" in linea:
                log(linea.strip())
                break
    empaquetar(out, ruta_tar)
    return 0


if __name__ == "__main__":
    sys.exit(main())
