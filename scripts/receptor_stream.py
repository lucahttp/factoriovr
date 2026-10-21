#!/usr/bin/env python3
"""receptor_stream.py — Receptor de los frames que manda el inyector.

Implementa el protocolo "XFV1" (ver `injector-linux/src/stream.rs`):

    cabecera (16 bytes, little-endian)
        0..4    magic   u32 = 0x31564658  ("XFV1")
        4..8    numero de frame  u32
        8..10   ancho   u16
        10..12  alto    u16
        12      flags   u8
        13      formato u8 (0 = RGBA8888)
        14..16  reservado u16
    carga: ancho * alto * 4 bytes RGBA

El script se usa de dos formas:

  * en el banco (`scripts/test_stream_render.sh`): cuenta los frames que
    llegan y busca los pixeles del cubo 3D (magenta) en la imagen, que es la
    prueba de que el render REAL del juego llega al visor;
  * como referencia del protocolo para el receptor del APK.

Escribe un JSON de resumen (--salida) que se actualiza con cada frame, para
que el banco pueda leerlo aunque el proceso este vivo.
"""

import argparse
import json
import os
import socket
import struct
import sys
import time
import zlib

MAGIC = 0x31564658
CABECERA = 16
FORMATO_RGBA8 = 0


def pixeles_del_cubo(pix, ancho):
    """Pixeles con el color del cubo 3D (magenta) y su caja.

    Mismo criterio que el inyector: mucho rojo y mucho azul, poco verde.
    En la escena de Factorio no hay nada de ese color.

    Devuelve (cuantos, x_min, x_max, y_min, y_max).  La caja sirve para
    comprobar el PARALAJE: con la camara desplazada por ojo, el cubo sale en
    una posicion distinta en la vista izquierda que en la derecha.
    """
    n = 0
    x_min = None
    x_max = None
    y_min = None
    y_max = None
    for i in range(0, len(pix) - 3, 4):
        r, g, b = pix[i], pix[i + 1], pix[i + 2]
        if r >= 60 and b >= 60 and (r - g) >= 40 and (b - g) >= 30:
            n += 1
            x = (i // 4) % ancho
            y = (i // 4) // ancho
            if x_min is None or x < x_min:
                x_min = x
            if x_max is None or x > x_max:
                x_max = x
            if y_min is None or y < y_min:
                y_min = y
            if y_max is None or y > y_max:
                y_max = y
    return n, x_min, x_max, y_min, y_max


def escribir_png(ruta, w, h, pix):
    """Escribe un PNG RGBA (zlib + CRC propio, sin dependencias)."""
    crudo = bytearray()
    for y in range(h):
        crudo.append(0)  # filtro "none"
        crudo.extend(pix[y * w * 4:(y + 1) * w * 4])

    def bloque(tipo, datos):
        return (struct.pack(">I", len(datos)) + tipo + datos
                + struct.pack(">I", zlib.crc32(tipo + datos) & 0xFFFFFFFF))

    png = b"\x89PNG\r\n\x1a\n"
    png += bloque(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += bloque(b"IDAT", zlib.compress(bytes(crudo), 6))
    png += bloque(b"IEND", b"")
    with open(ruta, "wb") as f:
        f.write(png)


def escribir_resumen(ruta, resumen):
    if not ruta:
        return
    tmp = ruta + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(resumen, f, indent=2)
    os.replace(tmp, ruta)


def recibir(conn, resumen, maximo):
    conn.settimeout(5.0)
    buffer = b""
    while True:
        if maximo and resumen["frames"] >= maximo:
            return
        try:
            trozo = conn.recv(65536)
        except socket.timeout:
            continue
        if not trozo:
            return
        buffer += trozo

        while len(buffer) >= CABECERA:
            magic, numero, w, h, flags, formato, _ = struct.unpack_from("<IIHHBBH", buffer, 0)
            if magic != MAGIC:
                # No es nuestro: se descarta lo que haya hasta el primer
                # byte que pueda ser el inicio de una cabecera.
                print(f"cabecera desconocida: 0x{magic:08x}", flush=True)
                buffer = b""
                break
            tam = w * h * 4
            if len(buffer) < CABECERA + tam:
                break
            pix = buffer[CABECERA:CABECERA + tam]
            buffer = buffer[CABECERA + tam:]

            del_cubo, cx0, cx1, cy0, cy1 = pixeles_del_cubo(pix, w)
            con_color = sum(
                1 for i in range(0, len(pix) - 3, 4) if pix[i] or pix[i + 1] or pix[i + 2]
            )
            ojo = flags & 1
            resumen["frames"] += 1
            resumen["ultimo_numero"] = numero
            resumen["ancho"] = w
            resumen["alto"] = h
            resumen["bytes_ultimo"] = tam
            resumen["pixeles_cubo_ultimo"] = del_cubo
            resumen["pixeles_color_ultimo"] = con_color
            if del_cubo > 0:
                # La posicion del cubo por ojo: si la camara esta desplazada,
                # el cubo sale mas a la izquierda en un ojo que en el otro.
                resumen["cubo_x_izq"] = cx1 if ojo == 0 else resumen["cubo_x_izq"]
                resumen["cubo_x_der"] = cx0 if ojo == 1 else resumen["cubo_x_der"]
                # El frame con MAS pixeles del cubo se guarda para verlo (a
                # times el cubo sale partido en el borde de la pantalla).
                if del_cubo > resumen["max_pixeles_cubo"]:
                    if resumen.get("_png"):
                        escribir_png(resumen["_png"], w, h, pix)
            resumen["max_pixeles_cubo"] = max(resumen["max_pixeles_cubo"], del_cubo)
            resumen["frames_con_cubo"] += 1 if del_cubo > 0 else 0
            resumen["frames_con_color"] += 1 if con_color > 0 else 0
            resumen["frames_izq"] += 1 if ojo == 0 else 0
            resumen["frames_der"] += 1 if ojo == 1 else 0
            resumen["ultimo_ts"] = time.time()
            escribir_resumen(resumen["_salida"], resumen)
            print(
                f"frame {numero}: {w}x{h} ojo={ojo} color={con_color} cubo={del_cubo}",
                flush=True,
            )


def main():
    ap = argparse.ArgumentParser(description="Receptor de frames XFV1")
    ap.add_argument("--puerto", type=int, default=27020)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--salida", default="", help="fichero JSON con el resumen")
    ap.add_argument("--frames", type=int, default=0, help="parar tras N frames (0 = sin limite)")
    ap.add_argument("--png", default="", help="guarda aqui el frame con mas pixeles del cubo")
    ap.add_argument("--timeout", type=float, default=0.0, help="segundos que espera conexiones")
    args = ap.parse_args()

    resumen = {
        "frames": 0,
        "ultimo_numero": -1,
        "ancho": 0,
        "alto": 0,
        "bytes_ultimo": 0,
        "pixeles_cubo_ultimo": 0,
        "pixeles_color_ultimo": 0,
        "max_pixeles_cubo": 0,
        "frames_con_cubo": 0,
        "frames_con_color": 0,
        "frames_izq": 0,
        "frames_der": 0,
        "cubo_x_izq": None,
        "cubo_x_der": None,
        "conexiones": 0,
        "error": "",
        "_salida": args.salida,
        "_png": args.png,
    }

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((args.host, args.puerto))
    srv.listen(1)
    srv.settimeout(args.timeout if args.timeout > 0 else 1.0)
    print(f"escuchando en {args.host}:{args.puerto}", flush=True)
    escribir_resumen(args.salida, resumen)

    fin = time.time() + args.timeout if args.timeout > 0 else None
    while True:
        if fin is not None and time.time() > fin:
            print("tiempo agotado", flush=True)
            break
        try:
            conn, addr = srv.accept()
        except socket.timeout:
            continue
        resumen["conexiones"] += 1
        print(f"conexion de {addr}", flush=True)
        escribir_resumen(args.salida, resumen)
        try:
            recibir(conn, resumen, args.frames)
        except Exception as exc:  # noqa: BLE001 - se registra y se sigue
            resumen["error"] = str(exc)
            print(f"error: {exc}", file=sys.stderr, flush=True)
        finally:
            conn.close()
        if args.frames and resumen["frames"] >= args.frames:
            break

    resumen.pop("_salida", None)
    resumen.pop("_png", None)
    escribir_resumen(args.salida, resumen)
    print(
        f"RESUMEN frames={resumen['frames']} con_cubo={resumen['frames_con_cubo']} "
        f"max_cubo={resumen['max_pixeles_cubo']}",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
