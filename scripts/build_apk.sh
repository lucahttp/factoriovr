#!/bin/bash
set -euo pipefail

# build_apk.sh — Compilación y empaquetado del APK Standalone para Pico 4 Ultra

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/build"
SDK_ROOT="/Volumes/X10Pro/Android/sdk"
NDK_ROOT="$SDK_ROOT/ndk/27.0.11718014"
BUILD_TOOLS="$SDK_ROOT/build-tools/34.0.0"
PLATFORM_JAR="$SDK_ROOT/platforms/android-34/android.jar"
TOOLCHAIN_BIN="$NDK_ROOT/toolchains/llvm/prebuilt/darwin-x86_64/bin"
CLANG_CXX="$TOOLCHAIN_BIN/aarch64-linux-android34-clang++"

echo "=== Compilando XFactorVR Tabletop para Pico 4 Ultra ==="

mkdir -p "$BUILD_DIR/apk/lib/arm64-v8a"
mkdir -p "$BUILD_DIR/compiled_res"

# 1. Compilar la biblioteca de Rust para ARM64 Android
echo "[1/4] Verificando target aarch64-linux-android en Rust..."
if ! rustup target list --installed | grep -q "aarch64-linux-android"; then
    rustup target add aarch64-linux-android
fi

echo "[1/4] Compilando libxfactorvr.so con Cargo..."
cargo build --release --target aarch64-linux-android

if [ -f "$PROJECT_ROOT/target/aarch64-linux-android/release/libxfactorvr.so" ]; then
    cp "$PROJECT_ROOT/target/aarch64-linux-android/release/libxfactorvr.so" "$BUILD_DIR/apk/lib/arm64-v8a/"
fi

# 2. Compilar el launcher nativo (NativeActivity) y el motor OpenXR con NDK Clang++
echo "[2/4] Compilando launcher nativo C++ y motor OpenXR..."
cp "$PROJECT_ROOT/android/lib/arm64-v8a/libopenxr_loader.so" "$BUILD_DIR/apk/lib/arm64-v8a/"

LIBCXX_SO="$NDK_ROOT/android-ndk-r27-beta1/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
if [ -f "$LIBCXX_SO" ]; then
    cp "$LIBCXX_SO" "$BUILD_DIR/apk/lib/arm64-v8a/"
fi

"$CLANG_CXX" -shared -fPIC -O3 -Wall \
    -I"$PROJECT_ROOT/android/include" \
    -DVK_USE_PLATFORM_ANDROID_KHR \
    -DXR_USE_PLATFORM_ANDROID=1 \
    -DXR_USE_GRAPHICS_API_OPENGL_ES=1 \
    "$PROJECT_ROOT/android/src/launcher.cpp" \
    "$PROJECT_ROOT/android/src/openxr_engine.cpp" \
    -L"$PROJECT_ROOT/android/lib/arm64-v8a" \
    -L"$BUILD_DIR/apk/lib/arm64-v8a" \
    -lopenxr_loader -lxfactorvr -lEGL -lGLESv3 \
    -static-libstdc++ \
    -llog -landroid \
    -o "$BUILD_DIR/apk/lib/arm64-v8a/libxfactorvr_launcher.so"

# 3. Empaquetar recursos con aapt2
echo "[3/4] Generando APK con aapt2..."
"$BUILD_TOOLS/aapt2" link \
    -I "$PLATFORM_JAR" \
    --manifest "$PROJECT_ROOT/android/AndroidManifest.xml" \
    --min-sdk-version 29 \
    --target-sdk-version 34 \
    --version-code 1 \
    --version-name "1.0.0" \
    -o "$BUILD_DIR/unaligned.apk"

# Agregar librerías nativas al paquete
cd "$BUILD_DIR/apk"
zip -u -r "$BUILD_DIR/unaligned.apk" lib/
cd "$PROJECT_ROOT"

# 4. Alinear y firmar con debug keystore
echo "[4/4] Alineando y firmando APK..."
"$BUILD_TOOLS/zipalign" -f 4 "$BUILD_DIR/unaligned.apk" "$BUILD_DIR/XFactorVR_Pico4Ultra.apk"

KEYSTORE="$BUILD_DIR/debug.keystore"
if [ ! -f "$KEYSTORE" ]; then
    keytool -genkeypair -v -keystore "$KEYSTORE" -alias androiddebugkey \
        -keyalg RSA -keysize 2048 -validity 10000 \
        -storepass android -keypass android \
        -dname "CN=Android Debug,O=Android,C=US"
fi

"$BUILD_TOOLS/apksigner" sign \
    --ks "$KEYSTORE" \
    --ks-pass pass:android \
    --key-pass pass:android \
    "$BUILD_DIR/XFactorVR_Pico4Ultra.apk"

echo "=== APK listo para Pico 4 Ultra: $BUILD_DIR/XFactorVR_Pico4Ultra.apk ==="
