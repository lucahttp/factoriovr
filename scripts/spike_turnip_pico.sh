#!/usr/bin/env bash
set -uo pipefail

# spike_turnip_pico.sh — ¿Puede el motor grafico usar la GPU de verdad en el
# visor Pico 4 Ultra (sin root y sin Termux)?
#
# QUE MIDE Y POR QUE EN DOS PARTES
#
#   Parte 1 (scripts/probe_kgsl.c): lo que el KERNEL deja hacer.
#       Abre /dev/kgsl-3d0 y prueba el ioctl de RESERVA de memoria de GPU
#       (IOCTL_KGSL_GPUMEM_ALLOC_ID).  En el visor el nodo es 666, asi que se
#       abre; lo que puede estar prohibido es la reserva.  Precedente medido
#       en otro Android con Adreno y SELinux enforcing (DroidDeck #245, Quest
#       3S): la consulta funciona, la reserva devuelve EACCES y Turnip lo
#       traduce a VK_ERROR_OUT_OF_DEVICE_MEMORY.  Sin esta parte no se puede
#       distinguir "Turnip no arranca" de "la GPU no deja reservar memoria".
#
#   Parte 2 (barrido de ICD): lo que el motor logra abrir.
#       Se prueba el cliente grafico real con distintas combinaciones de ICD
#       de Vulkan y se lee su linea "Initialised OpenGL", que dice el
#       renderer con el que ha abierto el contexto.
#
# POR QUE "MESA_LOADER_DRIVER_OVERRIDE=zink" Y NO "kgsl"
#
#   Verificado en los binarios del payload (build/mesa):
#     - libvulkan.so.1 (el loader de Vulkan) NO lee MESA_LOADER_DRIVER_OVERRIDE
#       (la cadena no aparece en el binario).  El ICD de Vulkan se elige solo
#       con VK_ICD_FILENAMES / VK_DRIVER_FILES.
#     - libEGL_mesa.so.0 y zink_dri.so SI lo leen: es el override del driver
#       DRI (capa GL), y ahi el driver correcto es "zink".
#   Asi que no hay conflicto: "zink" para la capa GL (necesario, porque en
#   Android no hay /dev/dri/renderD* y sin override el DRI no autodetecta
#   nada) y el ICD de Vulkan se elige por separado.  Aun asi, el barrido
#   incluye la variante "kgsl" por si el kernel de la Pico se comporta
#   distinto: es una medida, no una suposicion.
#
# CODIGOS DE SALIDA
#   0  GPU real: el motor ha abierto el contexto sobre el Adreno
#   1  la GPU no se puede usar (o el motor no abre contexto GL)
#   2  no se pudo VALIDAR (no hay dispositivo, no hay payload, ...)
#
# USO: bash scripts/spike_turnip_pico.sh [--timeout 60] [--solo-kernel]

ADB="${ADB:-$HOME/Library/Android/sdk/platform-tools/adb}"
SDK_ROOT="${SDK_ROOT:-/Volumes/X10Pro/Android/sdk}"
NDK_ROOT="${NDK_ROOT:-$SDK_ROOT/ndk/27.0.11718014}"
CLANG="$NDK_ROOT/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android34-clang"

REMOTO_MESA="/data/local/tmp/mesa"
REMOTO_GLIBC="/data/local/tmp/glibc"
REMOTO_LAUNCHER="/data/local/tmp/start_factorio_client.sh"
REMOTO_LOG="/data/local/tmp/spike_turnip.log"
REMOTO_PROBE="/data/local/tmp/probe_kgsl"
MESA_LIB="$REMOTO_MESA/usr/lib/aarch64-linux-gnu"

TIMEOUT=60
SOLO_KERNEL=0

while [ $# -gt 0 ]; do
    case "$1" in
        --timeout) TIMEOUT="${2:-60}"; shift 2 ;;
        --solo-kernel) SOLO_KERNEL=1; shift ;;
        *) shift ;;
    esac
done

say()  { printf '%s\n' "$*"; }
fail() { printf 'FALLO: %s\n' "$*" >&2; exit 1; }
nada() { printf 'NO VALIDABLE: %s\n' "$*" >&2; exit 2; }

# --------------------------------------------------------------------------
say "=== 0. Dispositivo y payload ==="
if [ ! -x "$ADB" ]; then
    nada "no se encuentra adb en $ADB"
fi
"$ADB" start-server >/dev/null 2>&1 || true
DEVICE=$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1; exit}')
if [ -z "$DEVICE" ]; then
    nada "no hay ninguna Pico conectada por adb (USB + ADB activado en las opciones de desarrollo)"
fi
say "    dispositivo: $DEVICE"
say "    PICO OS: $("$ADB" shell getprop ro.build.display.id 2>/dev/null | tr -d '\r')"
say "    GPU:     $("$ADB" shell cat /sys/class/kgsl/kgsl-3d0/gpu_model 2>/dev/null | tr -d '\r')"
say "    SELinux: $("$ADB" shell getenforce 2>/dev/null | tr -d '\r')"
say "    nodo:    $("$ADB" shell ls -l /dev/kgsl-3d0 2>/dev/null | tr -d '\r')"

if [ ! -d "$REMOTO_MESA" ]; then
    nada "no esta el payload de Mesa en el dispositivo (bash scripts/deploy_pico.sh)"
fi
if [ ! -f "$MESA_LIB/libvulkan_freedreno.so" ]; then
    say "    AVISO: no esta Turnip (libvulkan_freedreno.so): no se podra probar la GPU real"
fi

# --------------------------------------------------------------------------
say ""
say "=== 1. Lo que deja hacer el KERNEL (sonda de ioctls) ==="
if [ ! -x "$CLANG" ]; then
    nada "no se encuentra el compilador del NDK en $CLANG"
fi
say "    compilando la sonda..."
"$CLANG" -O2 -Wall -o build/probe_kgsl scripts/probe_kgsl.c || nada "no se pudo compilar la sonda"
"$ADB" push build/probe_kgsl "$REMOTE_PROBE" >/dev/null 2>&1 || nada "no se pudo subir la sonda"
"$ADB" shell chmod 755 "$REMOTE_PROBE" >/dev/null 2>&1 || true

"$ADB" shell "$REMOTE_PROBE" 2>&1 | tr -d '\r' | sed 's/^/    /'
KERNEL_RC=$?

# El veredicto del kernel decide si tiene sentido seguir con el barrido.
if [ "$KERNEL_RC" != "0" ]; then
    say ""
    say "AVISO: el kernel NO deja reservar memoria de GPU a este proceso."
    say "       Turnip no puede crear el device Vulkan por mucho que se ajuste"
    say "       el entorno: el fallo seria de policy, no de configuracion."
fi

if [ "$SOLO_KERNEL" = "1" ]; then
    say ""
    say "(--solo-kernel: no se lanza el motor)"
    exit "$KERNEL_RC"
fi

# --------------------------------------------------------------------------
say ""
say "=== 2. Barrido de ICD de Vulkan con el motor real ==="
if [ ! -f "$REMOTE_LAUNCHER" ]; then
    nada "no esta el launcher del cliente en el dispositivo"
fi

# Prepara los dos JSON de ICD con las rutas DEL DISPOSITIVO (los del payload
# apuntan a /usr/lib/..., que en el visor no existe).
"$ADB" shell "cat > $REMOTO_MESA/turnip_icd.json <<'JSON'
{
    \"file_format_version\": \"1.0.0\",
    \"ICD\": { \"library_path\": \"$MESA_LIB/libvulkan_freedreno.so\", \"api_version\": \"1.1.230\" }
}
JSON
cat > $REMOTE_MESA/lvp_icd.json <<'JSON'
{
    \"file_format_version\": \"1.0.0\",
    \"ICD\": { \"library_path\": \"$MESA_LIB/libvulkan_lvp.so\", \"api_version\": \"1.1.230\" }
}
JSON" >/dev/null 2>&1

# Cada caso: nombre | ICD | MESA_LOADER_DRIVER_OVERRIDE | vars extra
CASOS=(
    "A: turnip + zink|turnip|zink|VK_LOADER_DEBUG=warn,driver"
    "B: lavapipe + zink (CONTROL)|lvp|zink|"
    "C: turnip + kgsl|turnip|kgsl|VK_LOADER_DEBUG=warn,driver"
    "D: turnip + zink + GALLIUM_DRIVER|turnip|zink|GALLIUM_DRIVER=zink"
)

HAY_GPU=0
for CASO in "${CASOS[@]}"; do
    IFS='|' read -r NOMBRE ICD OVERRIDE EXTRA <<< "$CASO"
    say ""
    say "--- caso $NOMBRE ---"
    "$ADB" shell rm -f "$REMOTE_LOG" >/dev/null 2>&1 || true
    "$ADB" shell "pkill -9 -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true
    sleep 2
    "$ADB" shell rm -f /data/local/tmp/factorio/.lock >/dev/null 2>&1 || true

    ENTORNO="MESA_LOADER_DRIVER_OVERRIDE=$OVERRIDE"
    ENTORNO="$ENTORNO VK_ICD_FILENAMES=$REMOTE_MESA/${ICD}_icd.json"
    ENTORNO="$ENTORNO VK_DRIVER_FILES=$REMOTO_MESA/${ICD}_icd.json"
    ENTORNO="$ENTORNO __EGL_VENDOR_LIBRARY_FILENAMES=$MESA_LIB/libEGL_mesa.so.0"
    ENTORNO="$ENTORNO LIBGL_DRIVERS_PATH=$MESA_LIB/dri"
    ENTORNO="$ENTORNO LIBGL_ALWAYS_SOFTWARE=0 ZINK_DESCRIPTORS=auto"
    ENTORNO="$ENTORNO SDL_VIDEODRIVER=offscreen EGL_PLATFORM=surfaceless"
    ENTORNO="$ENTORNO WIDTH=640 HEIGHT=360 XFACTORVR_HOOK_EVERY=60"
    [ -n "$EXTRA" ] && ENTORNO="$ENTORNO $EXTRA"

    "$ADB" shell "cd /data/local/tmp && nohup env $ENTORNO sh $REMOTE_LAUNCHER > $REMOTE_LOG 2>&1 &" >/dev/null 2>&1

    LINEA=""
    for i in $(seq 1 "$TIMEOUT"); do
        LOG=$("$ADB" shell cat "$REMOTE_LOG" 2>/dev/null | tr -d '\r')
        LINEA=$(echo "$LOG" | grep -m1 "Initialised OpenGL" || true)
        [ -n "$LINEA" ] && break
        sleep 1
    done

    if [ -z "$LINEA" ]; then
        say "    el motor NO inicializo OpenGL en ${TIMEOUT}s"
        LOG=$("$ADB" shell cat "$REMOTE_LOG" 2>/dev/null | tr -d '\r')
        echo "$LOG" | grep -iE 'vulkan|egl|adreno|turnip|llvmpipe|zink|error' | tail -5 | sed 's/^/      /'
        continue
    fi
    say "    $LINEA"
    if echo "$LINEA" | grep -qiE 'adreno|freedreno'; then
        say "    ==> GPU REAL (Adreno)."
        HAY_GPU=1
    elif echo "$LINEA" | grep -qiE 'llvmpipe|lavapipe|swrast'; then
        say "    ==> software (lavapipe): la cadena GL funciona, pero no hay GPU."
    else
        say "    ==> renderer desconocido: hay que mirarlo a mano."
    fi
done

"$ADB" shell "pkill -9 -f 'bin/arm64/factorio'" >/dev/null 2>&1 || true

say ""
say "=== 3. Veredicto ==="
if [ "$HAY_GPU" = "1" ]; then
    say "OK: el motor ha abierto el contexto sobre la GPU de verdad."
    exit 0
fi
say "El motor no ha conseguido un contexto sobre el Adreno."
if [ "$KERNEL_RC" != "0" ]; then
    say "Causa probable: SELinux no deja reservar memoria de GPU (lo dice la parte 1)."
    say "Sin root no hay arreglo desde el usuario: el driver del fabricante"
    say "reserva por el gralloc, que si es privilegiado."
else
    say "El kernel si deja reservar memoria: el problema esta en el ICD o en la"
    say "cadena EGL. Con el caso B (lavapipe) en verde, el problema es el ICD;"
    say "con el caso B en rojo, es la cadena GL/EGL."
fi
exit 1
