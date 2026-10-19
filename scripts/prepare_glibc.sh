#!/bin/bash
set -euo pipefail

# prepare_glibc.sh — Prepara el runtime de glibc para el visor Pico 4 Ultra
#
# El motor ARM64 de Factorio es un binario de Linux y Android usa bionic, no
# glibc. Sin un linker de glibc el motor no arranca en el visor: no hay
# "modo standalone" posible sin este paso.
#
# Se toma el paquete `glibc` del repositorio oficial de Termux (mismo que
# usa `pkg install glibc` dentro de Termux) y se extrae su arbol `usr/glibc`
# para poder empujarlo al dispositivo por adb.
#
# Salida: build/glibc.tar.gz con la estructura `lib/...` que en el dispositivo
# se extrae en /data/local/tmp/glibc (quedando el linker en
# /data/local/tmp/glibc/lib/ld-linux-aarch64.so.1).

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/build"
OUT_TAR="$BUILD_DIR/glibc.tar.gz"
STAGE_DIR="$BUILD_DIR/glibc_stage"

GLIBC_REPO="https://packages.termux.dev/apt/termux-glibc"
PACKAGES_INDEX="$GLIBC_REPO/dists/glibc/stable/binary-aarch64/Packages"

echo "=== Preparando el runtime de glibc (ARM64) ==="

if [ -f "$OUT_TAR" ]; then
    echo "Ya existe $OUT_TAR ($(du -h "$OUT_TAR" | cut -f1)). Se reutiliza."
    echo "Borra $OUT_TAR para regenerarlo."
    exit 0
fi

mkdir -p "$BUILD_DIR"

# 1. Resolver la version vigente del paquete glibc para aarch64.
echo "[1/4] Consultando el indice de paquetes de Termux..."
DEB_PATH=$(curl -sSf "$PACKAGES_INDEX" | python3 -c '
import re, sys
blocks = sys.stdin.read().split("\n\n")
for b in blocks:
    if re.search(r"^Package: glibc$", b, re.M):
        fn = re.search(r"^Filename: (.+)$", b, re.M)
        if fn:
            print(fn.group(1).strip())
            break
')

if [ -z "$DEB_PATH" ]; then
    echo "No se pudo resolver el paquete glibc en $PACKAGES_INDEX" >&2
    exit 1
fi
echo "    paquete: $DEB_PATH"

# 2. Descargar el .deb y verificar su checksum contra el indice.
echo "[2/4] Descargando el paquete..."
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/xfactorvr_glibc.XXXXXX")
trap 'rm -rf "$WORK_DIR"' EXIT

DEB_URL="$GLIBC_REPO/$DEB_PATH"
curl -sSfL "$DEB_URL" -o "$WORK_DIR/glibc.deb"

EXPECTED_SHA=$(curl -sSf "$PACKAGES_INDEX" | python3 -c '
import re, sys
blocks = sys.stdin.read().split("\n\n")
for b in blocks:
    if re.search(r"^Package: glibc$", b, re.M):
        h = re.search(r"^SHA256: (.+)$", b, re.M)
        if h:
            print(h.group(1).strip())
            break
')
ACTUAL_SHA=$(shasum -a 256 "$WORK_DIR/glibc.deb" | awk '{print $1}')
if [ -n "$EXPECTED_SHA" ] && [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
    echo "Checksum incorrecto: esperado $EXPECTED_SHA, obtenido $ACTUAL_SHA" >&2
    exit 1
fi
echo "    sha256 verificado: $ACTUAL_SHA"

# 3. Extraer el arbol usr/glibc del .deb (formato ar + tar.xz).
echo "[3/4] Extrayendo el arbol de librerias..."
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"
python3 - "$WORK_DIR/glibc.deb" "$STAGE_DIR" <<'PY'
import io, os, sys, tarfile

deb_path, stage = sys.argv[1], sys.argv[2]
data = open(deb_path, "rb").read()
if data[:8] != b"!<arch>\n":
    raise SystemExit("el fichero no es un paquete .deb valido")

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
    raise SystemExit("el paquete no contiene data.tar")

# El .deb guarda todo bajo data/data/com.termux/files/usr/glibc/...
PREFIX = "./data/data/com.termux/files/usr/glibc/"
tf = tarfile.open(fileobj=io.BytesIO(payload))
extracted = 0
for member in tf.getmembers():
    if not member.name.startswith(PREFIX):
        continue
    rel = member.name[len(PREFIX):]
    if not rel or rel.startswith("/"):
        continue
    target = os.path.join(stage, rel)
    if member.isdir():
        os.makedirs(target, exist_ok=True)
    elif member.isfile():
        os.makedirs(os.path.dirname(target), exist_ok=True)
        src = tf.extractfile(member)
        with open(target, "wb") as dst:
            dst.write(src.read())
        os.chmod(target, member.mode & 0o777 or 0o644)
        extracted += 1
    elif member.issym():
        os.makedirs(os.path.dirname(target), exist_ok=True)
        if os.path.lexists(target):
            os.remove(target)
        os.symlink(member.linkname, target)

print(f"    {extracted} ficheros extraidos")
PY

LOADER="$STAGE_DIR/lib/ld-linux-aarch64.so.1"
if [ ! -f "$LOADER" ]; then
    echo "No se encontro el linker glibc en $LOADER" >&2
    exit 1
fi
chmod 755 "$LOADER"

# 4. Empaquetar para empujarlo al visor con una sola transferencia.
echo "[4/4] Empaquetando $OUT_TAR..."
tar -czf "$OUT_TAR" -C "$STAGE_DIR" lib
rm -rf "$STAGE_DIR"

echo "=== glibc listo: $OUT_TAR ($(du -h "$OUT_TAR" | cut -f1)) ==="
echo "En el visor se extrae en /data/local/tmp/glibc y el linker queda en:"
echo "  /data/local/tmp/glibc/lib/ld-linux-aarch64.so.1"
