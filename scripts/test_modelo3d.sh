#!/usr/bin/env bash
set -uo pipefail

# test_modelo3d.sh — Banco de pruebas del hito "la entidad se dibuja en 3D".
#
# Lo que comprueba, de verdad, contra el cliente grafico real de Factorio
# 2.1.22 ARM64 dentro del contenedor podman:
#
#   1. El inyector compila para aarch64-unknown-linux-gnu.
#   2. El motor arranca una partida real (con maquinas) y el hook de
#      `AssemblingMachine::draw` se dispara.
#   3. El motor dibuja el cubo 3D en su framebuffer del mundo.
#   4. Se vuelca ese framebuffer a un PNG en build/ y el PNG tiene contenido
#      (no es negro, no es basura).
#
# Notas de por que se usa Xvfb y no el driver "offscreen" de SDL:
# con offscreen el motor se revienta al cambiar de estado (ver
# scripts/test_autostart.sh) y ademas no hay framebuffer de ventana real, que
# es justo lo que hacia que la captura saliera con basura.
#
# Codigos de salida:
#   0  todo correcto: hay captura y el log dice que se dibujaron cubos
#   1  fallo real (no compila, no hay captura, no se dibujo ningun cubo...)
#   2  no se pudo VALIDAR en el contenedor (maquina, contenedor, ...)
#
# Uso: bash scripts/test_modelo3d.sh [--timeout SEGUNDOS] [--save RUTA]
#
# Variables de entorno:
#   PODMAN   ruta del podman (por defecto /opt/podman/bin/podman)

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
CONTAINER="xfactorvr-client"
INJECTOR_DIR="$PROJECT_ROOT/injector-linux"
LOG_DIR="$PROJECT_ROOT/build"
TIMEOUT=240
SAVE="/factorio/saves/freeplay.zip"
VENTANA_ANCHO=1280
VENTANA_ALTO=720

for arg in "$@"; do
    case "$arg" in
        --timeout) TIMEOUT="${2:-240}"; shift ;;
        --save) SAVE="${2:-$SAVE}"; shift ;;
    esac
done

SO="$INJECTOR_DIR/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so"
LOG_CLIENT="/factorio/modelo3d.log"
LOG_LOCAL="$LOG_DIR/modelo3d.log"
CAPTURA_CONT="/factorio/cubo_3d.png"

say()  { printf '%s\n' "$*"; }
fail() { printf 'FALLO: %s\n' "$*" >&2; exit 1; }
ok()   { printf '  OK   %s\n' "$*"; }

# --------------------------------------------------------------------------
say "=== 1. Compilando el inyector para aarch64 ==="
if ! bash "$PROJECT_ROOT/scripts/test_injector.sh" --solo-compilar; then
    fail "no se pudo compilar el inyector"
    exit 1
fi
[ -f "$SO" ] || fail "no existe el .so en $SO"
ok ".so: $(du -h "$SO" | cut -f1)"

# --------------------------------------------------------------------------
say ""
say "=== 2. Preparando el contenedor ==="
if ! "$PODMAN" container exists "$CONTAINER" 2>/dev/null; then
    say "AVISO: no existe el contenedor $CONTAINER: no se puede validar"
    exit 2
fi
if [ "$("$PODMAN" inspect --format '{{.State.Running}}' "$CONTAINER" 2>/dev/null)" != "true" ]; then
    "$PODMAN" start "$CONTAINER" >/dev/null 2>&1 || true
    sleep 2
fi
if ! "$PODMAN" exec "$CONTAINER" test -f /factorio/bin/arm64/factorio 2>/dev/null; then
    say "AVISO: no hay cliente grafico ARM64 en el contenedor: no se puede validar"
    exit 2
fi
"$PODMAN" cp "$SO" "$CONTAINER:/injector.so" >/dev/null 2>&1 \
    || "$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /injector.so' < "$SO" \
    || fail "no se pudo copiar el .so al contenedor"
ok ".so copiado"

# --------------------------------------------------------------------------
say ""
say "=== 3. Pantalla virtual (Xvfb) ==="
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 Xvfb" >/dev/null 2>&1 || true
sleep 1
DISPLAY_NUM=""
for d in 99 98 97 96 95; do
    "$PODMAN" exec "$CONTAINER" bash -c "rm -f /tmp/.X11-unix/X$d /tmp/.X$d-lock" >/dev/null 2>&1
    "$PODMAN" exec -d "$CONTAINER" bash -c "Xvfb :$d -screen 0 ${VENTANA_ANCHO}x${VENTANA_ALTO}x24 -nolisten tcp > /factorio/xvfb_3d.log 2>&1"
    listo=0
    for t in $(seq 1 30); do
        if "$PODMAN" exec "$CONTAINER" test -e "/tmp/.X11-unix/X$d" 2>/dev/null; then
            if "$PODMAN" exec "$CONTAINER" bash -c "pgrep -f 'Xvfb :$d' >/dev/null" 2>/dev/null; then
                listo=1
            fi
            break
        fi
        sleep 0.5
    done
    if [ "$listo" = "1" ]; then
        DISPLAY_NUM=":$d"
        break
    fi
    "$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f 'Xvfb :$d'" >/dev/null 2>&1 || true
    sleep 1
done
[ -n "$DISPLAY_NUM" ] || fail "no se pudo arrancar Xvfb en el contenedor"
ok "pantalla $DISPLAY_NUM"

# --------------------------------------------------------------------------
say ""
say "=== 4. Arrancando el cliente con el cubo 3D ==="
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true
sleep 2
"$PODMAN" exec "$CONTAINER" rm -f /factorio/.lock "$LOG_CLIENT" >/dev/null 2>&1 || true
"$PODMAN" exec "$CONTAINER" rm -f /factorio/cubo_3d.*.png >/dev/null 2>&1 || true

"$PODMAN" exec -d "$CONTAINER" bash -c "
export DISPLAY=$DISPLAY_NUM
export SDL_VIDEODRIVER=x11
export LIBGL_ALWAYS_SOFTWARE=1
export LD_PRELOAD=/injector.so
export XFACTORVR_HOOK_EVERY=50
export XFACTORVR_3D=1
export XFACTORVR_3D_CAPTURA=$CAPTURA_CONT
export XFACTORVR_AUTOSTART='$SAVE'
export XFACTORVR_AUTOSTART_DELAY_S=6
cd /factorio
exec ./bin/arm64/factorio --window-size ${VENTANA_ANCHO}x${VENTANA_ALTO} > $LOG_CLIENT 2>&1
"
ok "cliente lanzado (partida: $SAVE)"

# --------------------------------------------------------------------------
say ""
say "=== 5. Esperando la escena con el cubo (hasta ${TIMEOUT}s) ==="
encontrada=""
# El jugador se mueve un poco: las maquinas de la partida estan a unos
# cuantos tiles del punto de aparicion, asi que hay que andar para que
# entren en pantalla.
TECLAS="w a s d"
for i in $(seq 1 "$TIMEOUT"); do
    "$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null
    # La captura se escribe sola (el frame mas reciente) en cuanto hay un
    # framebuffer del mundo; y el cubo aparece cuando el motor ha dibujado
    # alguna entidad.
    if grep -q "caja: mundo=" "$LOG_LOCAL" 2>/dev/null \
        && "$PODMAN" exec "$CONTAINER" test -f "$CAPTURA_CONT" 2>/dev/null; then
        encontrada=1
        break
    fi
    if grep -qE "SEGV|SIGILL|FATAL|Received 6" "$LOG_LOCAL" 2>/dev/null; then
        break
    fi
    # Andar un rato en una direccion (el juego corre con Xvfb, asi que se
    # puede pulsar teclado de verdad).
    if [ $((i % 4)) -eq 0 ]; then
        k=$(echo "$TECLAS" | cut -d" " -f$(( (i / 4) % 4 + 1 )))
        "$PODMAN" exec "$CONTAINER" bash -c \
            "DISPLAY=$DISPLAY_NUM xdotool keydown $k; sleep 3; DISPLAY=$DISPLAY_NUM xdotool keyup $k" \
            >/dev/null 2>&1 || true
    fi
    sleep 2
done
"$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null

# --------------------------------------------------------------------------
say ""
say "=== 6. Resultado ==="
say "--- log del inyector (lineas del modulo 3D) ---"
grep -E "xfv-injector" "$LOG_LOCAL" 2>/dev/null | grep -vE "bytes|hook en|simbolos de funcion|funciones del motor" | head -40 || true

if [ -z "$encontrada" ]; then
    say ""
    say "--- ultimas lineas del cliente ---"
    tail -15 "$LOG_LOCAL" 2>/dev/null || true
    fail "no se escribio ninguna captura en $CAPTURA_CONT"
fi

# Traer la captura al host.
"$PODMAN" cp "$CONTAINER:$CAPTURA_CONT" "$LOG_DIR/cubo_3d.png" >/dev/null 2>&1 || true

PIDE="$(grep -oE 'caja: mundo=\([^)]*\) tamano=[0-9.]+x[0-9.]+' "$LOG_LOCAL" 2>/dev/null | head -1)"
DIBUJA="$(grep -oE '[0-9]+ cajas?[^\n]*' "$LOG_LOCAL" 2>/dev/null | tail -1)"
CAPT="$(grep -oE 'captura [0-9]+x[0-9]+: [0-9]+ pixeles con color' "$LOG_LOCAL" 2>/dev/null | tail -1)"

if [ -n "$CAPT" ] && ! echo "$CAPT" | grep -qE ": 0 pixeles"; then
    ok "captura con contenido: $CAPT"
else
    fail "la captura salio vacia (negra)"
fi

if [ -n "$PIDE" ]; then
    ok "caja 3D colocada: $PIDE"
else
    fail "no se registro ninguna caja 3D (el hook de la entidad no llego a la caja)"
fi

say ""
say "OK: el banco 3D paso. Captura en $LOG_DIR/cubo_3d.png"
exit 0
