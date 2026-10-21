#!/usr/bin/env bash
set -uo pipefail

# test_injector.sh — Valida el inyector LD_PRELOAD contra el cliente grafico
# real de Factorio 2.1.22 ARM64 dentro del contenedor podman.
#
# Lo que hace:
#   1. Prepara el sysroot de glibc ARM64 y compila injector-linux para
#      aarch64-unknown-linux-gnu.
#   2. Arranca la maquina de podman si esta apagada.
#   3. Copia el .so al contenedor y lanza el cliente grafico con
#      LD_PRELOAD y el entorno de Mesa/Zink (lavapipe, SDL offscreen).
#   4. Busca "hook vivo: N frames" con N > 0 en la salida.
#
# Codigos de salida:
#   0  todo correcto: el hook esta vivo y el cliente arranco
#   1  fallo real (no compila, el cliente no arranca, el hook no aparece...)
#   2  no se pudo VALIDAR en el contenedor (maquina o contenedor no
#      disponibles). No se da por validado algo que no lo esta.
#
# Uso: bash scripts/test_injector.sh [--timeout SEGUNDOS] [--keep]
#       bash scripts/test_injector.sh --solo-compilar
#
# Con --solo-compilar se hace solo el paso 1 (preparar el sysroot y compilar
# el inyector para aarch64) y se sale con 0: lo usa scripts/test_autostart.sh
# para no repetir elmontaje del toolchain.

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
IMAGE="docker.io/library/ubuntu:24.04"
CONTAINER="xfactorvr-client"
INJECTOR_DIR="$PROJECT_ROOT/injector-linux"
SYSROOT="$PROJECT_ROOT/build/glibc_sysroot"
LOG_DIR="$PROJECT_ROOT/build"
TIMEOUT=90
KEEP=0
SOLO_COMPILAR=0

for arg in "$@"; do
    case "$arg" in
        --timeout) TIMEOUT="${2:-90}"; shift ;;
        --keep) KEEP=1 ;;
        --solo-compilar) SOLO_COMPILAR=1 ;;
    esac
done

CLIENT_SRC="$PROJECT_ROOT/factorio_client_arm64/factorio"
SO="$INJECTOR_DIR/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so"
LOG="$LOG_DIR/injector_test.log"

say()  { printf '%s\n' "$*"; }
fail() { printf 'FALLO: %s\n' "$*" >&2; exit 1; }

# --------------------------------------------------------------------------
say "=== 1. Preparando el sysroot de glibc ARM64 y compilando el inyector ==="

if [ ! -f "$PROJECT_ROOT/build/glibc.tar.gz" ]; then
    say "    no hay build/glibc.tar.gz: ejecuta scripts/prepare_glibc.sh primero"
    exit 2
fi

mkdir -p "$SYSROOT" "$SYSROOT/link"
if [ ! -f "$SYSROOT/lib/libc.so.6" ]; then
    tar -xzf "$PROJECT_ROOT/build/glibc.tar.gz" -C "$SYSROOT"
    say "    sysroot extraido en build/glibc_sysroot"
fi

# El libc.so y el libm.so de Termux apuntan a rutas /data/data/... que no
# existen en el host: se sustituyen por scripts equivalentes con rutas del
# sysroot (dentro del sysroot, "/lib/..." se resuelve relativo a el).
cat > "$SYSROOT/link/libc.so" <<'EOF'
OUTPUT_FORMAT(elf64-littleaarch64)
GROUP ( /lib/libc.so.6 /lib/libc_nonshared.a AS_NEEDED ( /lib/ld-linux-aarch64.so.1 ) )
EOF
cat > "$SYSROOT/link/libm.so" <<'EOF'
OUTPUT_FORMAT(elf64-littleaarch64)
GROUP ( /lib/libm.so.6 AS_NEEDED ( /lib/libmvec.so.1 ) )
EOF

# El sysroot no trae libgcc_s. Se crea un archivo de biblioteca VACIO para
# que el enlazador de por hecho el "-lgcc_s" que pasa rustc: los simbolos
# _Unwind_* se quedan sin resolver en el .so y los stubs locales de
# src/gnu_unwind.rs los cubren (ver el ADR-003). Con el libunwind del NDK
# el .so no cargaba: referenciaba el simbolo interno de glibc __sF, que en
# aarch64 no esta exportado.
NDK="${ANDROID_NDK_HOME:-}"
if [ -z "$NDK" ] || [ ! -x "$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang" ]; then
    for cand in "${ANDROID_SDK_ROOT:-/Volumes/X10Pro/Android/sdk}"/ndk/* /Volumes/X10Pro/Android/sdk/ndk/*; do
        if [ -x "$cand/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang" ]; then NDK="$cand"; break; fi
    done
fi
if [ -z "$NDK" ]; then
    say "AVISO: no se encontro el NDK de Android: no se puede compilar el .so"
    exit 2
fi
say "    NDK: $NDK"

AR=$(ls "$NDK"/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-ar 2>/dev/null | head -1)
if [ -n "$AR" ]; then
    rm -f "$SYSROOT/link/libgcc_s.a"
    "$AR" crs "$SYSROOT/link/libgcc_s.a"
fi

if ! rustup target list --installed 2>/dev/null | grep -q "^aarch64-unknown-linux-gnu$"; then
    say "    instalando el target aarch64-unknown-linux-gnu..."
    rustup target add aarch64-unknown-linux-gnu >/dev/null 2>&1 || true
fi

# El .cargo/config.toml del crate lleva rutas absolutas: se ajustan al
# NDK y al sysroot de esta maquina antes de compilar.
CFG="$INJECTOR_DIR/.cargo/config.toml"
[ -f "$CFG" ] || fail "falta $CFG"
cp "$CFG" "$CFG.bak"
python3 - "$CFG" "$NDK" "$SYSROOT" <<'PY'
import re, sys
cfg, ndk, sysroot = sys.argv[1], sys.argv[2], sys.argv[3]
t = open(cfg, encoding="utf-8").read()
t = re.sub(r'^linker = ".*"$', f'linker = "{ndk}/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang"', t, flags=re.M)
t = re.sub(r'--sysroot=[^"]*', f'--sysroot={sysroot}', t)
t = re.sub(r'-Clink-arg=/[^"]*/libc\.so', f'-Clink-arg={sysroot}/link/libc.so', t)
t = re.sub(r'-Clink-arg=-L/[^"]*/link', f'-Clink-arg=-L{sysroot}/link', t)
open(cfg, "w", encoding="utf-8").write(t)
PY

# Se toca el fuente para que cargo recompile siempre: si el .so quedo mas
# nuevo que el fuente (por un mtime que no cuadra), cargo lo daria por
# bueno y el banco validaria un binario viejo.
touch "$INJECTOR_DIR/src/modelo3d.rs" "$INJECTOR_DIR/src/detour.rs" "$INJECTOR_DIR/src/stream.rs" 2>/dev/null || true
( cd "$INJECTOR_DIR" && cargo build --release --target aarch64-unknown-linux-gnu ) || {
    cp "$CFG.bak" "$CFG"; fail "no se pudo compilar el inyector"; }
cp "$CFG.bak" "$CFG"; rm -f "$CFG.bak"

[ -f "$SO" ] || fail "no se genero el .so en $SO"
say "    .so: $SO ($(du -h "$SO" | cut -f1))"
file_out=$("$PODMAN" run --rm --platform linux/arm64 "$IMAGE" true 2>/dev/null; echo ok)
say "    target: aarch64-unknown-linux-gnu"

if [ "$SOLO_COMPILAR" = "1" ]; then
    say "    (--solo-compilar: se para aqui)"
    exit 0
fi

# --------------------------------------------------------------------------
say ""
say "=== 2. Preparando la maquina de podman ==="
if ! "$PODMAN" machine inspect >/dev/null 2>&1; then
    say "    no hay maquina de podman: no se puede validar en el contenedor"
    exit 2
fi
if ! "$PODMAN" machine inspect --format '{{.State}}' 2>/dev/null | grep -q '^running$'; then
    say "    arrancando la maquina de podman..."
    "$PODMAN" machine start >/dev/null 2>&1 || true
    sleep 5
fi
if ! "$PODMAN" machine inspect --format '{{.State}}' 2>/dev/null | grep -q '^running$'; then
    say "AVISO: la maquina de podman no arranco: no se pudo validar en el contenedor"
    exit 2
fi
say "    maquina: en marcha"

# --------------------------------------------------------------------------
say ""
say "=== 3. Comprobando el contenedor ==="
if ! "$PODMAN" container exists "$CONTAINER" 2>/dev/null; then
    say "AVISO: el contenedor '$CONTAINER' no existe."
    say "        No se pudo validar en el contenedor (codigo 2)."
    say "        Para crearlo: bash scripts/container_client_check.sh"
    exit 2
fi
if [ "$("$PODMAN" inspect --format '{{.State.Running}}' "$CONTAINER" 2>/dev/null)" != "true" ]; then
    say "    arrancando el contenedor..."
    "$PODMAN" start "$CONTAINER" >/dev/null 2>&1 || true
    sleep 2
fi

# El cliente grafico ARM64 tiene que estar dentro.
if ! "$PODMAN" exec "$CONTAINER" test -f /factorio/bin/arm64/factorio 2>/dev/null; then
    say "    copiando el cliente grafico ARM64 al contenedor..."
    "$PODMAN" exec "$CONTAINER" mkdir -p /factorio 2>/dev/null
    if [ -d "$CLIENT_SRC" ]; then
        tar -C "$PROJECT_ROOT/factorio_client_arm64" -cf - factorio | \
            "$PODMAN" exec -i "$CONTAINER" tar -C / -xf - 2>/dev/null
    fi
fi
if ! "$PODMAN" exec "$CONTAINER" test -f /factorio/bin/arm64/factorio 2>/dev/null; then
    say "AVISO: no hay cliente grafico ARM64 en el contenedor: no se pudo validar"
    exit 2
fi

say "    copiando el .so..."
"$PODMAN" cp "$SO" "$CONTAINER:/injector.so" >/dev/null 2>&1 || {
    "$PODMAN" exec "$CONTAINER" cat > /injector.so < "$SO"; }

# --------------------------------------------------------------------------
say ""
say "=== 4. Lanzando el cliente grafico con LD_PRELOAD ==="
rm -f "$LOG"

# El motor usa un fichero de lock: si quedo otra instancia viva, el cliente
# se niega a arrancar ("Couldn't acquire exclusive lock").
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true
sleep 2
"$PODMAN" exec "$CONTAINER" rm -f /factorio/.lock >/dev/null 2>&1 || true

# El cliente se lanza desacoplado y escribe su salida dentro del contenedor;
# desde aqui se va leyendo el log por encima.
"$PODMAN" exec -d "$CONTAINER" bash -c '
export LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri
export MESA_LOADER_DRIVER_OVERRIDE=zink
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
export ZINK_DESCRIPTORS=auto
export SDL_VIDEODRIVER=offscreen
export EGL_PLATFORM=surfaceless
export LIBGL_ALWAYS_SOFTWARE=1
export LD_PRELOAD=/injector.so
export XFACTORVR_HOOK_EVERY=10
cd /factorio
mkdir -p logs saves config
cat > config/config-path.cfg <<CFG
config-path=/factorio/config
use-system-read-write-data-directories=false
CFG
exec ./bin/arm64/factorio > /factorio/injector_run.log 2>&1
'

say "    esperando hasta ${TIMEOUT}s a que el hook reporte frames..."
for i in $(seq 1 "$TIMEOUT"); do
    "$PODMAN" exec "$CONTAINER" cat /factorio/injector_run.log > "$LOG" 2>/dev/null
    if grep -qE "hook vivo: [0-9]+ frames" "$LOG" 2>/dev/null; then
        break
    fi
    sleep 1
done
"$PODMAN" exec "$CONTAINER" cat /factorio/injector_run.log > "$LOG" 2>/dev/null

# Parar el cliente (si hay pkill; si no, se queda corriendo).
"$PODMAN" exec "$CONTAINER" bash -c "pkill -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true

# --------------------------------------------------------------------------
say ""
say "=== 5. Resultado ==="
say "--- log del inyector ---"
grep -E "xfv-injector|xfactorvr" "$LOG" 2>/dev/null | head -20 || true
say "--- ultimas lineas del cliente ---"
tail -12 "$LOG" 2>/dev/null || true
say "--- log completo: $LOG ---"

if ! grep -q "xfv-injector" "$LOG" 2>/dev/null; then
    say ""
    say "FALLO: el inyector no arranco (no hay salida de [xfv-injector])"
    exit 1
fi

if ! grep -q "detour instalado" "$LOG" 2>/dev/null; then
    say ""
    say "FALLO: el inyector arranco pero no instalo el detour"
    exit 1
fi

if grep -qE "hook vivo: [1-9][0-9]* frames" "$LOG" 2>/dev/null; then
    FRAMES=$(grep -oE "hook vivo: [0-9]+ frames" "$LOG" | tail -1)
    say ""
    say "OK: hook vivo -> $FRAMES"
    exit 0
fi

say ""
say "FALLO: el hook no llego a ejecutarse (no hay 'hook vivo: N frames' con N > 0)"
exit 1
