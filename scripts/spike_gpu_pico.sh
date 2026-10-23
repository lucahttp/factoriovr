#!/usr/bin/env bash
set -uo pipefail

# spike_gpu_pico.sh — F0: ¿hay GPU real en la Pico 4 Ultra y cuántos FPS da
# el cliente grafico de Factorio?
#
# Este es el spike BLOQUEANTE de la variante V1 (el motor dibuja).
#
# Que mide:
#   1. Si el proceso del shell puede abrir el nodo de GPU del kernel
#      (/dev/kgsl-3d0). En la Pico 4 Ultra el nodo es 666, asi que sin root
#      puede bastar (lo que decide SELinux).
#   2. Si TURNIP (el driver abierto de Adreno de Mesa, que ya esta en el
#      payload) arranca dentro del proceso glibc del motor. Si arranca, el
#      motor imprime "Initialised OpenGL:[1] zink Vulkan 1.4(Adreno ...)".
#      Si no, sale "llvmpipe" (lavapipe, software).
#   3. Cuantos FPS da el motor (contador de frames del inyector), si el
#      inyector puede parchear codigo sin root.
#
# Por que Turnip y no el ICD de Adreno de /vendor: el del fabricante es un .so
# de BIONIC y se rompe dentro de un proceso glibc (dos libc en el mismo
# proceso). Turnip habla con el kernel por /dev/kgsl-3d0 y es glibc puro.
#
# Codigos de salida:
#   0  GPU real (Turnip ha arrancado)
#   1  Turnip NO ha arrancado (sigue en software) o fallo real
#   2  no se pudo VALIDAR (no hay dispositivo, no hay runtime...)
#
# Uso: bash scripts/spike_gpu_pico.sh [--timeout 180] [--lavapipe]
#                            [--solo-informe] [--x11]

ADB="${ADB:-$HOME/Library/Android/sdk/platform-tools/adb}"
REMOTE_MESA="/data/local/tmp/mesa"
REMOTE_LAUNCHER="/data/local/tmp/start_factorio_client.sh"
REMOTE_LOG="/data/local/tmp/spike_gpu.log"
TIMEOUT=180
SOLO_INFORME=0
USAR_LAVAPIPE=0
MODO_X11=0

# OJO: el parsing se hace con un while, no con "for arg in $@": con el for, un
# "shift" de un caso anterior desplaza los parametros y el valor de --timeout
# se come el nombre de la opcion.
while [ $# -gt 0 ]; do
    case "$1" in
        --timeout) TIMEOUT="${2:-180}"; shift 2 ;;
        --solo-informe) SOLO_INFORME=1; shift ;;
        --lavapipe) USAR_LAVAPIPE=1; shift ;;
        --x11) MODO_X11=1; shift ;;
        *) shift ;;
    esac
done

say()  { printf '%s\n' "$*"; }
fail() { printf 'FALLO: %s\n' "$*" >&2; exit 1; }
nada() { printf 'NO VALIDABLE: %s\n' "$*" >&2; exit 2; }

# --------------------------------------------------------------------------
say "=== 1. Dispositivo ==="
if [ ! -x "$ADB" ]; then
    nada "no se encuentra adb en $ADB"
fi
"$ADB" start-server >/dev/null 2>&1 || true
DEVICE=$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1; exit}')
if [ -z "$DEVICE" ]; then
    nada "no hay ninguna Pico conectada por adb (USB + ADB activado en las opciones de desarrollo)"
fi
say "    dispositivo: $DEVICE"
OS_ID=$("$ADB" shell getprop ro.build.display.id 2>/dev/null | tr -d '\r')
MODEL=$("$ADB" shell getprop ro.product.model 2>/dev/null | tr -d '\r')
say "    PICO OS: ${OS_ID:-?}  modelo: ${MODEL:-?}"

# --------------------------------------------------------------------------
say ""
say "=== 2. Acceso al nodo de GPU del kernel ==="
# Root (su) no es necesario si el nodo es 666 y SELinux lo permite.
if "$ADB" shell su -c id 2>/dev/null | tr -d '\r' | grep -q 'uid=0'; then
    say "    root: si (su)"
    SU="su -c"
else
    say "    root: no (se prueba sin root; el nodo /dev/kgsl-3d0 es 666 en la Pico)"
    SU=""
fi
KGSL=$("$ADB" shell ls -l /dev/kgsl-3d0 2>/dev/null | tr -d '\r')
say "    /dev/kgsl-3d0: ${KGSL:-no accesible}"
GPU_MODEL=$("$ADB" shell cat /sys/class/kgsl/kgsl-3d0/gpu_model 2>/dev/null | tr -d '\r')
say "    GPU: ${GPU_MODEL:-desconocida}"
say "    SELinux: $("$ADB" shell getenforce 2>/dev/null | tr -d '\r')"

# --------------------------------------------------------------------------
say ""
say "=== 3. Payload de Mesa ==="
TURNIP_SO="$REMOTE_MESA/usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so"
if "$ADB" shell test -f "$TURNIP_SO" 2>/dev/null; then
    say "    turnip: $TURNIP_SO"
    HAY_TURNIP=1
else
    say "    turnip: NO esta en el dispositivo (scripts/prepare_mesa.py + deploy_pico.sh)"
    HAY_TURNIP=0
fi
if ! "$ADB" shell test -f "$REMOTE_LAUNCHER" 2>/dev/null; then
    nada "no esta el launcher del cliente en el dispositivo"
fi
if [ "$SOLO_INFORME" = "1" ]; then
    say ""
    say "(--solo-informe: no se lanza nada)"
    exit 0
fi

# --------------------------------------------------------------------------
say ""
say "=== 4. Lanzando el cliente grafico ==="
"$ADB" shell rm -f "$REMOTE_LOG" >/dev/null 2>&1 || true
"$ADB" shell "pkill -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true
sleep 2

if [ "$USAR_LAVAPIPE" = "1" ]; then
    ICD_ENV="XFACTORVR_USE_TURNIP=0"
    say "    (modo lavapipe: sin GPU, solo de referencia)"
elif [ "$HAY_TURNIP" = "1" ]; then
    ICD_ENV="XFACTORVR_USE_TURNIP=1"
else
    nada "no hay turnip en el dispositivo y no se pidio --lavapipe"
fi

# El cliente grafico necesita un contexto: sin servidor X (el de Termux es de
# otro usuario y no se puede leer desde el shell) se usa el driver offscreen
# de SDL con EGL surfaceless.
if [ "$MODO_X11" = "1" ]; then
    ENTORNO="$ICD_ENV DISPLAY=:99 SDL_VIDEODRIVER=x11"
else
    ENTORNO="$ICD_ENV SDL_VIDEODRIVER=offscreen EGL_PLATFORM=surfaceless"
fi
ENTORNO="$ENTORNO WIDTH=640 HEIGHT=360 XFACTORVR_HOOK_EVERY=10"

say "    entorno: $ENTORNO"
# nohup + '&' para que el proceso sobreviva al cierre de la sesion de adb.
if [ -n "$SU" ]; then
    CMD="$ADB shell \"$SU 'cd /data/local/tmp && nohup env $ENTORNO sh $REMOTE_LAUNCHER > $REMOTE_LOG 2>&1 &'\""
else
    CMD="$ADB shell \"cd /data/local/tmp && nohup env $ENTORNO sh $REMOTE_LAUNCHER > $REMOTE_LOG 2>&1 &'\""
fi
eval "$CMD" >/dev/null 2>&1 || say "    (aviso: el comando de arranque dio error; se sigue mirando el log)"
sleep 3
if [ ! -s "$REMOTE_LOG" ] && [ "$("$ADB" shell "ps -ef | grep -c '[b]in/arm64/factorio'" | tr -d '\r')" = "0" ]; then
    say "    (aviso: el motor no arranco; se intenta en primer plano para ver el error)"
    "$ADB" shell "cd /data/local/tmp && timeout 25 env $ENTORNO sh $REMOTE_LAUNCHER" 2>&1 | tr -d '\r' | tail -20
fi

say "    esperando hasta ${TIMEOUT}s a que el motor inicialice OpenGL..."
for i in $(seq 1 "$TIMEOUT"); do
    LOG=$("$ADB" shell cat "$REMOTE_LOG" 2>/dev/null | tr -d '\r')
    if echo "$LOG" | grep -q "Initialised OpenGL"; then
        break
    fi
    sleep 1
done
LOG=$("$ADB" shell cat "$REMOTE_LOG" 2>/dev/null | tr -d '\r')

say ""
say "=== 5. Resultado ==="
GL_LINE=$(echo "$LOG" | grep -m1 "Initialised OpenGL" || true)
if [ -z "$GL_LINE" ]; then
    say "--- ultimas lineas del log ---"
    echo "$LOG" | tail -30
    say ""
    fail "el motor no llego a inicializar OpenGL (no hay linea 'Initialised OpenGL')"
fi
say "--- renderer del motor ---"
say "    $GL_LINE"
say "--- ICD usado ---"
echo "$LOG" | grep -m2 -E "turnip|lavapipe|ICD" | sed 's/^/    /' || true

if echo "$GL_LINE" | grep -qiE "adreno"; then
    say ""
    say "OK: TURNIP HA ARRANCADO -> el motor esta usando la GPU de verdad (Adreno)."
    GPU_OK=1
elif echo "$GL_LINE" | grep -qiE "llvmpipe|lavapipe|swrast"; then
    say ""
    say "AVISO: el motor sigue en SOFTWARE (llvmpipe/lavapipe): Turnip NO arranco."
    say "       Causas probables: SELinux no deja abrir /dev/kgsl-3d0 al usuario shell,"
    say "       o el ICD de Turnip no encuentra el nodo. Ver el log de Turnip."
    GPU_OK=0
else
    say ""
    say "AVISO: renderer desconocido: hay que mirarlo a mano."
    GPU_OK=0
fi

# --- El inyector: ¿puede parchear codigo sin root? ---
say ""
say "--- inyector ---"
if echo "$LOG" | grep -q "detour instalado"; then
    say "    el inyector pudo parchear el motor: los hooks y el contador de frames funcionan."
else
    say "    el inyector NO pudo parchear (probable SELinux sin root):"
    echo "$LOG" | grep -aE "xfv-injector" | tail -6 | sed 's/^/      /'
fi

# --- FPS: el inyector cuenta los frames del motor ---
say ""
say "--- FPS del motor (contador del inyector) ---"
FRAME_RE='hook vivo: ([0-9]+) frames'
N1=$(echo "$LOG" | grep -oE "$FRAME_RE" | tail -1 | grep -oE '[0-9]+' || true)
T1=$(date +%s)
sleep 10
LOG=$("$ADB" shell cat "$REMOTE_LOG" 2>/dev/null | tr -d '\r')
N2=$(echo "$LOG" | grep -oE "$FRAME_RE" | tail -1 | grep -oE '[0-9]+' || true)
T2=$(date +%s)
if [ -n "$N1" ] && [ -n "$N2" ] && [ "$N2" -gt "$N1" ]; then
    DT=$((T2 - T1)); [ "$DT" -lt 1 ] && DT=1
    FPS=$(( (N2 - N1) / DT ))
    say "    frames: $N1 -> $N2 en ${DT}s  =>  ${FPS} FPS del motor"
else
    say "    no se pudo medir (sin inyector funcionando)"
fi

"$ADB" shell "pkill -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true
say ""
say "log completo del spike: $REMOTE_LOG (en el dispositivo)"
[ "${GPU_OK:-0}" = "1" ] && exit 0
exit 1
