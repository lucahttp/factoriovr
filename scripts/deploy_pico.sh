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
REMOTE_CLIENT_SCRIPT="/data/local/tmp/start_factorio_client.sh"

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
MODO_CLIENT=0
MODO_CAMPANA=0
# Cliente grafico LIGERO (binario del cliente + datos del headless, sin
# audio): 280 MB en vez de 2 GB.  Con --completo se sube el paquete entero.
CLIENTE_COMPLETO=0
for arg in "$@"; do
    case "$arg" in
        --stream)   STREAM=1 ;;
        --no-start) START_SERVER=0 ;;
        --strip)    STRIP=1 ;;
        --client)   MODO_CLIENT=1 ;;
        --campana)  MODO_CLIENT=1; MODO_CAMPANA=1 ;;
        --completo) CLIENTE_COMPLETO=1 ;;
    esac
done

# El cliente GRAFICO (con Zink) es lo que permite jugar la campana: el
# headless no tiene jugador. Necesita el payload de Mesa y el inyector.
REMOTE_CLIENT="/data/local/tmp/factorio_client"
REMOTE_MESA="/data/local/tmp/mesa"
REMOTE_INJECTOR="/data/local/tmp/libxfactorvr_injector.so"
LOCAL_CLIENT="$PROJECT_ROOT/factorio_client_arm64/factorio"
LOCAL_MESA="$PROJECT_ROOT/build/mesa"
LOCAL_INJECTOR="$PROJECT_ROOT/injector-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so"

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

# 3 bis. Modo CAMPANA: dejar el nivel listo para multijugador ANTES de
# empaquetar el motor, para que el escenario viaje dentro del tar y llegue al
# servidor (el nivel de campana es single-player: usa game.players[1] en su
# init, asi que hay que esperar al jugador y usar el helper
# story_show_message_dialog, que es el unico que funciona en MP.
# Ver docs/ADR-005-campana-headless-mp.md.
if [ "$MODO_CAMPANA" -eq 1 ]; then
    NIVEL="$LOCAL_FACTORIO/data/base/campaigns/tutorial/level-01"
    # El motor carga los escenarios de data/<mod>/scenarios/<nombre>, asi que
    # el nivel tiene que quedar en base/scenarios/xfv-nivel (no dentro de
    # campaigns/, que es donde el nivel original vive).
    ESC_DIR="$LOCAL_FACTORIO/data/base/scenarios/xfv-nivel"
    if [ -f "$NIVEL/control.lua" ]; then
        echo "    Preparando el nivel de campana para multijugador..."
        mkdir -p "$(dirname "$ESC_DIR")"
        rm -rf "$ESC_DIR"
        cp -R "$NIVEL" "$ESC_DIR"
        python3 "$PROJECT_ROOT/scripts/preparar_campana_mp.py" "$ESC_DIR" 2>&1 | tail -3 || true
    else
        echo "    AVISO: no se encuentra el nivel de campana en $NIVEL"
    fi
fi

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

# 4b. Modo CLIENTE GRAFICO: el motor con graficos + Mesa/Zink + inyector.
#
# Es lo que hace falta para JUGAR LA CAMPANA: el headless no tiene jugador y
# la campana lo exige (game.players[1] en on_init). Ademas el cliente
# grafico es el que se puede enganchar con el inyector del mod 3D.
if [ "$MODO_CLIENT" -eq 1 ]; then
    echo "[4b/7] Desplegando el cliente GRAFICO con Mesa+Zink y el inyector..."

    if [ ! -d "$LOCAL_CLIENT" ]; then
        echo "    falta el cliente grafico en $LOCAL_CLIENT"
        echo "    descomprime factorio_linux-arm64_2.1.22.tar.xz ahi."
        exit 1
    fi
    if [ ! -f "$LOCAL_MESA/usr/lib/aarch64-linux-gnu/dri/zink_dri.so" ]; then
        echo "    generando el payload de Mesa con Zink..."
        python3 "$PROJECT_ROOT/scripts/prepare_mesa.py" >/dev/null 2>&1 || true
    fi
    if [ ! -f "$LOCAL_INJECTOR" ]; then
        echo "    compilando el inyector para aarch64..."
        cargo build --release --manifest-path "$PROJECT_ROOT/injector-linux/Cargo.toml" \
            --target aarch64-unknown-linux-gnu >/dev/null 2>&1 || true
    fi

    CLIENT_TAR="$PROJECT_ROOT/build/factorio_client.tar"
    MESA_TAR="$PROJECT_ROOT/build/mesa.tar.gz"

    # Cliente LIGERO (por defecto): el binario del cliente grafico (250 MB) con
    # los datos del motor headless (82 MB) en vez de los 2 GB del paquete
    # completo de Steam.
    #
    # Por que funciona: al motor grafico solo le falta el directorio de
    # sonidos (1,2 GB) y las expansiones.  Los datos del headless traen la
    # campana de la base, que es la que se juega; las expansiones (space-age,
    # quality) se quitan porque el motor grafico no las trae y mezclarlas
    # revienta al cargar los datos.  Validado en el contenedor ARM64.
    #
    # Con --completo se sube el paquete entero (2 GB), que ademas trae el
    # audio del juego.
    CLIENT_STAGE="$PROJECT_ROOT/build/factorio_stage_ligero/factorio"
    if [ "$CLIENTE_COMPLETO" -eq 1 ]; then
        echo "    empaquetando el cliente grafico COMPLETO (2 GB)..."
        COPYFILE_DISABLE=1 tar -cf "$CLIENT_TAR" -C "$(dirname "$LOCAL_CLIENT")" "$(basename "$LOCAL_CLIENT")"
    else
        echo "    empaquetando el cliente grafico LIGERO (sin audio)..."
        mkdir -p "$CLIENT_STAGE"
        cp -R "$LOCAL_CLIENT/bin" "$CLIENT_STAGE/" 2>/dev/null || true
        mkdir -p "$CLIENT_STAGE/data"
        cp -R "$LOCAL_FACTORIO/data/." "$CLIENT_STAGE/data/" 2>/dev/null || true
        # Las expansiones sobran (y chocan): el cliente grafico no las trae.
        for extra in space-age quality elevated-rails recycler; do
            rm -rf "$CLIENT_STAGE/data/$extra"
        done
        # El motor grafico espera base/sound: se crea vacio (sin audio).
        mkdir -p "$CLIENT_STAGE/data/base/sound"
        COPYFILE_DISABLE=1 tar -cf "$CLIENT_TAR" -C "$(dirname "$CLIENT_STAGE")" "$(basename "$CLIENT_STAGE")"
    fi

    # El tar del payload de Mesa lo hace prepare_mesa.py con Python y
    # dereference=True: nunca guarda enlaces duros.  Con el tar del sistema
    # (bsdtar en macOS) no se puede pedir eso, y si el tar llevase enlaces
    # duros al extraer en Android el link() falla con "Permission denied"
    # (SELinux no deja crear enlaces duros en shell_data_file) y el cliente
    # grafico se queda sin Zink.
    if [ ! -f "$MESA_TAR" ] || [ "$LOCAL_MESA/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0" -nt "$MESA_TAR" ]; then
        echo "    empaquetando el payload de Mesa..."
        python3 "$PROJECT_ROOT/scripts/prepare_mesa.py" --solo-tar >/dev/null 2>&1 || true
    fi
    if [ ! -f "$MESA_TAR" ]; then
        echo "    ERROR: no se pudo empaquetar el payload de Mesa."
        exit 1
    fi

    echo "    cliente: $(du -h "$CLIENT_TAR" | cut -f1)  mesa: $(du -h "$MESA_TAR" | cut -f1)"
    "$ADB" shell "mkdir -p $REMOTE_CLIENT $REMOTE_MESA"
    "$ADB" push "$CLIENT_TAR" "$REMOTE_CLIENT/client.tar"
    # El tar del cliente contiene la carpeta "factorio/" (o el nombre del
    # stage): se extrae en /data/local/tmp para que quede en
    # /data/local/tmp/factorio, que es la ruta que espera el script de
    # arranque del cliente.
    "$ADB" shell "cd /data/local/tmp && rm -rf factorio && tar -xf factorio_client/client.tar && rm -f factorio_client/client.tar && chmod 755 factorio/bin/arm64/factorio && mkdir -p factorio/mods factorio/config factorio/saves"
    "$ADB" push "$MESA_TAR" "$REMOTE_MESA/mesa.tar.gz"
    # El tar contiene la carpeta "mesa/", asi que se extrae en /data/local/tmp
    # para que quede en /data/local/tmp/mesa, que es lo que espera el script
    # del cliente ($MESA_DIR/lib/ld-linux-aarch64.so.1).
    "$ADB" shell "cd /data/local/tmp && tar -xzf mesa/mesa.tar.gz && rm -f mesa/mesa.tar.gz && chmod 755 mesa/lib/ld-linux-aarch64.so.1"
    # Se comprueba que Zink llego de verdad: si no, el cliente arranca sin
    # aceleracion (o no arranca) y el fallo se ve mucho mas tarde.
    if ! "$ADB" shell "test -f $REMOTE_MESA/mesa/usr/lib/aarch64-linux-gnu/dri/zink_dri.so"; then
        echo "    ERROR: el payload de Mesa no llego completo (falta zink_dri.so)."
        echo "           Revisa que el tar se haya extraido sin errores."
        exit 1
    fi
    echo "    Zink presente en el visor."
    if [ -f "$LOCAL_INJECTOR" ]; then
        "$ADB" push "$LOCAL_INJECTOR" "$REMOTE_INJECTOR"
        "$ADB" shell "chmod 755 $REMOTE_INJECTOR"
        echo "    inyector: $REMOTE_INJECTOR"
    else
        echo "    AVISO: sin inyector compilado."
    fi
    "$ADB" push "$PROJECT_ROOT/scripts/start_factorio_client.sh" "$REMOTE_SCRIPT"
    "$ADB" shell "chmod 755 $REMOTE_SCRIPT"
fi

# 5. Mod puente
# El nombre de la carpeta DEBE coincidir con <nombre>_<version> de info.json
# o Factorio no carga el mod y el puente RCON no existe.
echo "[5/7] Instalando el mod puente $MOD_ID..."
# En modo cliente el mod va al directorio del cliente grafico; en modo
# headless, al del motor headless.
if [ "$MODO_CLIENT" -eq 1 ]; then
    MODS_DIR="$REMOTE_CLIENT/factorio/mods"
else
    MODS_DIR="$REMOTE_FACTORIO/mods"
fi
"$ADB" shell "mkdir -p $MODS_DIR/$MOD_ID"
"$ADB" push "$LOCAL_MOD/control.lua" "$MODS_DIR/$MOD_ID/control.lua"
"$ADB" push "$LOCAL_MOD/info.json" "$MODS_DIR/$MOD_ID/info.json"
"$ADB" shell "chmod 644 $MODS_DIR/$MOD_ID/control.lua $MODS_DIR/$MOD_ID/info.json"

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
if [ "$MODO_CLIENT" -eq 1 ]; then
    "$ADB" push "$PROJECT_ROOT/scripts/start_factorio_client.sh" "$REMOTE_CLIENT_SCRIPT"
    "$ADB" shell "chmod 755 $REMOTE_CLIENT_SCRIPT"
fi

# 7b. Modo campana: el nivel ya se preparó antes de empaquetar el motor (paso
# 3 bis), asi que el escenario xfv-nivel viaja dentro del tar y llega al
# servidor en data/base/scenarios/xfv-nivel.
if [ "$MODO_CAMPANA" -eq 1 ]; then
    if "$ADB" shell "test -d $REMOTE_FACTORIO/data/base/scenarios/xfv-nivel"; then
        echo "    Escenario de campana en el servidor: base/xfv-nivel"
    else
        echo "    AVISO: el escenario de campana no llego al servidor."
    fi
fi

# 8. Arrancar el servidor Factorio
if [ "$START_SERVER" -eq 1 ]; then
    if [ "$MODO_CAMPANA" -eq 1 ]; then
        echo "Arrancando el servidor con el escenario de campana..."
        "$ADB" shell "SCENARIO=base/xfv-nivel sh $REMOTE_SCRIPT" 2>&1 | tee /tmp/xfactorvr_campana.log || true
    else
        echo "Arrancando el servidor Factorio headless..."
        if "$ADB" shell "sh $REMOTE_SCRIPT" 2>&1 | tee /tmp/xfactorvr_standalone.log; then
            echo "Servidor Factorio en marcha."
        else
            echo "AVISO: el servidor no arranco. Revisa /tmp/xfactorvr_standalone.log"
            echo "       y el registro en el visor: /data/local/tmp/factorio_arm64/logs/standalone.log"
        fi
    fi
fi

# 8b. Arrancar el cliente grafico (modo campana): se conecta al servidor y
# aporta el JUGADOR que el nivel de campana exige.
if [ "$MODO_CAMPANA" -eq 1 ] && [ "$MODO_CLIENT" -eq 1 ]; then
    echo "Arrancando el cliente grafico (conecta al servidor, aporta el jugador)..."
    "$ADB" shell "SERVER_ADDR=127.0.0.1 GAME_DIR=$REMOTE_CLIENT/factorio sh $REMOTE_CLIENT_SCRIPT" \
        >/dev/null 2>&1 &
    sleep 3
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
