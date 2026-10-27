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

# Ubuntu noble (24.04).  Se usa Ubuntu y no Debian por una razon concreta:
# con el Mesa de Debian 22.3 el vendor EGL (libEGL_mesa.so.0) NO exporta los
# simbolos egl*, asi que el despachador de glvnd se queda sin vendor y SDL2
# muere con "Could not find a valid EGL device to initialize".  Con el Mesa de
# Ubuntu 24.04 (25.2) la combinacion funciona: es exactamente la del
# contenedor, donde el cliente grafico abre ventana sin problema.
MIRROR = "http://ports.ubuntu.com/ubuntu-ports"
DIST = "noble"
# IMPORTANTE: noble-updates entra tambien.  Ahi estan el Mesa 25.x y el
# libllvm20 que pide la libgallium del Mesa de contenedores (que pide
# libLLVM.so.20.1).  Sin este suite el indice solo daba el Mesa 24.0.5 de
# noble, con su libLLVM-17, y el payload se quedaba sin la libreria que
# necesita el vendor EGL: "FALTAN librerias: libLLVM.so.20.1".
# Cada par es (suite, componente).
COMPONENTES = (
    ("noble", "main"),
    ("noble", "universe"),
    ("noble-updates", "main"),
    ("noble-updates", "universe"),
)
ARCH = "arm64"

# ---------------------------------------------------------------------------
# Mesa para CONTENEDORES en Android (lfdevs/mesa-for-android-container).
#
# POR QUE hace falta aparte del Mesa de Ubuntu:
#
#   En el visor, SELinux niega al dominio "shell" los nodos /dev/dri/*
#   (etiqueta graphics_device):
#       avc: denied { read } for name="renderD128"
#             scontext=u:r:shell:s0 tcontext=u:r:object_r:graphics_device:s0
#
#   La plataforma de devices de Mesa (la que usa el driver "offscreen" de
#   SDL2) enumera los devices por DRM: al no poder abrirlos, Mesa devuelve
#   CERO devices y SDL2 muere con
#       "Could not find a valid EGL device to initialize".
#
#   El Mesa de Ubuntu (el de arriba) NO abre contexto en el visor: medido,
#   0 devices con el mismo entorno que si funciona en el contenedor.
#   El fork de lfdevs si abre: trae los parches para contenedores en Android
#   (backend KGSL de freedreno).  Medido en la Pico 4 Ultra:
#       Initialised OpenGL:[1] zink Vulkan 1.4(Turnip Adreno (TM) 740v3
#                                      (MESA_TURNIP)); driver: 4.6 (Core Profile)
#
#   Ademas este Mesa declara el ICD de turnip con api_version 1.4.359 y su
#   libgallium pide libLLVM.so.20.1 (el libllvm20 de noble lo trae).
# ---------------------------------------------------------------------------
MESA_CONTENEDOR_VERSION = "26.3.0-devel-20260824"
MESA_CONTENEDOR_URL = (
    "https://github.com/lfdevs/mesa-for-android-container/releases/download/"
    f"mesa-{MESA_CONTENEDOR_VERSION}/"
    f"mesa-for-android-container_{MESA_CONTENEDOR_VERSION}_ubuntu_noble_arm64.tar.gz"
)

# Paquetes: runtime de Mesa con Zink, loader de Vulkan, lavapipe y glibc.
PAQUETES = [
    # glibc y acompanantes
    "libc6", "libgcc-s1", "libstdc++6", "zlib1g",
    # Mesa
    "libgl1", "libglx-mesa0", "libgl1-mesa-dri", "libgl1-mesa-glx",
    "libglapi-mesa", "libegl1", "libegl-mesa0", "libgbm1", "libdrm2", "libdrm-common",
    # glvnd: el runtime que despacha las llamadas de GL.  Sin el,
    # libGL.so.1 no se puede cargar (le falta libGLdispatch.so.0) y el
    # cliente grafico se queda sin contexto: "Could not initialize OpenGL".
    "libglvnd0", "libeglvnd0", "libglx0",
    # OpenGL ES 2.0 (Mesa): es lo que usa el driver "offscreen" de SDL2, que
    # es el unico que funciona en Android sin servidor X.  Sin esta libreria el
    # cliente grafico arranca pero muere con "Could not initialize OpenGL /
    # GLES library".
    "libgles2-mesa", "libgles2",
    "libvulkan1", "mesa-vulkan-drivers", "mesa-zink", "mesa-libgallium",
    "libllvm20", "libsensors5",
    "libxcb1", "libx11-6", "libxau6", "libxdmcp6", "libxext6", "libbsd0",
    "libmd0",
    # Dependencias DIRECTAS de zink_dri.so / libvulkan_*.so / libEGL_mesa.so.0.
    # El enlazador las exige en el momento de cargar la libreria, asi que si
    # falta una sola, el driver no carga y el cliente grafico muere con
    # "Could not initialize OpenGL / GLES library". Con estas, ademas, el
    # cliente puede abrir ventana de verdad en el visor (no solo en el
    # contenedor, que ya traia la pila completa de Ubuntu).
    "libexpat1", "libzstd1", "libelf1",
    "libdrm-radeon1", "libdrm-amdgpu1", "libdrm-nouveau2",
    "libxcb-dri3-0", "libxcb-dri2-0", "libxcb-present0", "libxcb-sync1",
    "libxcb-randr0", "libxcb-xfixes0", "libxcb-shm0", "libxcb-glx0",
    "libxshmfence1", "libx11-xcb1", "libxfixes3", "libxxf86vm1",
    "libwayland-client0", "libwayland-server0",
    # Dependencias de SEGUNDO NIVEL (las necesitan libLLVM-15, libwayland-*,
    # libffi...).  El enlazador tambien las exige al cargar, asi que si faltan
    # el vendor EGL de Mesa no llega a cargarse nunca.
    "libffi8", "libedit2", "libtinfo6", "libxml2", "libz3-4",
    "libicu74", "liblzma5",
    # SDL2 (el cliente lo lleva estatico, pero lo dejamos por si acaso)
    "libsdl2-2.0-0",
]

# Lo que hay que comprobar al final.
ESPERADOS = [
    # El driver de Zink va en el paquete propio mesa-zink.
    "usr/lib/aarch64-linux-gnu/dri/zink_dri.so",
    "usr/lib/aarch64-linux-gnu/libGL.so.1",
    "usr/lib/aarch64-linux-gnu/libEGL.so.1",
    # El vendor EGL de Mesa. Sin el, libEGL.so.1 (glvnd) no tiene a quien
    # repartir y el driver "offscreen" de SDL2 no puede crear contexto: eso
    # es lo que usa el cliente grafico en Android, donde no hay servidor X.
    "usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0",
    "usr/lib/aarch64-linux-gnu/libvulkan.so.1",
    # lavapipe: Vulkan por software, para validar sin GPU.
    "usr/lib/aarch64-linux-gnu/libvulkan_lvp.so",
    # OpenGL ES 2.0 de Mesa (para el driver "offscreen" de SDL2).
    "usr/lib/aarch64-linux-gnu/libGLESv2.so.2",
    # OJO: el ICD de lavapipe de Ubuntu se llama "lvp_icd.json" (sin sufijo de
    # arquitectura).  El que trae el Mesa de contenedores es el
    # "lvp_icd.aarch64.json".  Se comprueba aparte (AL_MENOS_UNO), porque solo
    # hace falta uno de los dos.
    # El linker de glibc (Debian lo deja en /lib).
    # La glibc no va en este payload: la aporta el runtime del visor
    # (prepare_glibc.sh), que es mas nueva y no choca con el motor grafico.
    "usr/lib/aarch64-linux-gnu/libgcc_s.so.1",
    # Mesa de contenedores en Android: el vendor EGL que SI abre contexto
    # sobre el Adreno del visor, y el ICD de turnip (Vulkan sobre KGSL).
    "usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so",
    # La libgallium del Mesa de contenedores la pide (libLLVM 20).
    "usr/lib/aarch64-linux-gnu/libLLVM.so.20.1",
    # El manifiesto de glvnd: sin el, el despachador no encuentra al vendor.
    "usr/share/glvnd/egl_vendor.d/50_mesa.json",
]

# Grupos en los que basta con que exista UNO de los ficheros.  El ICD de
# lavapipe lo llama Ubuntu "lvp_icd.json" y el Mesa de contenedores
# "lvp_icd.aarch64.json".
AL_MENOS_UNO = [
    ("usr/share/vulkan/icd.d/lvp_icd.json",
     "usr/share/vulkan/icd.d/lvp_icd.aarch64.json"),
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
    """Mapa paquete -> (version, ruta en el pool) para arm64.

    Se leen los componentes main y universe: los paquetes de Mesa estan en
    universe, y algunos de sus acompañantes en main.
    """
    out = {}
    for suite, comp in COMPONENTES:
        url = f"{MIRROR}/dists/{suite}/{comp}/binary-{ARCH}/Packages.gz"
        log(f"Descargando el indice de paquetes ({suite}/{comp}/{ARCH})...")
        try:
            crudo = descargar(url)
        except Exception as exc:  # noqa: BLE001
            log(f"  aviso: no se pudo descargar {suite}/{comp}: {exc}")
            continue
        txt = gzip.decompress(crudo).decode("utf-8", "replace")
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
            if name.endswith(".zst"):
                # Ubuntu 24.04 comprime los .deb con zstd y Python 3.13 no lo
                # trae de serie: se descomprime con la CLI de zstd.
                import subprocess
                import tempfile
                fd, tmp = tempfile.mkstemp(suffix=".tar")
                try:
                    os.write(fd, body)
                    os.close(fd)
                    with open(tmp + ".out", "wb") as salida:
                        # subprocess necesita la RUTA del fichero, no el
                        # fichero abierto (TypeError: expected str, bytes or
                        # os.PathLike object, not BufferedReader).
                        subprocess.run(["zstd", "-d", "-c", tmp], stdout=salida, check=True)
                    payload = open(tmp + ".out", "rb").read()
                finally:
                    for f in (tmp, tmp + ".out"):
                        if os.path.exists(f):
                            os.remove(f)
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


def podar_alias_dri(out):
    """Deja un solo nombre por fichero del megadriver de Mesa.

    El paquete de Debian trae los 43 drivers de DRI (radeonsi, zink, lima,
    panfrost...) como enlaces duros al MISMO binario de 24 MB.  En el host
    eso no ocupa nada, pero al empaquetar con tar se guardan como enlaces
    duros y al extraer en Android el `link()` falla con "Permission denied"
    (SELinux no deja crear enlaces duros en shell_data_file), dejando el
    payload a medias y el cliente grafico sin Zink.

    Se deja el driver que se usa (zink) y el resto de alias se CONVIERTE EN
    ENLACES SIMBOLICOS al mismo fichero: el contenido es identico, asi que lo
    unico que cambia es el nombre, y el nombre es justo lo que usa Mesa para
    decidir que driver carga ("swrast_dri.so" para el software, "zink_dri.so"
    para Zink).

    OJO (bug encontrado el 2026-10-23): antes los alias se BORRABAN.  En el
    visor eso dejaba a Mesa sin driver software, y como el visor no tiene
    /dev/dri/renderD* (solo card0/card1), eglQueryDevicesEXT no encontraba
    ningun device y SDL moria con "Could not find a valid EGL device to
    initialize".  Con el alias swrast_dri.so presente, Mesa ofrece el device
    software y el cliente grafico abre ventana."""
    dri = os.path.join(out, "usr", "lib", "aarch64-linux-gnu", "dri")
    if not os.path.isdir(dri):
        return 0
    # Se agrupa por inodo: un solo nombre puede sobreviver por inodo.
    por_inodo = {}
    for f in sorted(os.listdir(dri)):
        p = os.path.join(dri, f)
        if os.path.islink(p) or not os.path.isfile(p):
            continue
        try:
            st = os.stat(p)
        except OSError:
            continue
        por_inodo.setdefault((st.st_dev, st.st_ino), []).append(f)

    # 1. Se limpian los enlaces simbolicos que hubiera de una ronda anterior:
    #    pueden apuntar a un fichero que ya no existe (enlaces rotos).
    for f in sorted(os.listdir(dri)):
        p = os.path.join(dri, f)
        if os.path.islink(p):
            try:
                os.remove(p)
            except OSError:
                pass

    # 2. Se agrupan los ficheros reales por inodo (los alias del megadriver
    #    son el mismo contenido).
    por_inodo = {}
    for f in sorted(os.listdir(dri)):
        p = os.path.join(dri, f)
        if os.path.islink(p) or not os.path.isfile(p):
            continue
        try:
            st = os.stat(p)
        except OSError:
            continue
        por_inodo.setdefault((st.st_dev, st.st_ino), []).append(f)

    reemplazados = 0
    for _clave, nombres in por_inodo.items():
        if len(nombres) < 2:
            continue
        # El fichero real se llama SIEMPRE zink_dri.so (es el que se usa en el
        # visor); el resto de nombres pasan a ser enlaces a ese.
        elegido = "zink_dri.so"
        if elegido not in nombres:
            # Si el fichero real tiene otro nombre, se renombra.
            origen = os.path.join(dri, nombres[0])
            destino = os.path.join(dri, elegido)
            if not os.path.lexists(destino):
                try:
                    os.rename(origen, destino)
                    nombres = [elegido] + [n for n in nombres if n != nombres[0]]
                except OSError:
                    elegido = nombres[0]
        for f in nombres:
            if f == elegido:
                continue
            destino = os.path.join(dri, f)
            try:
                if os.path.lexists(destino):
                    os.remove(destino)
                os.symlink(elegido, destino)
                reemplazados += 1
            except OSError:
                pass
    if reemplazados:
        log(f"Convertidos {reemplazados} alias del megadriver en enlaces simbolicos")

    # 3. El megadriver de Ubuntu (25.2) llega con los drivers como enlaces
    #    simbolicos a "libdril_dri.so" (zink_dri.so -> libdril_dri.so).  El
    #    paso 1 de esta misma funcion BORRA todos los enlaces simbolicos (para
    #    limpiar los rotos de una ronda anterior), asi que se lleva por
    #    delante justamente los nombres que Mesa necesita para elegir driver:
    #    sin "zink_dri.so" no hay Zink y el cliente grafico no abre contexto.
    #    Se vuelven a crear los nombres que importan.
    reales = [f for f in os.listdir(dri)
              if os.path.isfile(os.path.join(dri, f))
              and not os.path.islink(os.path.join(dri, f))]
    if not reales:
        return reemplazados
    destino_real = "zink_dri.so" if "zink_dri.so" in reales else reales[0]
    creados = 0
    for nombre in ("zink_dri.so", "swrast_dri.so"):
        p = os.path.join(dri, nombre)
        if os.path.lexists(p):
            continue
        try:
            os.symlink(destino_real, p)
            creados += 1
        except OSError:
            pass
    if creados:
        log(f"Recreados {creados} nombres del megadriver "
            f"(zink_dri.so / swrast_dri.so -> {destino_real})")
    return reemplazados


def enlaces_duros(out):
    """Devuelve los grupos de ficheros que comparten inodo (enlaces duros)."""
    grupos = {}
    for r, _ds, fs in os.walk(out):
        for f in fs:
            p = os.path.join(r, f)
            try:
                st = os.lstat(p)
            except OSError:
                continue
            if st.st_nlink > 1:
                grupos.setdefault(st.st_ino, []).append(p)
    return [g for g in grupos.values() if len(g) > 1]


def empaquetar_tar(out, ruta_tar):
    """Empaqueta el payload en un .tar.gz que Android pueda extraer.

    En Android el `tar` de toybox no puede crear enlaces duros: falla con
    "Permission denied" (SELinux no lo permite sobre shell_data_file) y deja
    el payload a medias.  Por eso, antes de empaquetar, se comprueba que no
    quede ni un enlace duro en el payload; si queda, se corta el empaquetado
    en vez de subir un tar que no se va a poder extraer.

    Los symlinks se dejan como symlinks: el payload tiene un farm de ellos
    (libGL.so.1 -> libGL.so.1.7.0, /usr/share/doc/...) y toybox los crea sin
    problema.  Con dereference=True, ademas, Python reventaba con los symlinks
    rotos que apuntan fuera del paquete."""
    duros = enlaces_duros(out)
    if duros:
        raise RuntimeError(
            "el payload todavia tiene %d grupos de enlaces duros (ej. %s). "
            "Ejecuta la poda antes de empaquetar."
            % (len(duros), duros[0][0]))

    import tarfile
    base = os.path.dirname(out.rstrip("/"))
    nombre = os.path.basename(out.rstrip("/"))
    with tarfile.open(ruta_tar, "w:gz") as tf:
        tf.add(out, arcname=nombre, recursive=True)
    tam = os.path.getsize(ruta_tar)
    log(f"Empaquetado {ruta_tar} ({tam/1e6:.1f} MB, sin enlaces duros)")


def podar_glibc_debian(out):
    """Quita la glibc de Debian del payload de Mesa.

    El payload trae libc6, libgcc-s1 y libstdc++6 de Debian (bookworm, glibc
    2.36).  Esa libc PISA la del runtime de glibc del visor (Termux, 2.44) y
    entonces el cliente grafico de Factorio, que pide GLIBC_2.38, falla con
    "version `GLIBC_2.38' not found".

    La glibc que se usa es la del runtime del visor, asi que aqui solo se
    deja lo que ese runtime no trae: libgcc_s.so.1.

    OJO (bug encontrado el 2026-10-23): antes este paso BORRABA el directorio
    lib/ entero.  En Debian con /usr fusionado, muchos paquetes siguen
    instalando ahi: zlib1g pone libz.so.1 y libexpat1 pone libexpat.so.1.
    Al borrar el directorio se perdian, el driver no cargaba y el cliente
    grafico moria con "Could not initialize OpenGL / GLES library" sin decir
    cual era el problema de verdad.

    Ahora: lo que NO es glibc se muda a usr/lib (que es donde esta el camino
    de busqueda) y lo que SI es glibc se borra.
    """
    # Archivos que son de la glibc de Debian (y sus enlaces).
    glibc = {
        "libc.so.6", "libm.so.6", "libmvec.so.1", "libpthread.so.0",
        "libdl.so.2", "librt.so.1", "libresolv.so.2", "libanl.so.1",
        "libnsl.so.1", "libutil.so.1", "libBrokenLocale.so.1",
        "libthread_db.so.1", "ld-linux-aarch64.so.1", "libcrypt.so.1",
        "libnss_compat.so.2", "libnss_db.so.2", "libnss_dns.so.2",
        "libnss_files.so.2", "libnss_hesiod.so.2", "libnss_nisplus.so.2",
        "libnss_nis.so.2", "libnss_sss.so.2",
    }
    destino = os.path.join(out, "usr/lib/aarch64-linux-gnu")
    os.makedirs(destino, exist_ok=True)
    mudados = 0
    borrados = 0
    for base in ("lib/aarch64-linux-gnu", "lib64", "lib"):
        carpeta = os.path.join(out, base)
        if not os.path.isdir(carpeta):
            continue
        for nombre in sorted(os.listdir(carpeta)):
            origen = os.path.join(carpeta, nombre)
            if not os.path.isfile(origen) and not os.path.islink(origen):
                continue
            # Para un enlace, lo que decide es el destino.
            clave = os.path.basename(os.readlink(origen)) if os.path.islink(origen) else nombre
            if clave in glibc or nombre in glibc:
                os.remove(origen)
                borrados += 1
                continue
            destino_final = os.path.join(destino, nombre)
            if os.path.lexists(destino_final):
                os.remove(destino_final)
            shutil.move(origen, destino_final)
            mudados += 1
        # Se quitan los directorios vacios que quedan.
        for r, dirs, files in os.walk(carpeta, topdown=False):
            if not os.listdir(r):
                os.rmdir(r)
    if mudados or borrados:
        log(f"  glibc quitada ({borrados} ficheros); {mudados} mudados a usr/lib")

    # libgcc_s.so.1 se conserva (la glibc la aporta el runtime del visor).
    for base in ("usr/lib/aarch64-linux-gnu", "lib/aarch64-linux-gnu"):
        p = os.path.join(out, base, "libgcc_s.so.1")
        if os.path.exists(p):
            destino_final = os.path.join(out, "usr/lib/aarch64-linux-gnu", "libgcc_s.so.1")
            if os.path.abspath(p) != os.path.abspath(destino_final):
                shutil.copy2(p, destino_final)
            break
    log("  glibc de Debian quitada del payload de Mesa")


def dependencias_needed(ruta):
    """Lista las librerias DT_NEEDED de un ELF (sin herramientas externas)."""
    import struct
    with open(ruta, "rb") as f:
        datos = f.read()
    if datos[:4] != b"\x7fELF":
        return []
    shoff, = struct.unpack_from("<Q", datos, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", datos, 0x3A)

    def sh(i):
        return struct.unpack_from("<IIQQQQIIQQ", datos, shoff + i * shentsize)

    strtab = sh(shstrndx)[4]
    secciones = {}
    for i in range(shnum):
        s = sh(i)
        secciones[datos[strtab + s[0]:].split(b"\0")[0].decode()] = s
    if ".dynamic" not in secciones or ".dynstr" not in secciones:
        return []
    dynstr = secciones[".dynstr"][4]
    salida = []
    dy = secciones[".dynamic"]
    for off in range(dy[4], dy[4] + dy[5], 16):
        tag, val = struct.unpack_from("<qQ", datos, off)
        if tag == 0:
            break
        if tag == 1:
            fin = datos.index(b"\0", dynstr + val)
            salida.append(datos[dynstr + val:fin].decode())
    return salida


# Lo que aporta el runtime de glibc del visor (prepare_glibc.sh), no el payload.
DE_LA_GLIBC = {
    "libc.so.6", "libm.so.6", "libpthread.so.0", "libdl.so.2", "librt.so.1",
    "libresolv.so.2", "libgcc_s.so.1", "ld-linux-aarch64.so.1",
    "libnsl.so.1", "libutil.so.1",
}

# Librerias criticas: si alguna de estas no carga, el cliente grafico no
# puede crear contexto y muere con "Could not initialize OpenGL".
CRITICAS = [
    "usr/lib/aarch64-linux-gnu/dri/zink_dri.so",
    "usr/lib/aarch64-linux-gnu/dri/swrast_dri.so",
    "usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0",
    "usr/lib/aarch64-linux-gnu/libGLX_mesa.so.0",
    "usr/lib/aarch64-linux-gnu/libGL.so.1",
    "usr/lib/aarch64-linux-gnu/libEGL.so.1",
    "usr/lib/aarch64-linux-gnu/libvulkan.so.1",
    "usr/lib/aarch64-linux-gnu/libvulkan_lvp.so",
    "usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so",
]


def verificar_dependencias(out):
    """Comprueba que todas las DT_NEEDED de las librerias criticas esten.

    Esto es lo que faltaba: el payload se construia sin libexpat, libz, la
    pila xcb/wayland... y el enlazador se quejaba de "cannot open shared
    object file" al cargar el driver, que el motor traducía a un
    "Could not initialize OpenGL" sin decir cual era el problema de verdad.
    """
    base = os.path.join(out, "usr/lib/aarch64-linux-gnu")

    # Se indexan todas las librerias del payload por nombre (con sus enlaces).
    por_nombre = {}
    for raiz, _dirs, files in os.walk(out):
        for f in files:
            p = os.path.join(raiz, f)
            por_nombre.setdefault(f, p)
        for d in os.listdir(raiz):
            p = os.path.join(raiz, d)
            if os.path.islink(p):
                por_nombre.setdefault(os.path.basename(d), p)

    faltan = {}

    def visita(ruta, origen, nivel=0):
        if nivel > 8:
            return
        for dep in dependencias_needed(ruta):
            if dep in DE_LA_GLIBC:
                continue
            destino = por_nombre.get(dep)
            if destino is None:
                faltan.setdefault(dep, []).append(origen)
                continue
            # Tambien se comprueban las dependencias de segundo nivel: el
            # vendor EGL de Mesa, por ejemplo, no carga si libLLVM no puede
            # cargar libffi.
            visita(destino, origen, nivel + 1)

    for rel in CRITICAS:
        ruta = os.path.join(out, rel)
        if not os.path.exists(ruta):
            continue
        visita(ruta, os.path.basename(rel))
    if faltan:
        log("FALTAN librerias en el payload (el driver no cargara):")
        for dep, usos in sorted(faltan.items()):
            log(f"    {dep}  (la necesitan: {', '.join(sorted(set(usos)))})")
        log("  anadelas a PAQUETES y vuelve a construir el payload.")
        return False
    log("Dependencias de las librerias criticas: todas presentes.")
    return True


def aplicar_mesa_contenedor(out, cache):
    """Superpone el Mesa de contenedores en Android sobre el payload.

    Se hace DESPUES de podar: la poda convierte los alias del megadriver de
    DRI en enlaces a zink_dri.so, y el Mesa de contenedores trae sus
    propios drivers (kgsl_dri.so, msm_dri.so) que hay que respetar.

    Se superpone (no borra) lo que ya hay: el payload conserva glvnd, el
    loader de Vulkan y las librerias del sistema de Ubuntu noble.
    """
    destino = os.path.join(cache, f"mesa-contenedor-{MESA_CONTENEDOR_VERSION}.tar.gz")
    if not os.path.exists(destino):
        log(f"  descargando Mesa de contenedores {MESA_CONTENEDOR_VERSION}...")
        descargar(MESA_CONTENEDOR_URL, destino)

    tmp = os.path.join(cache, "mesa_contenedor_tmp")
    if os.path.isdir(tmp):
        shutil.rmtree(tmp)
    os.makedirs(tmp, exist_ok=True)
    with tarfile.open(destino, "r:gz") as tf:
        tf.extractall(tmp)

    src = os.path.join(tmp, "usr")
    if not os.path.isdir(src):
        raise RuntimeError("el tar del Mesa de contenedores no trae usr/")
    shutil.copytree(src, os.path.join(out, "usr"), dirs_exist_ok=True)

    # El ICD de turnip se deja con la ruta de Ubuntu: el launcher del visor
    # reescribe una copia con la ruta del dispositivo (turnip_icd.json).
    log("  Mesa de contenedores superpuesto (vendor EGL + turnip + gallium).")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="build/mesa")
    ap.add_argument("--cache", default="build/debs")
    ap.add_argument("--no-download", action="store_true")
    ap.add_argument("--no-tar", action="store_true",
                    help="no empaquetar el payload en un .tar.gz")
    ap.add_argument("--solo-tar", action="store_true",
                    help="no reextraer los .deb: solo podar y empaquetar lo que "
                         "ya hay en el payload (para redesplegar)")
    ap.add_argument("--tar", default="build/mesa.tar.gz",
                    help="ruta del .tar.gz que se genera (por defecto "
                         "build/mesa.tar.gz)")
    ap.add_argument("--sin-contenedor", action="store_true",
                    help="no superponer el Mesa de contenedores (26.3.0-devel) "
                         "sobre el de Ubuntu.  El Mesa de contenedores trae "
                         "zink/gallium de la 26.3.0 pero NO trae lavapipe, que "
                         "si viene de Ubuntu: el resultado es un zink 26.3 "
                         "hablando con un lavapipe 25.2, y Zink rechaza el "
                         "dispositivo con 'failed to choose pdev' (el cliente "
                         "muerte con 'Could not find a valid EGL device').")
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

    if args.solo_tar:
        log("Modo --solo-tar: no se reextraen los .deb")
    else:
        log("Extrayendo los paquetes...")
        os.makedirs(out, exist_ok=True)
        for deb in sorted(os.listdir(cache)):
            if not deb.endswith(".deb"):
                continue
            try:
                extraer_deb(os.path.join(cache, deb), out)
            except Exception as exc:  # noqa: BLE001
                log(f"  aviso: no se pudo extraer {deb}: {exc}")

    log("Podando los alias del megadriver de DRI...")
    podar_alias_dri(out)
    log("Quitando la glibc de Debian (la del visor es mas nueva)...")
    podar_glibc_debian(out)

    if args.sin_contenedor:
        log("Omitiendo el Mesa de contenedores (--sin-contenedor):")
        log("  el payload se queda con el Mesa de Ubuntu, todo de la misma version.")
    else:
        log("Superponiendo el Mesa de contenedores en Android...")
        try:
            aplicar_mesa_contenedor(out, cache)
        except Exception as exc:  # noqa: BLE001
            log(f"  AVISO: no se pudo aplicar el Mesa de contenedores: {exc}")
            log("  Sin el, el cliente grafico NO abre contexto EGL en el visor.")

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

    # El ICD de lavapipe: vale cualquiera de los dos nombres (el de Ubuntu y
    # el del Mesa de contenedores).  El cliente reescribe el JSON con la ruta
    # del dispositivo, asi que solo importa que la libreria este.
    for grupo in AL_MENOS_UNO:
        presentes = [g for g in grupo if os.path.exists(os.path.join(out, g))]
        if presentes:
            log(f"  OK      ICD de lavapipe: {presentes[0]}")
        else:
            log(f"  AUSENTE ICD de lavapipe (vale cualquiera de: {', '.join(grupo)})")
            faltan.append(grupo[0])

    log("")
    log("Comprobando las dependencias de las librerias criticas:")
    if not verificar_dependencias(out):
        return 1

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
    if not args.no_tar:
        ruta_tar = os.path.join(raiz, args.tar) if not os.path.isabs(args.tar) else args.tar
        empaquetar_tar(out, ruta_tar)
    log("El payload tiene Mesa con Zink, el loader de Vulkan y glibc.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
