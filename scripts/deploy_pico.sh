#!/bin/bash
set -euo pipefail

# deploy_pico.sh — Despliegue de XFactorVR en Pico 4 Ultra vía ADB

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADB="/Volumes/X10Pro/Android/sdk/platform-tools/adb"
APK="$PROJECT_ROOT/build/XFactorVR_Pico4Ultra.apk"
PACKAGE="com.xfactorvr.tabletop"
REMOTE_DATA="/sdcard/XFactorVR"

echo "=== Desplegando XFactorVR en Pico 4 Ultra ==="

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

echo "[1/3] Instalando $APK..."
"$ADB" install -r "$APK"
"$ADB" shell appops set "$PACKAGE" MANAGE_EXTERNAL_STORAGE allow 2>/dev/null || true

# 3. Transferir assets de Factorio y modelos 3D al almacenamiento del visor
echo "[2/3] Sincronizando datos de Factorio a la Pico 4 Ultra ($REMOTE_DATA)..."
"$ADB" shell "mkdir -p $REMOTE_DATA"

GAME_DATA="/Volumes/X10Pro/SteamLibrary/steamapps/common/Factorio_Linux/data"
if [ -d "$GAME_DATA" ]; then
    echo "Subiendo data/base y data/core..."
    "$ADB" push "$GAME_DATA/base" "$REMOTE_DATA/"
    "$ADB" push "$GAME_DATA/core" "$REMOTE_DATA/"
fi

MODELS_DIR="$PROJECT_ROOT/factorio-3d-mod/models"
if [ ! -d "$MODELS_DIR" ]; then
    echo "Descargando modelos 3D de factorio-3d-mod..."
    git clone https://github.com/GDani31/factorio-3d-mod.git "$PROJECT_ROOT/factorio-3d-mod"
fi
if [ -d "$MODELS_DIR" ]; then
    echo "Subiendo modelos 3D GLTF de máquinas..."
    "$ADB" push "$MODELS_DIR" "$REMOTE_DATA/"
fi

# 4. Iniciar la aplicación
echo "[3/3] Iniciando XFactorVR Tabletop en la Pico 4 Ultra..."
"$ADB" shell am start -n "$PACKAGE/android.app.NativeActivity"

echo "=== Despliegue completado con éxito ==="
if [ "${1:-}" = "--stream" ]; then
    echo "Monitoreando logs en tiempo real (Ctrl+C para salir):"
    "$ADB" logcat -s XFactorVR_Launcher:V libxfactorvr:V
else
    echo "Capturando logs de inicio en Pico 4 Ultra:"
    sleep 2
    "$ADB" logcat -d -s XFactorVR_Launcher:V libxfactorvr:V
fi
