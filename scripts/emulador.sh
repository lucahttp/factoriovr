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

export ANDROID_SDK_ROOT="$SDK"
export ANDROID_HOME="$SDK"
export PATH="$SDK/emulator:$SDK/platform-tools:$SDK/cmdline-tools/latest/bin:$PATH"

case "${1:-iniciar}" in
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
        # -no-window: sin ventana (headless).  -gpu swiftshader_indirect: el
        # render por software (el Mac no virtualiza la GPU del Adreno).
        # -no-snapshot: arranque limpio, siempre igual.
        nohup "$EMU" -avd "$AVD" -no-window -no-audio -no-boot-anim \
            -gpu swiftshader_indirect -no-snapshot -memory 4096 \
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
