#!/usr/bin/env bash
set -uo pipefail

# test_stream_render.sh — Banco del hito "el render REAL del juego llega al visor".
#
# Lo que comprueba, contra el cliente grafico real de Factorio 2.1.22 ARM64
# dentro del contenedor podman:
#
#   1. El inyector compila para aarch64-unknown-linux-gnu.
#   2. El motor arranca una partida real (pantalla virtual Xvfb).
#   3. Con la consola del juego se crea una maquina al lado del jugador, para
#      que haya una entidad que dibujar en 3D.
#   4. El inyector lee el framebuffer del mundo y lo manda por TCP con el
#      protocolo "XFV1".
#   5. Un receptor (scripts/receptor_stream.py) recibe esos frames: hay que
#      ver frames con contenido Y con pixeles del cubo 3D. Eso prueba que la
#      imagen que compone el motor (con el cubo dentro) viaja al visor.
#
# Codigos de salida:
#   0  todo correcto: llegaron frames con el cubo 3D dentro
#   1  fallo real (no compila, no llegan frames, la imagen va vacia...)
#   2  no se pudo VALIDAR en el contenedor (maquina, contenedor, ...)
#
# Uso: bash scripts/test_stream_render.sh [--timeout SEGUNDOS]
#
# Variables de entorno:
#   PODMAN   ruta del podman (por defecto /opt/podman/bin/podman)

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
CONTAINER="xfactorvr-client"
INJECTOR_DIR="$PROJECT_ROOT/injector-linux"
LOG_DIR="$PROJECT_ROOT/build"
TIMEOUT=300
SAVE="/factorio/saves/xfv-virtual.zip"
VENTANA_ANCHO=1280
VENTANA_ALTO=720
PUERTO=27020
# El RCON del CLIENTE grafico: por el se manda la orden de crear la maquina
# (no depende del foco de la ventana, que en un Xvfb sin gestor de ventanas
# es un problema).
RCON_CLIENTE=27022
RCON_PASSWORD=xfactorvr
STREAM_ANCHO=640
STREAM_ALTO=360
STREAM_FPS=10
# Desplazamiento de la camara del motor, en pixeles (0 = desactivado).
CAMARA_DESPLAZAMIENTO=0

for arg in "$@"; do
    case "$arg" in
        --timeout) TIMEOUT="${2:-300}"; shift ;;
        --camara) CAMARA_DESPLAZAMIENTO="${2:-0}"; shift ;;
    esac
done

SO="$INJECTOR_DIR/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so"
LOG_CLIENT="/factorio/stream.log"
LOG_LOCAL="$LOG_DIR/stream.log"
RESUMEN_CONT="/factorio/stream.json"
RESUMEN_LOCAL="$LOG_DIR/stream.json"

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
"$PODMAN" cp "$PROJECT_ROOT/scripts/receptor_stream.py" "$CONTAINER:/tmp/receptor_stream.py" >/dev/null 2>&1 \
    || "$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /tmp/receptor_stream.py' < "$PROJECT_ROOT/scripts/receptor_stream.py" \
    || fail "no se pudo copiar el receptor al contenedor"
"$PODMAN" cp "$PROJECT_ROOT/scripts/rcon_probe.py" "$CONTAINER:/tmp/rcon_probe.py" >/dev/null 2>&1 \
    || "$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /tmp/rcon_probe.py' < "$PROJECT_ROOT/scripts/rcon_probe.py" \
    || fail "no se pudo copiar el cliente RCON al contenedor"
ok ".so y receptor copiados"

# --------------------------------------------------------------------------
say ""
say "=== 3. Pantalla virtual (Xvfb) ==="
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 Xvfb" >/dev/null 2>&1 || true
sleep 1
DISPLAY_NUM=""
for d in 99 98 97 96 95; do
    "$PODMAN" exec "$CONTAINER" bash -c "rm -f /tmp/.X11-unix/X$d /tmp/.X$d-lock" >/dev/null 2>&1 || true
    "$PODMAN" exec -d "$CONTAINER" bash -c "Xvfb :$d -screen 0 ${VENTANA_ANCHO}x${VENTANA_ALTO}x24 -nolisten tcp > /factorio/xvfb_stream.log 2>&1"
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
say "=== 4. Arrancando el receptor de frames (puerto $PUERTO) ==="
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f receptor_stream.py" >/dev/null 2>&1 || true
"$PODMAN" exec "$CONTAINER" rm -f "$RESUMEN_CONT" >/dev/null 2>&1 || true
"$PODMAN" exec -d "$CONTAINER" bash -c "
python3 /tmp/receptor_stream.py --host 127.0.0.1 --puerto $PUERTO --salida $RESUMEN_CONT --png /factorio/frame_cubo.png --timeout $((TIMEOUT + 60)) > /factorio/receptor.log 2>&1
"
sleep 2
if ! "$PODMAN" exec "$CONTAINER" test -f "$RESUMEN_CONT" 2>/dev/null; then
    fail "el receptor no arranco (mira $LOG_DIR/../build/receptor.log dentro del contenedor)"
fi
ok "receptor escuchando en 127.0.0.1:$PUERTO"

# --------------------------------------------------------------------------
say ""
say "=== 5. Arrancando el cliente con el inyector ==="

# El arranque automatico de la partida (el hook de autostart del inyector) es
# la parte mas fragil: si el juego peta al cargar, se reintenta una vez.
arrancar_cliente() {
    "$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true
    sleep 3
    "$PODMAN" exec "$CONTAINER" rm -f /factorio/.lock "$LOG_CLIENT" >/dev/null 2>&1 || true

    "$PODMAN" exec -d "$CONTAINER" bash -c "
export DISPLAY=$DISPLAY_NUM
export SDL_VIDEODRIVER=x11
export LIBGL_ALWAYS_SOFTWARE=1
export LD_PRELOAD=/injector.so
export XFACTORVR_HOOK_EVERY=50
export XFACTORVR_3D=1
export XFACTORVR_STREAM=127.0.0.1:$PUERTO
export XFACTORVR_STREAM_ANCHO=$STREAM_ANCHO
export XFACTORVR_STREAM_ALTO=$STREAM_ALTO
export XFACTORVR_STREAM_FPS=$STREAM_FPS
# Camara desplazada alterna por ojo: es el ESTEREO REAL (el motor dibuja dos
# vistas y el visor muestra cada una en su ojo).
export XFACTORVR_3D_CAMARA=$CAMARA_DESPLAZAMIENTO
export XFACTORVR_STREAM_DEBUG_PNG=/factorio/stream_debug.png
export XFACTORVR_AUTOSTART='$SAVE'
export XFACTORVR_AUTOSTART_DELAY_S=6
cd /factorio
exec ./bin/arm64/factorio --window-size ${VENTANA_ANCHO}x${VENTANA_ALTO} > $LOG_CLIENT 2>&1
"
}

en_juego=0
for intento in 1 2; do
    arrancar_cliente
    ok "cliente lanzado (intento $intento, transmitiendo a 127.0.0.1:$PUERTO)"
    # El arranque es LENTO: en el contenedor (render por software) el motor
    # tarda mas de 100 s en cargar la partida (tiene que generar los mipmaps
    # de 1,3 GB de atlas).  Con menos de esto el banco falla sin que haya
    # ningun fallo de verdad.
    for t in $(seq 1 150); do
        "$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null
        if grep -q "cima_tipo=4" "$LOG_LOCAL" 2>/dev/null; then
            en_juego=1
            break
        fi
        if grep -qE "SEGV|SIGILL|FATAL|Received 6" "$LOG_LOCAL" 2>/dev/null; then
            say "    el motor se ha caido al cargar; se reintenta"
            break
        fi
        sleep 2
    done
    [ "$en_juego" = "1" ] && break
done
[ "$en_juego" = "1" ] || fail "el motor no llego a estar en juego"
ok "partida cargada"

# --- crear una maquina al lado del jugador (para que se dibuje el cubo) ---
COMANDO_CONSOLA='/c local p = game.player.position; local e = game.surfaces[1].create_entity{name="assembling-machine-1", position={p.x+4, p.y}}; if e then game.print("MQUINA CREADA") else game.print("NO SE PUDO") end'
printf '%s' "$COMANDO_CONSOLA" > "$LOG_DIR/consola_stream.txt"
"$PODMAN" cp "$LOG_DIR/consola_stream.txt" "$CONTAINER:/tmp/consola_stream.txt" >/dev/null 2>&1 || true
# Sin gestor de ventanas en el Xvfb la ventana del juego no tiene el foco, y
# las teclas se pierden: hay que enfocarla antes de teclear.
enfocar_juego() {
    "$PODMAN" exec "$CONTAINER" bash -c "
        for w in \$(DISPLAY=$DISPLAY_NUM xdotool search --name 'Factorio' 2>/dev/null | head -3); do
            DISPLAY=$DISPLAY_NUM xdotool windowfocus \$w 2>/dev/null || true
            DISPLAY=$DISPLAY_NUM xdotool windowactivate \$w 2>/dev/null || true
        done
        true
    " >/dev/null 2>&1 || true
}
# OJO: el RCON del cliente grafico NO se abre (--rcon-port solo aplica al
# servidor), asi que la maquina se crea con la consola del juego.  Y la
# partida de campana es la que hay que usar: la de freeplay abre un dialogo
# de bienvenida MODAL que se come el teclado (la consola no abre).
for intento in 1 2 3; do
    enfocar_juego
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key grave" >/dev/null 2>&1 || true
    sleep 2
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool type --delay 25 --file /tmp/consola_stream.txt" >/dev/null 2>&1 || true
    sleep 1
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key Return" >/dev/null 2>&1 || true
    sleep 8
    # Cerrar la consola del juego: si se deja abierta, sale en la captura.
    enfocar_juego
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key Escape" >/dev/null 2>&1 || true
    "$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null
    if grep -q "caja: mundo=" "$LOG_LOCAL" 2>/dev/null; then
        ok "maquina creada (el cubo 3D se dibuja)"
        break
    fi
    say "    la consola del juego no respondio; se reintenta"
    "$PODMAN" exec "$CONTAINER" bash -c "DISPLAY=$DISPLAY_NUM xdotool key Escape" >/dev/null 2>&1 || true
    sleep 2
done

# --------------------------------------------------------------------------
say ""
say "=== 7. Esperando frames con el cubo 3D ==="
frames=0
con_cubo=0
max_cubo=0
for t in $(seq 1 40); do
    "$PODMAN" exec "$CONTAINER" cat "$RESUMEN_CONT" > "$RESUMEN_LOCAL" 2>/dev/null
    if [ -s "$RESUMEN_LOCAL" ]; then
        frames=$(python3 -c "import json;print(json.load(open('$RESUMEN_LOCAL')).get('frames',0))" 2>/dev/null || echo 0)
        con_cubo=$(python3 -c "import json;print(json.load(open('$RESUMEN_LOCAL')).get('frames_con_cubo',0))" 2>/dev/null || echo 0)
        max_cubo=$(python3 -c "import json;print(json.load(open('$RESUMEN_LOCAL')).get('max_pixeles_cubo',0))" 2>/dev/null || echo 0)
    fi
    if [ "${con_cubo:-0}" -gt 0 ] 2>/dev/null; then
        break
    fi
    sleep 2
done

"$PODMAN" exec "$CONTAINER" cat "$LOG_CLIENT" > "$LOG_LOCAL" 2>/dev/null
"$PODMAN" exec "$CONTAINER" cat "$RESUMEN_CONT" > "$RESUMEN_LOCAL" 2>/dev/null || true

say "--- resumen del receptor ---"
cat "$RESUMEN_LOCAL" 2>/dev/null | sed 's/^/    /' || true

say ""
say "--- lineas del inyector sobre la transmision ---"
grep -E "transmisi|frame transmitido|visor conectado|no se pudo conectar" "$LOG_LOCAL" 2>/dev/null | head -12 | sed 's/^/    /' || true

if [ "${frames:-0}" -lt 1 ] 2>/dev/null; then
    say ""
    say "--- ultimas lineas del cliente ---"
    tail -15 "$LOG_LOCAL" 2>/dev/null || true
    fail "no llego ningun frame al receptor"
fi
ok "llegaron $frames frames ($STREAM_ANCHO x $STREAM_ALTO objetivo)"

# La maquina tiene que existir: sin ella no hay cubo que dibujar y el banco
# no estaria comprobando nada.
if ! grep -q "caja: mundo=" "$LOG_LOCAL" 2>/dev/null; then
    fail "no se creo la maquina: el hook de la entidad no llego a la caja (no hay 3D que enviar)"
fi
ok "la maquina se creo (hay entidad que se dibuja en 3D)"

# El cubo son miles de pixeles magenta: un par de pixeles seria ruido.
if [ "${max_cubo:-0}" -lt 100 ] 2>/dev/null; then
    fail "los frames llegaron pero el cubo 3D no se ve (max_pixeles_cubo=$max_cubo, hacen falta cientos)"
fi
ok "$con_cubo frames con el cubo 3D dentro (maximo $max_cubo pixeles)"

# Con la camara desplazada, las dos vistas tienen que diferir: el cubo sale
# mas a la izquierda en una que en la otra.  Es el paralaje real.
X_IZQ=$(python3 -c "import json;v=json.load(open('$RESUMEN_LOCAL')).get('cubo_x_izq');print(v if v is not None else -1)" 2>/dev/null || echo -1)
X_DER=$(python3 -c "import json;v=json.load(open('$RESUMEN_LOCAL')).get('cubo_x_der');print(v if v is not None else -1)" 2>/dev/null || echo -1)
if [ "$CAMARA_DESPLAZAMIENTO" != "0" ]; then
    if [ "$X_IZQ" = "-1" ] || [ "$X_DER" = "-1" ]; then
        fail "no hay frames de los dos ojos (izq=$X_IZQ der=$X_DER)"
    fi
    DIF=$(( X_IZQ > X_DER ? X_IZQ - X_DER : X_DER - X_IZQ ))
    if [ "$DIF" -lt 5 ]; then
        fail "las dos vistas son casi iguales (cubo izq=$X_IZQ der=$X_DER): la camara no se desplaza"
    fi
    ok "paralaje real: el cubo sale en x=$X_IZQ (izq) y x=$X_DER (der), diferencia $DIF px"
fi

# El frame con mas pixeles del cubo (lo guarda el receptor).
if "$PODMAN" exec "$CONTAINER" test -f /factorio/frame_cubo.png 2>/dev/null; then
    "$PODMAN" exec "$CONTAINER" cat /factorio/frame_cubo.png > "$LOG_DIR/frame_cubo.png" 2>/dev/null || true
    ok "frame con el cubo: $LOG_DIR/frame_cubo.png"
fi

# El PNG de diagnostico es lo que se manda de verdad: se reduce DENTRO del
# contenedor (los ficheros grandes se cortan al copiarlos) y se trae la
# version pequena para poder mirarla.
"$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /tmp/reducir_captura.py' < "$PROJECT_ROOT/scripts/reducir_captura.py" >/dev/null 2>&1 || true
"$PODMAN" exec "$CONTAINER" python3 /tmp/reducir_captura.py \
    /factorio/stream_debug.png /factorio/stream_debug_small.png >/dev/null 2>&1 || true
if "$PODMAN" exec "$CONTAINER" test -f /factorio/stream_debug_small.png 2>/dev/null; then
    "$PODMAN" exec "$CONTAINER" cat /factorio/stream_debug_small.png > "$LOG_DIR/stream_debug_small.png" 2>/dev/null || true
    ok "imagen de lo que se manda: $LOG_DIR/stream_debug_small.png"
fi

say ""
say "OK: el render real del juego llega al visor. Banco en verde."
exit 0
