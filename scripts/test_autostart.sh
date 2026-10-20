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
# La comprobacion es de verdad, en dos partes:
#   (a) el RCON ABRE y responde a /version
#   (b) el JUEGO AVANZA: dos /time seguidas dan valores DISTINTOS
# (leer "Loading map ..." en el log NO prueba nada: el motor lee el fichero y
# luego se puede quedar atascado sin llegar a crear el mapa).
#
# Uso: bash scripts/test_autostart.sh [--timeout SEGUNDOS] [--ruta SAVE]
#                                     [--via gui|injector]
#
# Variables de entorno:
#   XFACTORVR_AUTOSTART   que arrancar (por defecto, el save del banco)
#   PODMAN                ruta del podman (por defecto /opt/podman/bin/podman)
#
# Modos:
#   --via injector  (por defecto) el inyector arranca la partida con
#                   `AppManager::createGame()` / la caja de carga.
#   --via gui       arranca el cliente SIN argumentos de carga y conduce el
#                   menu con el raton (xdotool) hasta el boton "Continue":
#                   es la via que usaria una persona (o un dedo en Android).
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
VIA="injector"

for arg in "$@"; do
    case "$arg" in
        --timeout) TIMEOUT="${2:-210}"; shift ;;
        --ruta) SAVE_POR_DEFECTO="${2:-$SAVE_POR_DEFECTO}"; shift ;;
        --via) VIA="${2:-injector}"; shift ;;
    esac
done

# Que arrancar. Por defecto, el save del banco.
AUTOSTART="${XFACTORVR_AUTOSTART:-$SAVE_POR_DEFECTO}"

LOG_CLIENT="/factorio/autostart_test.log"
LOG_MAPAS="/factorio/maps.txt"
# Tamano de la ventana del juego (el menu se dibuja aqui arriba a la izq).
VENTANA_ANCHO=640
VENTANA_ALTO=360

say()  { printf '%s\n' "$*"; }
ok()   { printf '  OK   %s\n' "$*"; }
fail() { printf '  FALLO %s\n' "$*"; }
info() { printf '  ..   %s\n' "$*"; }

say "=== test_autostart.sh ==="
say "  contenedor : $CONTAINER"
say "  arrancar   : $AUTOSTART"
say "  via        : $VIA"
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

# Argumentos de carga: en modo GUI NO se pasan (el motor cargara el save
# cuando se pulse "Continue"); en modo inyector se pasa el save al inyector.
if [ "$VIA" = "gui" ]; then
    CARGA_CLIENTE=""
    AUTOSTART_ENV=""
else
    CARGA_CLIENTE=""
    AUTOSTART_ENV="export XFACTORVR_AUTOSTART='$AUTOSTART'
export XFACTORVR_AUTOSTART_DELAY_S=6"
fi

"$PODMAN" exec -d "$CONTAINER" bash -c "
export DISPLAY=$DISPLAY_NUM
export SDL_VIDEODRIVER=x11
export LIBGL_ALWAYS_SOFTWARE=1
export LD_PRELOAD=/injector.so
export XFACTORVR_HOOK_EVERY=50
export XFACTORVR_DIAG_PILA=1
export XFACTORVR_DIAG_CADA=100
$AUTOSTART_ENV
cd /factorio
exec ./bin/arm64/factorio --window-size ${VENTANA_ANCHO}x${VENTANA_ALTO} --rcon-port $RCON_PORT --rcon-password $RCON_PASS > $LOG_CLIENT 2>&1
"
ok "cliente lanzado (RCON en el puerto $RCON_PORT)"

# --- 2b. Modo GUI: pulsar "Continue" con el raton -------------------------
if [ "$VIA" = "gui" ]; then
    say ""
    say "--- 2b. Conduciendo el menu con el raton (boton Continue) ---"
    # El menu tarda ~30 s en aparecer (carga los atlas).
    for t in $(seq 1 40); do
        if "$PODMAN" exec "$CONTAINER" bash -c \
            "grep -q 'Factorio initialised' $LOG_CLIENT" 2>/dev/null; then
            break
        fi
        sleep 2
    done
    sleep 5
    # El boton "Continue" es el primero de la lista, en el panel de la
    # izquierda: a media anchura de la ventana y a un 14% de su altura.
    CX=$((VENTANA_ANCHO / 2))
    CY=$((VENTANA_ALTO * 14 / 100))
    info "clic en Continue ($CX,$CY)"
    "$PODMAN" exec "$CONTAINER" bash -c "
export DISPLAY=$DISPLAY_NUM
xdotool mousemove $CX $CY 2>/dev/null
sleep 1
xdotool click 1 2>/dev/null
" >/dev/null 2>&1
    ok "clic enviado"
fi

# --- 3. Esperar a que la partida arranque -------------------------------
say ""
say "--- 3. Esperando a que la partida arranque ---"

# El probe de RCON se ejecuta en la maquina de podman (el contenedor no tiene
# python3), y desde ahi se llega al contenedor por la red de podman.
"$PODMAN" machine ssh -- bash -c 'cat > /tmp/xfv_rcon_probe.py' < "$PROJECT_ROOT/scripts/rcon_probe.py" >/dev/null 2>&1

IP_CONT="$("$PODMAN" inspect "$CONTAINER" --format '{{.NetworkSettings.IPAddress}}' 2>/dev/null | tr -d '\r')"
[ -n "$IP_CONT" ] || IP_CONT="10.88.0.2"

# (a) El RCON ABRE y RESPONDE. El RCON del cliente grafico solo se abre
#     cuando la partida esta en marcha, asi que que conteste ya es una
#     prueba de que el juego llego a esa fase.
# (b) El JUEGO AVANZA: dos consultas a `/time` separadas unos segundos tienen
#     que dar valores DISTINTOS. Asi se prueba que el tick del mapa avanza de
#     verdad y no que solo se leyo el fichero del save.
#
# OJO: leer "Loading map ..." en el log NO prueba nada de esto: el motor lee
# el fichero y luego se puede quedar atascado sin llegar a crear el mapa.
rcon_ok=0
tick_ok=0
tiempo_1=""
tiempo_2=""
fin=$((SECONDS + TIMEOUT))
while [ $SECONDS -lt $fin ]; do
    if [ "$rcon_ok" = "0" ]; then
        salida="$("$PODMAN" machine ssh -- python3 /tmp/xfv_rcon_probe.py "$IP_CONT" "$RCON_PORT" "$RCON_PASS" "/version" 2>&1 | tr -d '\r')"
        case "$salida" in
            *"ERROR"*) : ;;                       # el RCON no contesto
            *"respuesta"*) rcon_ok=1 ;;          # contesto de verdad
        esac
    fi
    if [ "$rcon_ok" = "1" ] && [ "$tick_ok" = "0" ]; then
        t1="$("$PODMAN" machine ssh -- python3 /tmp/xfv_rcon_probe.py "$IP_CONT" "$RCON_PORT" "$RCON_PASS" "/time" 2>&1 | tr -d '\r' | grep -oE 'The time is [0-9.]+' | head -1)"
        sleep 6
        t2="$("$PODMAN" machine ssh -- python3 /tmp/xfv_rcon_probe.py "$IP_CONT" "$RCON_PORT" "$RCON_PASS" "/time" 2>&1 | tr -d '\r' | grep -oE 'The time is [0-9.]+' | head -1)"
        if [ -n "$t1" ] && [ -n "$t2" ] && [ "$t1" != "$t2" ]; then
            tick_ok=1
            tiempo_1="$t1"
            tiempo_2="$t2"
        fi
    fi
    if [ "$rcon_ok" = "1" ] && [ "$tick_ok" = "1" ]; then
        break
    fi
    sleep 5
done

# --- 4. Resultado --------------------------------------------------------
say ""
say "--- 4. Resultado ---"

"$PODMAN" exec "$CONTAINER" bash -c "grep -vE 'MESA|ALSA' $LOG_CLIENT 2>/dev/null | grep -E 'Loading (map|level)|Map version|Checksum for script|carga:|Map loading failed|Map tick|SIGSEGV|Starting RCON|Heavy mode|Error '" \
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

# Diagnostico: si la pila de estados ha acabado en [InMainMenu, InGame] sin
# mapa, el motor ha cancelado la carga (ver ADR-004, seccion 12).
DIAG="$("$PODMAN" exec "$CONTAINER" bash -c "grep 'diag:' $LOG_CLIENT 2>/dev/null | tail -6" 2>/dev/null | tr -d '\r')"
if [ -n "$DIAG" ]; then
    say "  diagnostico de la pila de estados:"
    printf '%s\n' "$DIAG" | sed 's/^/    /'
    say ""
    if printf '%s' "$DIAG" | grep -q "tipo=InGame"; then
        fail "el motor dejo un InGame sin vista de control (mapa no creado): carga cancelada"
    fi
fi

say "  (a) el RCON ABRE y responde     : $([ "$rcon_ok" = "1" ] && echo SI || echo NO)"
say "  (b) el tick del juego AVANZA     : $([ "$tick_ok" = "1" ] && echo SI || echo NO)"
if [ "$tick_ok" = "1" ]; then
    say "      $tiempo_1  ->  $tiempo_2"
fi

if [ "$rcon_ok" = "1" ] && [ "$tick_ok" = "1" ]; then
    ok "la partida arranco, el RCON responde y el tick avanza"
    exit 0
fi
if [ "$rcon_ok" = "0" ]; then
    fail "el RCON NO respondio: la partida no llego a la fase de juego"
else
    fail "el RCON respondio pero el tick del juego NO avanza"
fi
exit 1
