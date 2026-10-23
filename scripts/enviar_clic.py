#!/usr/bin/env python3
"""enviar_clic.py — Manda un clic al motor de Factorio por el puente de entrada.

El puente de entrada (protocolo "XFCE") es el canal que sustituye a RCON para
las ordenes: el motor recibe el clic como si fuera el raton de un jugador, y
decide el (docs/ADR-013).

Uso:
    enviar_clic.py [--host 127.0.0.1] [--puerto 27021] X Y
                   [--boton izquierdo|medio|derecho] [--accion clic|bajo|arriba]

Ejemplo: mandar un clic en el centro de la imagen (640x360):
    enviar_clic.py 320 180

En el visor de verdad este cliente es el APK: el rayo de la mano golpea el
plano de la mesa, se convierte en un pixel de la imagen del motor y se manda
por el mismo canal.
"""

import argparse
import socket
import struct
import sys

MAGIC = 0x45434658  # "XFCE" en little-endian
TIPO_CLIC = 1
CABECERA = 7

ACCIONES = {"bajo": 1, "arriba": 2, "clic": 3}
BOTONES = {"izquierdo": 1, "medio": 2, "derecho": 3}


def construir(x: int, y: int, boton: int, accion: int) -> bytes:
    """Arma el mensaje del protocolo XFCE."""
    carga = struct.pack("<HHBB", x, y, boton, accion)
    return struct.pack("<IBH", MAGIC, TIPO_CLIC, len(carga)) + carga


def main() -> int:
    ap = argparse.ArgumentParser(description="Manda un clic al motor por el puente de entrada.")
    ap.add_argument("x", type=int, help="pixel X en la imagen transmitida")
    ap.add_argument("y", type=int, help="pixel Y en la imagen transmitida")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--puerto", type=int, default=27021)
    ap.add_argument("--boton", default="izquierdo", choices=list(BOTONES))
    ap.add_argument("--accion", default="clic", choices=list(ACCIONES))
    args = ap.parse_args()

    if not (0 <= args.x <= 0xFFFF and 0 <= args.y <= 0xFFFF):
        print("x e y tienen que caber en 16 bits", file=sys.stderr)
        return 1

    mensaje = construir(args.x, args.y, BOTONES[args.boton], ACCIONES[args.accion])
    try:
        with socket.create_connection((args.host, args.puerto), timeout=5) as s:
            s.sendall(mensaje)
    except OSError as e:
        print(f"no se pudo conectar a {args.host}:{args.puerto}: {e}", file=sys.stderr)
        return 2

    print(
        f"clic enviado: ({args.x},{args.y}) boton={args.boton} accion={args.accion} "
        f"-> {args.host}:{args.puerto} ({len(mensaje)} bytes)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
