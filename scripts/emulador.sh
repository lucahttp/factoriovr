#!/usr/bin/env bash
set -uo pipefail

# emulador.sh — Emulador Android ARM64 (ATD) en el disco externo.
#
# Para que el motor de Factorio ARM64 se pueda probar en un Android de
# verdad (con shell, con SELinux, con /data/local/tmp) SIN depender del USB
# de la Pico 4 Ultra, que se suelta cada dos por tres.
#
# El SDK esta en el disco externo: /Volumes/X10Pro/Android/sdk
# (ahi esta el NDK que se usa para compilar el APK).
#
# Uso:
#   bash scripts/emulador.sh iniciar     # arranca el emulador y espera al boot
#   bash scripts/emulador.sh parar
#   bash scripts/emulador.sh estado
#   bash scripts/emulador.sh shell "comando"

SDK="${ANDROID_SDK_ROOT:-/Volumes/X10Pro/Android/sdk}"
AVD="${AVD:-xfv}"
EMU="$SDK/emulator/emulator"
SERIAL="${SERIAL:-emulator-5554}"
# El adb del SDK de disco externo.  OJO: antes esta variable NO estaba
# definida y se usaba "$ADB" en `iniciar` y `estado`: con "set -u" eso es
# un error de variable no definida y el script se caia al arrancar.
ADB="${ADB:-$SDK/platform-tools/adb}"

export ANDROID_SDK_ROOT="$SDK"
export ANDROID_HOME="$SDK"
export PATH="$SDK/emulator:$SDK/platform-tools:$SDK/cmdline-tools/latest/bin:$PATH"

case "${1:-iniciar}" in
    instalar_imagen)
        # Instala la imagen del sistema desde un zip YA DESCARGADO, sin volver
        # a pasar por sdkmanager (que la volveria a bajar entera).
        #
        # Por que esto existe: en esta maquina dl.google.com entrega ~250-400
        # KB/s y la imagen son 533 MB.  sdkmanager la baja a pieces pero
        # BORRA la descarga a medias si se corta, asi que se baja a mano (con
        # varias conexiones en paralelo) y se instala desde aqui.
        ZIP="${ZIP:-$SDK/.temp/descarga/arm64-v8a-33_r02.zip}"
        DEST="$SDK/system-images/android-33"
        [ -f "$ZIP" ] || { echo "no esta el zip $ZIP" >&2; exit 1; }
        echo "instalando la imagen desde $ZIP..."
        mkdir -p "$DEST"
        ( cd "$DEST" && unzip -q -o "$ZIP" )
        # El zip de la imagen de ATD trae "arm64-v8a/" en la raiz (sin la
        # carpeta "aosp_atd"), pero el emulador la busca en
        # system-images/android-33/aosp_atd/arm64-v8a/.
        #
        # OJO: la carpeta destino puede YA existir vacia (la deja el
        # sdkmanager cuando se corta a medias), asi que no vale con "si no
        # existe, mover": hay que VACIARLA y meter el contenido dentro.
        if [ -d "$DEST/arm64-v8a" ]; then
            mkdir -p "$DEST/aosp_atd/arm64-v8a"
            for f in "$DEST/arm64-v8a"/*; do
                [ -e "$f" ] || continue
                mv "$f" "$DEST/aosp_atd/arm64-v8a/"
            done
            rmdir "$DEST/arm64-v8a" 2>/dev/null || true
        fi
        if [ ! -f "$DEST/aosp_atd/arm64-v8a/system.img" ]; then
            echo "la imagen no ha quedado bien en $DEST/aosp_atd/arm64-v8a:" >&2
            ls -la "$DEST/aosp_atd/arm64-v8a" >&2
            exit 1
        fi
        echo "imagen instalada en $DEST/aosp_atd/arm64-v8a"
        ls "$DEST/aosp_atd/arm64-v8a" | head
        ;;
    crear_avd)
        # Crea el AVD a mano, sin avdmanager: avdmanager exige que el paquete
        # este REGISTRADO en el SDK (package.xml), y una imagen instalada a
        # mano no lo esta.  El emulador solo necesita config.ini con
        # image.sysdir.1 apuntando a la imagen.
        AVDS_DIR="$HOME/.android/avd"
        mkdir -p "$AVDS_DIR/$AVD.avd"
        cat > "$AVDS_DIR/$AVD.ini" <<INI
avd.ini.encoding=UTF-8
path=$AVDS_DIR/$AVD.avd
path.rel=avd/$AVD.avd
target=android-33
INI
        cat > "$AVDS_DIR/$AVD.avd/config.ini" <<CFG
avd.ini.encoding=UTF-8
AvdId=$AVD
abi.type=arm64-v8a
avd.ini.displayname=$AVD
disk.dataPartition.size=6G
fastboot.forceColdBoot=yes
hw.accelerometer=yes
hw.audioInput=no
hw.battery=yes
hw.camera.back=none
hw.camera.front=none
hw.cpu.arch=arm64
hw.cpu.ncore=4
hw.dPad=no
hw.device.manufacturer=Google
hw.device.name=pixel_6
hw.gps=no
hw.gpu.enabled=yes
hw.gpu.mode=swiftshader
hw.keyboard=yes
hw.lcd.density=420
hw.lcd.height=2400
hw.lcd.width=1080
hw.mainKeys=no
hw.ramSize=4096
hw.sdCard=no
hw.sensors.orientation=no
image.sysdir.1=system-images/android-33/aosp_atd/arm64-v8a/
tag.display=Android Automated Test Device
tag.id=aosp_atd
vm.heapSize=256
CFG
        echo "AVD $AVD creado en $AVDS_DIR/$AVD.avd"
        "$EMU" -list-avds 2>/dev/null | sed 's/^/  AVD: /'
        ;;
    instalar)
        # 1. El emulador y la imagen ATD (la mas ligera: sin Google Play).
        if [ ! -x "$EMU" ] || [ ! -d "$SDK/system-images/android-33/aosp_atd/arm64-v8a" ]; then
            echo "instalando emulador e imagen ATD en $SDK (tarda unos minutos)..."
            yes | sdkmanager --install "emulator" \
                "system-images;android-33;aosp_atd;arm64-v8a" || exit 1
        fi
        # 2. El AVD.
        if ! "$SDK/emulator/emulator" -list-avds 2>/dev/null | grep -qx "$AVD"; then
            echo "creando el AVD $AVD..."
            echo no | avdmanager create avd -n "$AVD" \
                -k "system-images;android-33;aosp_atd;arm64-v8a" \
                --device "pixel_6" --force || exit 1
        fi
        # 3. Arrancar.
        exec "$0" iniciar
        ;;
    iniciar)
        if "$ADB" devices 2>/dev/null | grep -q "^$SERIAL"; then
            echo "el emulador ya esta en marcha"
            exit 0
        fi
        # -no-window: sin ventana (headless).  -gpu: el render por software
        # (el Mac no virtualiza la GPU del Adreno).
        #
        # OJO: "swiftshader_indirect" YA NO EXISTE en el emulador 37.2.12 (este
        # SDK): los modos validos son auto, host, software, lavapipe,
        # swiftshader y swangle.  Con el nombre viejo el emulador se niega a
        # arrancar.  Se usa "swiftshader", que es el camino de siempre para
        # headless.
        # -no-snapshot: arranque limpio, siempre igual.
        nohup "$EMU" -avd "$AVD" -no-window -no-audio -no-boot-anim \
            -gpu swiftshader -no-snapshot -memory 4096 \
            -wipe-data > /tmp/emulador.log 2>&1 &
        echo "emulador arrancando (log en /tmp/emulador.log)..."
        for i in $(seq 1 180); do
            if "$ADB" shell getprop sys.boot_completed 2>/dev/null | grep -q "1"; then
                echo "emulador listo ($SERIAL)"
                "$ADB" shell getprop ro.build.version.release 2>/dev/null | head -1
                exit 0
            fi
            sleep 2
        done
        echo "el emulador no arranco (mira /tmp/emulador.log)"
        exit 1
        ;;
    parar)
        "$ADB" -s "$SERIAL" emu kill 2>/dev/null || true
        pkill -f "emulator.*-avd $AVD" 2>/dev/null || true
        echo "emulador parado"
        ;;
    estado)
        "$ADB" devices -l
        "$ADB" -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null
        ;;
    shell)
        shift
        "$ADB" -s "$SERIAL" shell "$@"
        ;;
    *)
        echo "uso: $0 {iniciar|parar|estado|shell ...}"
        exit 2
        ;;
esac
