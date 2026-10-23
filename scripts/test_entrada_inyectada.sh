#!/usr/bin/env bash
set -uo pipefail

# test_entrada_inyectada.sh — Valida el PUENTE DE ENTRADA: que la mano del VR
# llegue al motor como un clic de raton, sin pasar por RCON.
#
# Que valida (por niveles):
#   1. El inyector encuentra las funciones de SDL del motor y abre el puerto
#      del puente de entrada.
#   2. Un clic enviado por el protocolo "XFCE" llega al motor: el motor
#      recibe el evento de SDL (se ve en el log del inyector).
#   3. (En el dispositivo) El motor ACTUA sobre el clic: el jugador se mueve,
#      aparece el fantasma de construccion, etc.
#
# El nivel 3 necesita el motor en juego y con ventana con foco (en la Pico,
# con el X de Termux). En el contenedor el motor va con SDL_VIDEODRIVER=
# offscreen, sin ventana con foco: ahi se validan los niveles 1 y 2.
#
# Codigos de salida:
#   0  todo correcto
#   1  fallo real
#   2  no se pudo VALIDAR (no hay contenedor, no compila...)
#
# Uso: bash scripts/test_entrada_inyectada.sh [--timeout 120]

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
IMAGE="docker.io/library/ubuntu:24.04"
CONTAINER="xfactorvr-client"
LOG_DIR="$PROJECT_ROOT/build"
LOG="$LOG_DIR/entrada_test.log"
PUERTO_ENTRADA=27021
TIMEOUT=120

for arg in "$@"; do
    case "$arg" in
        --timeout) TIMEOUT="${2:-120}"; shift ;;
    esac
done

say()  { printf '%s\n' "$*"; }
fail() { printf 'FALLO: %s\n' "$*" >&2; exit 1; }
nada() { printf 'NO VALIDABLE: %s\n' "$*" >&2; exit 2; }

# --------------------------------------------------------------------------
say "=== 1. Compilando el inyector para ARM64 ==="
if ! bash "$PROJECT_ROOT/scripts/test_injector.sh" --solo-compilar; then
    nada "no se pudo compilar el inyector (ver scripts/prepare_glibc.sh)"
fi
SO="$PROJECT_ROOT/injector-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so"
[ -f "$SO" ] || nada "no se genero el .so en $SO"
say "    .so: $SO ($(du -h "$SO" | cut -f1))"

# --------------------------------------------------------------------------
say ""
say "=== 2. Contenedor ==="
if ! "$PODMAN" machine inspect >/dev/null 2>&1; then
    nada "no hay maquina de podman"
fi
if ! "$PODMAN" machine inspect --format '{{.State}}' 2>/dev/null | grep -q '^running$'; then
    "$PODMAN" machine start >/dev/null 2>&1 || true
    sleep 5
fi
if ! "$PODMAN" container exists "$CONTAINER" 2>/dev/null; then
    nada "no existe el contenedor '$CONTAINER' (scripts/container_client_check.sh)"
fi
"$PODMAN" start "$CONTAINER" >/dev/null 2>&1 || true
"$PODMAN" cp "$SO" "$CONTAINER:/injector.so" >/dev/null 2>&1 || \
    "$PODMAN" exec "$CONTAINER" cat > /injector.so < "$SO"

# --------------------------------------------------------------------------
say ""
say "=== 3. Lanzando el cliente con el puente de entrada ==="
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true
sleep 1
"$PODMAN" exec "$CONTAINER" rm -f /factorio/.lock >/dev/null 2>&1 || true
rm -f "$LOG"

"$PODMAN" exec -d "$CONTAINER" bash -c "
export LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri
export MESA_LOADER_DRIVER_OVERRIDE=zink
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
export ZINK_DESCRIPTORS=auto
export SDL_VIDEODRIVER=offscreen
export EGL_PLATFORM=surfaceless
export LIBGL_ALWAYS_SOFTWARE=1
export LD_PRELOAD=/injector.so
export XFACTORVR_HOOK_EVERY=10
export XFACTORVR_ENTRADA_PUERTO=$PUERTO_ENTRADA
cd /factorio
mkdir -p logs saves config
cat > config/config-path.cfg <<CFG
config-path=/factorio/config
use-system-read-write-data-directories=false
CFG
exec ./bin/arm64/factorio > /factorio/entrada_run.log 2>&1
"

say "    esperando hasta ${TIMEOUT}s a que el puente de entrada levante..."
for i in $(seq 1 "$TIMEOUT"); do
    "$PODMAN" exec "$CONTAINER" cat /factorio/entrada_run.log > "$LOG" 2>/dev/null
    if grep -q "entrada: escuchando" "$LOG" 2>/dev/null; then
        break
    fi
    sleep 1
done
"$PODMAN" exec "$CONTAINER" cat /factorio/entrada_run.log > "$LOG" 2>/dev/null

if ! grep -q "entrada: escuchando" "$LOG" 2>/dev/null; then
    say "--- log ---"
    grep -E "xfv-injector|xfactorvr" "$LOG" 2>/dev/null | head -20 || true
    tail -10 "$LOG" 2>/dev/null || true
    say ""
    fail "el puente de entrada no levanto (no hay 'entrada: escuchando')"
fi
say "    OK: $(grep -m1 'entrada: escuchando' "$LOG")"

# --------------------------------------------------------------------------
say ""
say "=== 4. Mandando un clic por el protocolo XFCE ==="
# El injector escucha en 127.0.0.1 DENTRO del contenedor: el clic se manda
# desde dentro. Se usa python3 (el contenedor lo tiene) porque el /dev/tcp de
# bash no es fiable para esto.
#   magic "XFCE" | tipo 1 | tamano 6 | x u16 | y u16 | boton | accion
"$PODMAN" exec "$CONTAINER" python3 -c "
import socket, struct, sys
msg = struct.pack('<IBH', 0x45434658, 1, 6) + struct.pack('<HHBB', 320, 180, 1, 3)
try:
    s = socket.create_connection(('127.0.0.1', $PUERTO_ENTRADA), timeout=5)
    s.sendall(msg)
    s.close()
except OSError as e:
    print('no se pudo enviar:', e); sys.exit(1)
" || fail "no se pudo enviar el clic al puente de entrada"
sleep 1
"$PODMAN" exec "$CONTAINER" cat /factorio/entrada_run.log > "$LOG" 2>/dev/null

if grep -q "entrada: clic #" "$LOG" 2>/dev/null; then
    say "    OK: $(grep -m1 'entrada: clic #' "$LOG")"
else
    say "--- ultimas lineas ---"
    grep -E "xfv-injector" "$LOG" 2>/dev/null | tail -10 || true
    say ""
    fail "el clic no llego al inyector (no hay 'entrada: clic #')"
fi

"$PODMAN" exec "$CONTAINER" bash -c "pkill -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true

say ""
say "=== 5. Resultado ==="
say "OK: el puente de entrada mete el clic en la cola de eventos del motor."
say ""
say "Lo que NO se ha validado aqui (nivel 3): que el motor ACTUE sobre el"
say "clic (el jugador se mueva, aparezca el fantasma de construccion...). Eso"
say "necesita el motor en juego y con ventana con foco: se valida en la Pico"
say "con el X de Termux (scripts/spike_gpu_pico.sh + el visor)."
say ""
say "log: $LOG"
exit 0
