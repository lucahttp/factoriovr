#!/bin/bash
set -euo pipefail

# deploy_pico.sh — Despliegue de XFactorVR en Pico 4 Ultra vía ADB
#
# Deja el visor en modo standalone:
#   1. instala el APK,
#   2. copia el motor headless de Factorio 2.0 a /data/local/tmp (nunca a
#      /sdcard, que esta montado como noexec),
#   3. copia el runtime de glibc (Android usa bionic, Factorio necesita glibc),
#   4. instala el mod puente xfactorvr_bridge,
#   5. arranca el servidor Factorio con RCON en 127.0.0.1:27015,
#   6. abre la app, que se conecta al servidor por RCON.
#
# Opciones:
#   --stream    sigue los logs en vivo (Ctrl+C para salir)
#   --no-start  no arranca el servidor (solo deja todo listo)
#   --strip     recorta el binario del motor (249 MB -> 62 MB) para acelerar
#               la transferencia. Opcional: el motor funciona igual sin depurar.

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADB="${ADB:-/Volumes/X10Pro/Android/sdk/platform-tools/adb}"
APK="$PROJECT_ROOT/build/XFactorVR_Pico4Ultra.apk"
PACKAGE="com.xfactorvr.tabletop"
REMOTE_DATA="/sdcard/XFactorVR"

# El motor va a /data/local/tmp porque /sdcard esta montado como noexec.
REMOTE_FACTORIO="/data/local/tmp/factorio_arm64"
REMOTE_GLIBC="/data/local/tmp/glibc"
REMOTE_SCRIPT="/data/local/tmp/start_factorio_standalone.sh"

# Payload local del motor headless ARM64.
LOCAL_FACTORIO="$PROJECT_ROOT/factorio_arm64"
LOCAL_MOD="$PROJECT_ROOT/mods/xfactorvr_bridge"
# La carpeta del mod debe llamarse <nombre>_<version> de info.json, o Factorio
# no lo carga. Se deriva del propio info.json para que no se desincronice.
MOD_ID=$(python3 -c "
import json
d = json.load(open('$LOCAL_MOD/info.json'))
print(d['name'] + '_' + d['version'])
")

STREAM=0
START_SERVER=1
STRIP=0
for arg in "$@"; do
    case "$arg" in
        --stream)   STREAM=1 ;;
        --no-start) START_SERVER=0 ;;
        --strip)    STRIP=1 ;;
    esac
done

echo "=== Desplegando XFactorVR en Pico 4 Ultra (modo standalone) ==="

# 1. Verificar conexión ADB con el visor
DEVICE_COUNT=$("$ADB" devices | grep -v "List" | grep "device" | wc -l | tr -d ' ')
if [ "$DEVICE_COUNT" -eq 0 ]; then
    echo "No se detectó la Pico 4 Ultra conectada por USB o Wi-Fi."
    echo "Conecta el visor y asegúrate de que el Modo Desarrollador y la Depuración USB estén habilitados."
    exit 1
fi

DEVICE_ID=$("$ADB" devices | grep "device$" | head -n 1 | awk '{print $1}')
echo "Pico 4 Ultra detectada: $DEVICE_ID"

# 2. Instalar el APK
if [ ! -f "$APK" ]; then
    echo "El APK no existe. Ejecuta primero: bash scripts/build_apk.sh"
    exit 1
fi

echo "[1/6] Instalando $APK..."
"$ADB" install -r "$APK"
"$ADB" shell appops set "$PACKAGE" MANAGE_EXTERNAL_STORAGE allow 2>/dev/null || true

# 3. Motor headless de Factorio
# Se empaqueta en un .tar sin comprimir para que la transferencia sea una sola
# en vez de miles de ficheros sueltos, y porque el toybox de Android no
# descomprime xz de forma fiable.
echo "[2/6] Empaquetando el motor headless de Factorio..."
if [ ! -d "$LOCAL_FACTORIO/bin/arm64" ]; then
    echo "No se encuentra el motor local en $LOCAL_FACTORIO"
    echo "Descargalo con el tarball factorio-headless_arm64.tar.xz del repositorio."
    exit 1
fi

PAYLOAD_TAR="$PROJECT_ROOT/build/factorio_arm64.tar"
STAGE_DIR="$PROJECT_ROOT/build/factorio_stage"
mkdir -p "$PROJECT_ROOT/build"

# COPYFILE_DISABLE evita que bsdtar meta los metadatos de macOS (AppleDouble)
# dentro del tar: en Linux aparecerian como ficheros basura ._* al extraer.
COPYFILE_DISABLE=1 tar -cf "$PAYLOAD_TAR" -C "$LOCAL_FACTORIO" data config-path.cfg

if [ "$STRIP" -eq 1 ]; then
    echo "    recortando el binario del motor..."
    rm -rf "$STAGE_DIR"
    mkdir -p "$STAGE_DIR/bin/arm64"
    cp "$LOCAL_FACTORIO/bin/arm64/factorio" "$STAGE_DIR/bin/arm64/factorio"
    STRIP_BIN="/Volumes/X10Pro/Android/sdk/ndk/27.0.11718014/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-strip"
    if [ -x "$STRIP_BIN" ]; then
        "$STRIP_BIN" --strip-unneeded "$STAGE_DIR/bin/arm64/factorio"
    else
        echo "    llvm-strip no disponible; se envia el binario sin recortar."
    fi
    COPYFILE_DISABLE=1 tar -rf "$PAYLOAD_TAR" -C "$STAGE_DIR" bin
    rm -rf "$STAGE_DIR"
else
    COPYFILE_DISABLE=1 tar -rf "$PAYLOAD_TAR" -C "$LOCAL_FACTORIO" bin
fi

echo "    payload: $(du -h "$PAYLOAD_TAR" | cut -f1)"

echo "[3/6] Copiando el motor a $REMOTE_FACTORIO ..."
"$ADB" shell "mkdir -p $REMOTE_FACTORIO"
"$ADB" push "$PAYLOAD_TAR" "$REMOTE_FACTORIO/payload.tar"
"$ADB" shell "cd $REMOTE_FACTORIO && tar -xf payload.tar && rm -f payload.tar && chmod 755 bin/arm64/factorio && mkdir -p mods"

# 4. Runtime de glibc
echo "[4/6] Preparando el runtime de glibc..."
if [ ! -f "$PROJECT_ROOT/build/glibc.tar.gz" ]; then
    bash "$PROJECT_ROOT/scripts/prepare_glibc.sh"
fi

if [ -f "$PROJECT_ROOT/build/glibc.tar.gz" ]; then
    "$ADB" shell "mkdir -p $REMOTE_GLIBC"
    "$ADB" push "$PROJECT_ROOT/build/glibc.tar.gz" "$REMOTE_GLIBC/glibc.tar.gz"
    "$ADB" shell "cd $REMOTE_GLIBC && tar -xzf glibc.tar.gz && rm -f glibc.tar.gz && chmod 755 lib/ld-linux-aarch64.so.1"
    echo "    linker: $REMOTE_GLIBC/lib/ld-linux-aarch64.so.1"
else
    echo "    AVISO: sin runtime de glibc. Si el visor tiene Termux con"
    echo "    'pkg install glibc' el motor usara ese linker automaticamente."
fi

# 5. Mod puente
# El nombre de la carpeta DEBE coincidir con <nombre>_<version> de info.json
# o Factorio no carga el mod y el puente RCON no existe.
echo "[5/7] Instalando el mod puente $MOD_ID..."
"$ADB" shell "mkdir -p $REMOTE_FACTORIO/mods/$MOD_ID"
"$ADB" push "$LOCAL_MOD/control.lua" "$REMOTE_FACTORIO/mods/$MOD_ID/control.lua"
"$ADB" push "$LOCAL_MOD/info.json" "$REMOTE_FACTORIO/mods/$MOD_ID/info.json"
"$ADB" shell "chmod 644 $REMOTE_FACTORIO/mods/$MOD_ID/control.lua $REMOTE_FACTORIO/mods/$MOD_ID/info.json"

# 6. Assets reales del juego (atlas de sprites, iconos y textos).
# El motor VR dibuja los sprites que usa Factorio: sin el atlas se dibujan
# primitivas y la escena parece una maqueta.
LOCAL_ASSETS="$PROJECT_ROOT/build/factorio_assets"
if [ ! -f "$LOCAL_ASSETS/manifest.json" ]; then
    echo "    Generando el atlas de sprites desde la instalacion de Factorio..."
    FACTIO_DATA="${FACTORIO_DATA:-/Volumes/X10Pro/SteamLibrary/steamapps/common/Factorio/factorio.app/Contents/data}"
    # Solo los tipos del juego temprano, a media escala y solo la capa de
    # cuerpo: el atlas completo son 637 MB y no cabe en la GPU del visor.
    # Asi son 5 paginas de 2048x2048 (84 MB de VRAM).
    TYPES="assembling-machine-1,assembling-machine-2,assembling-machine-3,burner-mining-drill,electric-mining-drill,stone-furnace,steel-furnace,electric-furnace,transport-belt,fast-transport-belt,express-transport-belt,inserter,fast-inserter,small-electric-pole,medium-electric-pole,big-electric-pole,iron-chest,steel-chest,gun-turret,radar,roboport,splitter,underground-belt,substation,iron-ore,copper-ore,coal,uranium-ore,stone,crude-oil,tree"
    python3 "$PROJECT_ROOT/scripts/extract_factorio_assets.py" --data "$FACTORIO_DATA" \
        --out "$LOCAL_ASSETS" --only-listed --scale 0.5 --capas cuerpo --types "$TYPES" --quiet || true
fi
if [ -f "$LOCAL_ASSETS/manifest.json" ]; then
    echo "[6/7] Subiendo los assets reales del juego..."
    "$ADB" shell "mkdir -p $REMOTE_DATA/assets"
    "$ADB" push "$LOCAL_ASSETS/manifest.json" "$REMOTE_DATA/assets/manifest.json"
    # El atlas va paginado (atlas_0.rgba, atlas_1.rgba...): una textura de mas
    # de 4096 px de alto no se puede subir en la GPU del visor.
    for f in "$LOCAL_ASSETS"/atlas_*.rgba; do
        [ -f "$f" ] || continue
        "$ADB" push "$f" "$REMOTE_DATA/assets/$(basename "$f")"
    done
    for f in "$LOCAL_ASSETS"/atlas_*.png; do
        [ -f "$f" ] || continue
        "$ADB" push "$f" "$REMOTE_DATA/assets/$(basename "$f")"
    done
    "$ADB" shell "chmod 644 $REMOTE_DATA/assets/*" 2>/dev/null || true
else
    echo "[6/7] AVISO: sin atlas de sprites; se dibujaran primitivas."
fi

# 7. Script de arranque standalone
echo "[7/7] Subiendo el script de arranque standalone..."
"$ADB" push "$PROJECT_ROOT/scripts/start_factorio_standalone.sh" "$REMOTE_SCRIPT"
"$ADB" shell "chmod 755 $REMOTE_SCRIPT"

# 8. Arrancar el servidor Factorio
if [ "$START_SERVER" -eq 1 ]; then
    echo "Arrancando el servidor Factorio headless..."
    if "$ADB" shell "sh $REMOTE_SCRIPT" 2>&1 | tee /tmp/xfactorvr_standalone.log; then
        echo "Servidor Factorio en marcha."
    else
        echo "AVISO: el servidor no arranco. Revisa /tmp/xfactorvr_standalone.log"
        echo "       y el registro en el visor: /data/local/tmp/factorio_arm64/logs/standalone.log"
    fi
fi

# 8. Iniciar la aplicación
echo "Iniciando XFactorVR Tabletop en la Pico 4 Ultra..."
"$ADB" shell am start -n "$PACKAGE/android.app.NativeActivity"

echo "=== Despliegue completado con éxito ==="
echo "El visor ya tiene el motor real: la mesa OpenXR muestra Factorio 2.0 en vivo."
if [ "$STREAM" -eq 1 ]; then
    echo "Monitoreando logs en tiempo real (Ctrl+C para salir):"
    "$ADB" logcat -s XFactorVR_Launcher:V libxfactorvr:V
else
    echo "Capturando logs de inicio en Pico 4 Ultra:"
    sleep 2
    "$ADB" logcat -d -s XFactorVR_Launcher:V libxfactorvr:V
fi
