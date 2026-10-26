#!/usr/bin/env python3
"""probe_xfv1.py — Un visor FALSO que habla el protocolo XFV1.

Sirve para BANCO: no hay visor OpenXR en el contenedor, asi que esto se
conecta al socket del motor (127.0.0.1:27020) y comprueba que llegan de
verdad:

  * los frames de pixeles (protocolo XFV1, formato 0);
  * los mensajes de GEOMETRIA (flag bit 1, formato 1): la lista de entidades
    que el motor dibuja, con su posicion y su modelo (canal del ADR-021).

Ademas escribe un fichero de ordenes en el directorio de la consola, que
el inyector vigila y pasa al stdin del motor (canal de acciones sin RCON,
ADR-020), para comprobar que la consola responde.

Uso:
    probe_xfv1.py [--puerto 27020] [--segundos 30] [--consola DIR]
                  [--orden "remote.call('xfactorvr','get_inventory')"]

Codigos de salida:
    0  llego al menos un frame de pixeles y uno de geometria
    1  no llego lo esperado
    2  no se pudo ni conectar
"""

import argparse
import os
import socket
import struct
import sys
import time

MAGIC = 0x31564658
FORMATO_RGBA8 = 0
FORMATO_GEOMETRIA = 1
FLAG_GEOMETRIA = 0x02


def log(msg):
    print(msg, flush=True)


def leer_exacto(sock, n):
    datos = b""
    while len(datos) < n:
        trozo = sock.recv(n - len(datos))
        if not trozo:
            return None
        datos += trozo
    return datos


def parsear_geometria(carga):
    """Devuelve (nombres, [(wx, wy, sx, sy, dir, modelo), ...])."""
    pos = 0
    (n_nombres,) = struct.unpack_from("<I", carga, pos)
    pos += 4
    nombres = []
    for _ in range(n_nombres):
        largo = carga[pos]
        pos += 1
        nombres.append(carga[pos : pos + largo].decode("utf-8", "replace"))
        pos += largo
    (n_entidades,) = struct.unpack_from("<I", carga, pos)
    pos += 4
    entidades = []
    for _ in range(n_entidades):
        wx, wy, sx, sy, direccion, modelo = struct.unpack_from("<ffffII", carga, pos)
        pos += 24
        entidades.append((wx, wy, sx, sy, direccion, modelo))
    return nombres, entidades



def escribir_png(ruta, ancho, alto, rgba):
    """Escribe un PNG RGBA minimo (sin dependencias)."""
    import zlib

    crudo = bytearray()
    for y in range(alto):
        crudo.append(0)  # filtro "none"
        crudo += rgba[y * ancho * 4 : (y + 1) * ancho * 4]

    def trozo(tipo, datos):
        return (
            struct.pack(">I", len(datos))
            + tipo
            + datos
            + struct.pack(">I", zlib.crc32(tipo + datos) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += trozo(b"IHDR", struct.pack(">IIBBBBB", ancho, alto, 8, 6, 0, 0, 0))
    png += trozo(b"IDAT", zlib.compress(bytes(crudo), 6))
    png += trozo(b"IEND", b"")
    with open(ruta, "wb") as f:
        f.write(png)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--puerto", type=int, default=27020)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--segundos", type=float, default=30.0)
    ap.add_argument("--consola", default=None, help="directorio de ordenes")
    ap.add_argument("--orden", default=None, help="orden a mandar por la consola")
    ap.add_argument(
        "--guardar",
        default=None,
        help="guarda el ultimo frame de pixeles recibido como PNG (para VERLO)",
    )
    args = ap.parse_args()

    limite = time.time() + args.segundos
    frames = 0
    geometrias = 0
    ojos = set()
    entidades_vistas = []
    nombres_vistos = []
    errores = []

    # En XFV1 el motor es el CLIENTE y el visor el SERVIDOR: aqui toca
    # ESCUCHAR, no conectar.
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        srv.bind(("0.0.0.0", args.puerto))
        srv.listen(1)
        srv.settimeout(args.segundos)
    except OSError as exc:
        log(f"NO SE PUDO ESCUCHAR en el puerto {args.puerto}: {exc}")
        return 2
    log(f"escuchando en {args.host}:{args.puerto} (esperando al motor)")
    try:
        sock, addr = srv.accept()
    except OSError as exc:
        log(f"el motor no se conecto: {exc}")
        return 2
    sock.settimeout(2.0)
    log(f"el motor se ha conectado desde {addr}")

    # La orden para la consola del motor (canal de acciones sin RCON).
    if args.consola and args.orden:
        try:
            os.makedirs(args.consola, exist_ok=True)
            ruta = os.path.join(args.consola, "pendiente-00000001.txt")
            with open(ruta, "w") as f:
                f.write(args.orden + "\n")
            log(f"orden dejada en {ruta}")
        except OSError as exc:
            log(f"no se pudo dejar la orden: {exc}")

    while time.time() < limite:
        try:
            cab = leer_exacto(sock, 16)
        except socket.timeout:
            continue
        except OSError as exc:
            errores.append(str(exc))
            break
        if cab is None:
            break
        magic, numero, w, h = struct.unpack_from("<IIHH", cab, 0)
        flags = cab[12]
        formato = cab[13]
        if magic != MAGIC:
            log(f"cabecera desconocida: 0x{magic:08x}")
            break
        if formato == FORMATO_GEOMETRIA:
            tam = 4 + 4  # como minimo: dos contadores
            # El tamaño exacto no viene en la cabecera: se lee a pelo.
            # Pero el emisor pone w = numero de nombres y h = numero de
            # entidades, asi que se puede leer de mas o de menos.
            resto = b""
            try:
                resto = sock.recv(65536)
            except socket.timeout:
                resto = b""
            # Se lee hasta tener la carga completa.
            carga = resto
            n_nombres = w
            n_entidades = h
            faltan = 4
            for _ in range(n_nombres):
                if len(carga) < faltan:
                    carga += leer_exacto(sock, faltan - len(carga)) or b""
                faltan = 1
                if len(carga) < faltan:
                    break
                largo = carga[faltan - 1]
                faltan = 1 + largo
            # Sencillo: se acumula hasta tener n_entidades*24 mas lo que falte.
            objetivo = 4 + n_entidades * 24
            while len(carga) < objetivo:
                trozo = leer_exacto(sock, 4096)
                if trozo is None:
                    break
                carga += trozo
            try:
                nombres, entidades = parsear_geometria(carga)
                geometrias += 1
                if not entidades_vistas:
                    entidades_vistas = entidades
                    nombres_vistos = nombres
                    log(
                        f"GEOMETRIA: {len(entidades)} entidades, "
                        f"{len(nombres)} prototipos: {nombres[:6]}"
                    )
                    for e in entidades[:5]:
                        log(
                            f"   wx={e[0]:.2f} wy={e[1]:.2f} "
                            f"sx={e[2]:.2f} sy={e[3]:.2f} dir={e[4]} modelo={e[5]}"
                        )
            except Exception as exc:  # noqa: BLE001
                errores.append(f"geometria: {exc}")
        else:
            tam = w * h * 4
            pix = leer_exacto(sock, tam)
            if pix is None:
                break
            frames += 1
            ojos.add(flags & 1)
            if frames == 1:
                log(f"PIXELES: primer frame {w}x{h} (ojo {flags & 1})")
            # El ultimo frame se guarda como PNG para poder VERLO (asi se
            # comprueba si lo que llega al visor trae el GUI del juego).
            if args.guardar:
                try:
                    escribir_png(args.guardar, w, h, pix)
                except OSError as exc:
                    log(f"no se pudo guardar el PNG: {exc}")

    log("")
    log("=== RESUMEN ===")
    log(f"frames de pixeles : {frames}  (ojos vistos: {sorted(ojos)})")
    log(f"mensajes de geometria: {geometrias}")
    if entidades_vistas:
        log(f"prototipos: {nombres_vistos[:8]}")
        log(f"ejemplo de entidad: {entidades_vistas[0]}")
    if errores:
        log(f"errores: {errores[:3]}")

    if frames >= 1 and geometrias >= 1:
        log("OK: llegan pixeles Y geometria por el canal XFV1")
        return 0
    if frames >= 1:
        log("AVISO: llegan pixeles pero NO geometria")
        return 1
    log("FALLO: no llego ningun frame")
    return 1


if __name__ == "__main__":
    sys.exit(main())
