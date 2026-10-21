#!/usr/bin/env bash
set -uo pipefail

# test_modelo3d.sh — Banco de pruebas del hito "la entidad se dibuja en 3D".
#
# Lo que comprueba, de verdad, contra el cliente grafico real de Factorio
# 2.1.22 ARM64 dentro del contenedor podman:
#
#   1. El inyector compila para aarch64-unknown-linux-gnu.
#   2. El motor arranca una partida real.
#   3. Con la consola del juego se crea una maquina (assembling-machine-1)
#      justo al lado del jugador: asi hay una entidad que dibujar SIEMPRE, sin
#      depender de que la partida traiga maquinas cerca del punto de
#      aparicion (andar a ciegas no es fiable).
#   4. El hook de `AssemblingMachine::draw` se dispara y el motor dibuja el
#      cubo 3D en su framebuffer del mundo.
#   5. Se vuelca ese framebuffer a un PNG en build/ y el PNG es valido y
#      tiene pixeles del cubo (es decir: el cubo SE VE).
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

# --- 5a: esperar a que la partida este cargada ---
#
# El inyector avisa por el log cuando el motor ya esta en juego
# ("cima_tipo=4"): hasta entonces la consola del juego no existe y teclear
# seria mandar teclas al menu de inicio.
en_juego=0
for t in $(seq 1 60); do
    "$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null
    if grep -q "cima_tipo=4" "$LOG_LOCAL" 2>/dev/null; then
        en_juego=1
        break
    fi
    if grep -qE "SEGV|SIGILL|FATAL|Received 6" "$LOG_LOCAL" 2>/dev/null; then
        break
    fi
    sleep 2
done
[ "$en_juego" = "1" ] || fail "el motor no llego a estar en juego"
ok "partida cargada (el motor esta en juego)"

# --- 5b: crear una maquina al lado del jugador con la consola del juego ---
#
# Es determinista: andar buscando maquinas con el teclado no lo es (la
# partida no siempre trae fabricas cerca del punto de aparicion).
COMANDO_CONSOLA='/c local p = game.player.position; local e = game.surfaces[1].create_entity{name="assembling-machine-1", position={p.x+4, p.y}}; if e then game.print("MQUINA CREADA") else game.print("NO SE PUDO") end'
printf '%s' "$COMANDO_CONSOLA" > "$LOG_DIR/consola.txt"
"$PODMAN" cp "$LOG_DIR/consola.txt" "$CONTAINER:/tmp/consola.txt" >/dev/null 2>&1 || true

for intento in 1 2 3; do
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key grave" >/dev/null 2>&1 || true
    sleep 2
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool type --delay 25 --file /tmp/consola.txt" >/dev/null 2>&1 || true
    sleep 1
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key Return" >/dev/null 2>&1 || true
    sleep 8
    # Cerrar la consola del juego: si se deja abierta, sale en la captura.
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key Escape" >/dev/null 2>&1 || true
    "$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null
    if grep -q "caja: mundo=" "$LOG_LOCAL" 2>/dev/null; then
        ok "maquina creada con la consola del juego"
        break
    fi
    # Si el comando no ha entrado, la consola no estaba abierta: se prueba otra
    # vez (cerrando antes cualquier menu que se haya abierto).
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key Escape" >/dev/null 2>&1 || true
    sleep 2
done

# --- 5c: esperar a la captura con el cubo ---
encontrada=""
for t in $(seq 1 30); do
    "$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null
    if grep -q "caja: mundo=" "$LOG_LOCAL" 2>/dev/null \
        && "$PODMAN" exec "$CONTAINER" test -f "$CAPTURA_CONT" 2>/dev/null; then
        encontrada=1
        break
    fi
    if grep -qE "SEGV|SIGILL|FATAL|Received 6" "$LOG_LOCAL" 2>/dev/null; then
        break
    fi
    sleep 2
done
"$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null

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
#
# `podman cp` se corta con los ficheros grandes (la captura son 3,6 MB y
# llegaban 688 KB), y el banco daba un fallo que parecen del cubo cuando lo
# que estaba mal era la copia.  Se copia comprimida y se comprueba el
# tamaño antes de dar nada por bueno.
"$PODMAN" exec "$CONTAINER" bash -c "gzip -c '$CAPTURA_CONT' > /tmp/xfv_captura.gz" >/dev/null 2>&1 || true
"$PODMAN" exec "$CONTAINER" cat /tmp/xfv_captura.gz > "$LOG_DIR/cubo_3d.png.gz" 2>/dev/null || true
if [ -s "$LOG_DIR/cubo_3d.png.gz" ]; then
    gunzip -f "$LOG_DIR/cubo_3d.png.gz" 2>/dev/null || true
    mv -f "$LOG_DIR/cubo_3d.png" "$LOG_DIR/cubo_3d.png" 2>/dev/null || true
fi
if [ -f "$LOG_DIR/cubo_3d.png.gz" ]; then
    say "AVISO: la captura no se pudo traer entera (copia truncada)"
    rm -f "$LOG_DIR/cubo_3d.png"
fi

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

# El PNG tiene que ser valido (zlib con el Adler-32 bien puesto) y tener
# pixeles del cubo: eso es la prueba de que el cubo SE VE en la imagen.
#
# La validacion se hace DENTRO del contenedor: la captura son 3,6 MB y
# cualquier copia por el socket de podman se corta (llegan 688 KB o 3 MB),
# lo que hacia que el banco fallara por la copia y no por el cubo.
"$PODMAN" cp "$PROJECT_ROOT/scripts/validar_png_cubo.py" "$CONTAINER:/tmp/validar_png_cubo.py" >/dev/null 2>&1 || true
"$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /tmp/validar_png_cubo.py' < "$PROJECT_ROOT/scripts/validar_png_cubo.py" >/dev/null 2>&1 || true
if "$PODMAN" exec "$CONTAINER" python3 /tmp/validar_png_cubo.py "$CAPTURA_CONT" 2>&1 | tail -3 | sed 's/^/    /'; then
    ok "PNG valido con pixeles del cubo"
else
    fail "el PNG de la captura no es valido o no tiene el cubo"
fi

# La imagen se deja en el contenedor; al host se trae en trozos (por si se
# quiere mirar).
"$PODMAN" exec "$CONTAINER" bash -c "gzip -c '$CAPTURA_CONT' > /tmp/xfv_captura.gz" >/dev/null 2>&1 || true
TROZOS=0
while [ "$TROZOS" -lt 40 ]; do
    "$PODMAN" exec "$CONTAINER" bash -c \
        "dd if=/tmp/xfv_captura.gz bs=262144 skip=$TROZOS count=1 2>/dev/null" \
        >> "$LOG_DIR/cubo_3d.png.gz" 2>/dev/null || true
    TROZOS=$((TROZOS + 1))
    if ! "$PODMAN" exec "$CONTAINER" bash -c \
        "dd if=/tmp/xfv_captura.gz bs=262144 skip=$TROZOS count=1 2>/dev/null | head -c 1" \
        2>/dev/null | grep -q .; then
        break
    fi
done
if [ -s "$LOG_DIR/cubo_3d.png.gz" ]; then
    gunzip -f "$LOG_DIR/cubo_3d.png.gz" 2>/dev/null || true
fi

say ""
say "OK: el banco 3D paso. Captura en $LOG_DIR/cubo_3d.png"
exit 0
