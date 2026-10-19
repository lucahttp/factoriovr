#!/usr/bin/env python3
"""
rcon_probe.py — Cliente RCON minimo para inspeccionar el servidor de Factorio.

Sirve para comprobar que comandos realmente devuelven su salida por el
socket RCON. Es la duda critica del puente: si Factorio solo imprime el
resultado en la consola y no lo devuelve al cliente, el parseo JSON del
lado Rust no tiene nada que leer.

Uso:
    python3 scripts/rcon_probe.py <host> <port> <password> <comando> [...]
"""
import socket
import struct
import sys

TYPE_AUTH = 3
TYPE_AUTH_RESPONSE = 2
TYPE_COMMAND = 2
TYPE_RESPONSE = 0


def pack(req_id, ptype, body):
    payload = body.encode("utf-8") + b"\x00\x00"
    return struct.pack("<iii", len(payload) + 8, req_id, ptype) + payload


def read_packet(sock):
    header = b""
    while len(header) < 12:
        chunk = sock.recv(12 - len(header))
        if not chunk:
            return None, None, None
        header += chunk
    length, req_id, ptype = struct.unpack("<iii", header)
    body = b""
    remaining = length - 8
    while len(body) < remaining:
        chunk = sock.recv(remaining - len(body))
        if not chunk:
            break
        body += chunk
    if body.endswith(b"\x00\x00"):
        body = body[:-2]
    return req_id, ptype, body.decode("utf-8", errors="replace")


def main():
    guardar = None
    args = sys.argv[1:]
    if "--guardar" in args:
        i = args.index("--guardar")
        guardar = args[i + 1]
        del args[i:i + 2]

    if len(args) >= 2 and args[0] == "--file":
        if len(args) < 3:
            print(__doc__)
            return 2
        host, port, password = "127.0.0.1", int(args[2]), args[3]
        with open(args[1], encoding="utf-8") as fh:
            commands = [line.rstrip("\n") for line in fh if line.strip()]
    elif len(args) >= 4:
        host, port, password = args[0], int(args[1]), args[2]
        commands = args[3:]
    else:
        print(__doc__)
        return 2

    sock = socket.create_connection((host, port), timeout=10)
    sock.settimeout(5)

    sock.sendall(pack(1, TYPE_AUTH, password))
    req_id, ptype, body = read_packet(sock)
    print(f"auth -> id={req_id} tipo={ptype} cuerpo={body!r}")
    if ptype != TYPE_AUTH_RESPONSE:
        print("ERROR: el servidor no respondio con SERVERDATA_AUTH_RESPONSE")
        return 1

    for i, cmd in enumerate(commands, start=2):
        print(f"\n$ {cmd}")
        sock.sendall(pack(i, TYPE_COMMAND, cmd))
        chunks = []
        # Factorio puede partir la salida en varios paquetes.
        for _ in range(8):
            try:
                rid, pt, b = read_packet(sock)
            except socket.timeout:
                break
            if rid is None:
                break
            chunks.append(b)
            if b.strip():
                break
        out = "\n".join(c for c in chunks if c)
        print(f"  respuesta ({len(chunks)} paquete/s): {out!r}")
        if not out.strip():
            print("  >> RESPUESTA VACIA <<")
        elif guardar and '"entities"' in out:
            # Se guarda la ultima respuesta con entidades: la instantanea final.
            with open(guardar, "w", encoding="utf-8") as fh:
                fh.write(out.strip())
            print(f"  instantanea guardada en {guardar}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
