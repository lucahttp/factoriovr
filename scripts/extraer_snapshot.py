#!/usr/bin/env python3
"""extraer_snapshot.py — Saca el snapshot del mod por RCON y lo guarda tal cual.

Sirve para comprobar el parseo del cliente Rust (src/game_bridge.rs) con el
JSON REAL que produce el motor, y no con una muestra escrita a mano.

Uso: extraer_snapshot.py [host:puerto] [password] [salida]
"""

import socket
import struct
import sys


def empaquetar(id_, tipo, cuerpo):
    carga = cuerpo.encode("utf-8") + b"\x00\x00"
    return struct.pack("<iii", len(carga) + 8, id_, tipo) + carga


def leer(sock):
    cabecera = b""
    while len(cabecera) < 12:
        trozo = sock.recv(12 - len(cabecera))
        if not trozo:
            return None
        cabecera += trozo
    longitud, _id, _tipo = struct.unpack("<iii", cabecera)
    cuerpo = b""
    faltan = max(0, longitud - 8)
    while len(cuerpo) < faltan:
        trozo = sock.recv(faltan - len(cuerpo))
        if not trozo:
            break
        cuerpo += trozo
    return cuerpo


def main():
    direccion = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1:27015"
    password = sys.argv[2] if len(sys.argv) > 2 else "xfactorvr"
    salida = sys.argv[3] if len(sys.argv) > 3 else "/tmp/snapshot.json"

    host, _, puerto = direccion.rpartition(":")
    sock = socket.create_connection((host, int(puerto)), timeout=10)
    sock.settimeout(15)

    sock.sendall(empaquetar(1, 3, password))
    leer(sock)

    # El primer comando de consola de cada sesion lo rechaza Factorio: hay que
    # repetirlo hasta que lo acepte.
    for _ in range(4):
        sock.sendall(empaquetar(2, 2, "/silent-command rcon.print('listo')"))
        cuerpo = leer(sock)
        if cuerpo and b"Please repeat the command" not in cuerpo:
            break

    sock.sendall(
        empaquetar(
            3,
            2,
            "/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0, 0)",
        )
    )
    cuerpo = leer(sock) or b""
    texto = cuerpo.decode("utf-8", "replace")
    with open(salida, "w", encoding="utf-8") as f:
        f.write(texto)
    print(f"escritos {len(texto)} bytes en {salida}")
    print(texto[:160])


if __name__ == "__main__":
    main()
