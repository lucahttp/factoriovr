#!/usr/bin/env python3
"""rcon_comandos.py — Manda comandos de consola a Factorio por RCON y imprime
las respuestas, una por linea.

Es el hermano de scripts/extraer_snapshot.py: aquel saca el snapshot a un
fichero, este manda comandos sueltos (crear una caja, mover un item...).

Factorio rechaza el PRIMER comando de consola de cada sesion con el aviso
"Using Lua console commands will disable achievements. Please repeat the
command to proceed", y hasta que se repite y se acepta NO ACEPTA NINGUN OTRO.
Por eso aqui se manda un calentamiento y se repite hasta que el servidor lo
acepte.

Uso: rcon_comandos.py host:puerto password "comando 1" "comando 2" ...
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
    if cuerpo.endswith(b"\x00\x00"):
        cuerpo = cuerpo[:-2]
    return cuerpo.decode("utf-8", "replace")


def main():
    direccion = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1:27015"
    password = sys.argv[2] if len(sys.argv) > 2 else "xfactorvr"
    comandos = sys.argv[3:]

    host, _, puerto = direccion.rpartition(":")
    sock = socket.create_connection((host, int(puerto)), timeout=10)
    sock.settimeout(20)

    sock.sendall(empaquetar(1, 3, password))
    leer(sock)

    for _ in range(4):
        sock.sendall(empaquetar(2, 2, "/silent-command rcon.print('listo')"))
        cuerpo = leer(sock)
        if cuerpo and "Please repeat the command" not in cuerpo:
            break

    for indice, comando in enumerate(comandos, start=3):
        sock.sendall(empaquetar(indice, 2, comando))
        cuerpo = leer(sock)
        if cuerpo is None:
            print(f"{comando}\tNADA")
        else:
            print(f"{comando}\t{cuerpo.replace(chr(10), ' ')}")


if __name__ == "__main__":
    main()
