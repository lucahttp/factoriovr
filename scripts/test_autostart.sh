#!/usr/bin/env bash
# ===========================================================================
# test_autostart.sh — Banco de pruebas del arranque automatico de partida.
#
# Comprueba que el INYECTOR arranca una partida real dentro del cliente
# GRAFICO de Factorio 2.1.22 (ARM64) y que el RCON del cliente responde.
#
# Es el complemento de scripts/test_injector.sh (que comprueba que el hook
# sigue vivo). Aqui se comprueba lo otro: que el juego arranca solo.
#
# Uso: bash scripts/test_autostart.sh [--timeout SEGUNDOS] [--ruta SAVE]
#
# Variables de entorno:
#   XFACTORVR_AUTOSTART   que arrancar (por defecto, el save del banco)
#   PODMAN                ruta del podman (por defecto /opt/podman/bin/podman)
#
# Codigos de salida:
#   0  la partida arranco y el RCON respondio   (LO CORRECTO)
#   1  la partida NO arranco, o el RCON no respondio  (FALLO)
#   2  no se pudo ni siquiera hacer la prueba (contenedor, podman, ...)
# ===========================================================================
set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
CONTAINER="xfactorvr-client"
SAVE_POR_DEFECTO="/factorio/saves/client_test.zip"
TIMEOUT=300
RCON_PORT=27015
RCON_PASS="xfvtest"

for arg in "$@"; do
    case "$arg" in
        --timeout) TIMEOUT="${2:-210}"; shift ;;
        --ruta) SAVE_POR_DEFECTO="${2:-$SAVE_POR_DEFECTO}"; shift ;;
    esac
done

# Que arrancar. Por defecto, el save del banco.
AUTOSTART="${XFACTORVR_AUTOSTART:-$SAVE_POR_DEFECTO}"

LOG_CLIENT="/factorio/autostart_test.log"
LOG_MAPAS="/factorio/maps.txt"

say()  { printf '%s\n' "$*"; }
ok()   { printf '  OK   %s\n' "$*"; }
fail() { printf '  FALLO %s\n' "$*"; }
info() { printf '  ..   %s\n' "$*"; }

say "=== test_autostart.sh ==="
say "  contenedor : $CONTAINER"
say "  arrancar   : $AUTOSTART"
say "  timeout    : ${TIMEOUT}s"

# --- 0. El banco tiene que existir ---------------------------------------
if ! "$PODMAN" container exists "$CONTAINER" 2>/dev/null; then
    fail "no existe el contenedor $CONTAINER"
    exit 2
fi
if ! "$PODMAN" container inspect "$CONTAINER" --format '{{.State.Running}}' 2>/dev/null | grep -q true; then
    info "el contenedor no estaba arrancado; lo arranco"
    "$PODMAN" start "$CONTAINER" >/dev/null 2>&1 || { fail "no se pudo arrancar el contenedor"; exit 2; }
fi

# --- 1. Compilar el inyector ---------------------------------------------
say ""
say "--- 1. Compilando el inyector ---"
if ! bash "$PROJECT_ROOT/scripts/test_injector.sh" --solo-compilar; then
    fail "no se pudo compilar el inyector"
    exit 2
fi
SO="$PROJECT_ROOT/injector-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so"
[ -f "$SO" ] || { fail "no existe $SO"; exit 2; }
"$PODMAN" cp "$SO" "$CONTAINER:/injector.so" >/dev/null 2>&1 \
    || "$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /injector.so' < "$SO" \
    || { fail "no se pudo copiar el .so al contenedor"; exit 2; }
ok "inyector copiado al contenedor"

# --- 2. Preparar el cliente ----------------------------------------------
say ""
say "--- 2. Arrancando el cliente grafico con el inyector ---"
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f 'bin/arm64/factorio'" >/dev/null 2>&1
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 Xvfb" >/dev/null 2>&1
sleep 2
"$PODMAN" exec "$CONTAINER" rm -f /factorio/.lock "$LOG_CLIENT" "$LOG_MAPAS" >/dev/null 2>&1

# Se necesita una pantalla: el cliente grafico necesita un servidor X. Con
# SDL_VIDEODRIVER=offscreen la transicion de estado del motor (que toca el
# GUI) revienta con SIGSEGV; con Xvfb + el driver x11 va bien.
"$PODMAN" exec "$CONTAINER" bash -c "pkill -9 Xvfb" >/dev/null 2>&1
sleep 1
DISPLAY_NUM=""
for d in 99 98 97 96 95; do
    "$PODMAN" exec "$CONTAINER" bash -c "rm -f /tmp/.X11-unix/X$d /tmp/.X$d-lock" >/dev/null 2>&1
    "$PODMAN" exec -d "$CONTAINER" bash -c "Xvfb :$d -screen 0 1280x720x24 -nolisten tcp > /factorio/xvfb.log 2>&1"
    # Se espera a que aparezca el socket de X (si no, el cliente muere con
    # "x11 not available"). Se mira que APAREZCA, no que exista: un socket
    # viejo de un Xvfb anterior engaña.
    listo=0
    for t in $(seq 1 30); do
        if ! "$PODMAN" exec "$CONTAINER" test -e "/tmp/.X11-unix/X$d" 2>/dev/null; then
            sleep 0.5
            continue
        fi
        # Y que ademas responda: se comprueba que el proceso Xvfb esta vivo.
        if "$PODMAN" exec "$CONTAINER" bash -c "pgrep -f 'Xvfb :$d' >/dev/null" 2>/dev/null; then
            listo=1
        fi
        break
    done
    if [ "$listo" = "1" ]; then
        DISPLAY_NUM=":$d"
        break
    fi
    "$PODMAN" exec "$CONTAINER" bash -c "pkill -9 -f 'Xvfb :$d'" >/dev/null 2>&1
    sleep 1
done
if [ -z "$DISPLAY_NUM" ]; then
    fail "no se pudo arrancar Xvfb en el contenedor (mira /factorio/xvfb.log)"
    exit 2
fi
ok "servidor X virtual listo en $DISPLAY_NUM"

"$PODMAN" exec -d "$CONTAINER" bash -c "
export DISPLAY=$DISPLAY_NUM
export SDL_VIDEODRIVER=x11
export LIBGL_ALWAYS_SOFTWARE=1
export LD_PRELOAD=/injector.so
export XFACTORVR_HOOK_EVERY=50
export XFACTORVR_AUTOSTART='$AUTOSTART'
export XFACTORVR_AUTOSTART_DELAY_S=6
cd /factorio
exec ./bin/arm64/factorio --window-size 1280x720 --rcon-port $RCON_PORT --rcon-password $RCON_PASS > $LOG_CLIENT 2>&1
"
ok "cliente lanzado (RCON en el puerto $RCON_PORT)"

# --- 3. Esperar a que la partida arranque -------------------------------
say ""
say "--- 3. Esperando a que la partida arranque ---"

# El probe de RCON se ejecuta en la maquina de podman (el contenedor no tiene
# python3), y desde ahi se llega al contenedor por la red de podman.
"$PODMAN" machine ssh -- bash -c 'cat > /tmp/xfv_rcon_probe.py' < "$PROJECT_ROOT/scripts/rcon_probe.py" >/dev/null 2>&1

IP_CONT="$("$PODMAN" inspect "$CONTAINER" --format '{{.NetworkSettings.IPAddress}}' 2>/dev/null | tr -d '\r')"
[ -n "$IP_CONT" ] || IP_CONT="10.88.0.2"

cargado=0
rcon_ok=0
fin=$((SECONDS + TIMEOUT))
while [ $SECONDS -lt $fin ]; do
    # (a) el log dice que se ha cargado el nivel / la partida
    if "$PODMAN" exec "$CONTAINER" grep -qE "Loading (map|level\.dat)" "$LOG_CLIENT" 2>/dev/null; then
        cargado=1
    fi
    # (b) el RCON responde
    if [ "$rcon_ok" = "0" ]; then
        salida="$("$PODMAN" machine ssh -- python3 /tmp/xfv_rcon_probe.py "$IP_CONT" "$RCON_PORT" "$RCON_PASS" "/version" 2>&1 | tr -d '\r')"
        case "$salida" in
            *"ERROR"*) : ;;                       # el RCON no contesto
            *"respuesta"*) rcon_ok=1 ;;          # contesto de verdad
        esac
    fi
    if [ "$cargado" = "1" ] && [ "$rcon_ok" = "1" ]; then
        break
    fi
    sleep 5
done

# --- 4. Resultado --------------------------------------------------------
say ""
say "--- 4. Resultado ---"

"$PODMAN" exec "$CONTAINER" bash -c "grep -vE 'MESA|ALSA' $LOG_CLIENT 2>/dev/null | grep -E 'Loading (map|level)|Map version|Checksum for script|Starting RCON|RemoteCommand|SIGSEGV|Error '" \
    > /tmp/xfv_autostart_evidencia.txt 2>/dev/null
if [ -s /tmp/xfv_autostart_evidencia.txt ]; then
    say "  lineas del cliente que lo demuestran:"
    sed 's/^/    /' /tmp/xfv_autostart_evidencia.txt
    say ""
fi

"$PODMAN" exec "$CONTAINER" bash -c "grep 'xfv-injector' $LOG_CLIENT 2>/dev/null | tail -12" \
    > /tmp/xfv_autostart_injector.txt 2>/dev/null
if [ -s /tmp/xfv_autostart_injector.txt ]; then
    say "  ultimas lineas del inyector:"
    sed 's/^/    /' /tmp/xfv_autostart_injector.txt
    say ""
fi

say "  (a) el nivel se cargo  : $([ "$cargado" = "1" ] && echo SI || echo NO)"
say "  (b) el RCON respondio  : $([ "$rcon_ok" = "1" ] && echo SI || echo NO)"

if [ "$cargado" = "1" ] && [ "$rcon_ok" = "1" ]; then
    ok "la partida arranco y el RCON respondio"
    exit 0
fi
if [ "$cargado" = "0" ]; then
    fail "la partida NO arranco (no se cargo ningun nivel)"
else
    fail "la partida arranco pero el RCON NO respondio"
fi
exit 1
