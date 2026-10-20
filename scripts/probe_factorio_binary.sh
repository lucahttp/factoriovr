#!/bin/bash
# probe_factorio_binary.sh — Sondeo estático del binario ARM64 de Factorio
#
# Extrae, sin ejecutar nada, los hechos que necesita el ADR-001 para decidir
# cómo meter el cliente GRÁFICO de Factorio dentro de una app Android:
#
#   1. Dependencias dinámicas reales (DT_NEEDED) y símbolos importados.
#   2. Qué subsistema de vídeo usa (SDL2 / SDL3, X11, Wayland, EGL...).
#   3. Qué versión de OpenGL exige (loader GLAD) y qué shaders usa.
#   4. Puntos de enganche de render con su DIRECCIÓN real en el ELF
#      (sirven para Frida / LD_PRELOAD sobre Linux ARM64).
#
# NO modifica nada. Solo lee. Si el binario no existe avisa y sale con 0.
#
# Uso:
#   scripts/probe_factorio_binary.sh [ruta/al/binario]
#   FACTORIO_BIN=/ruta/al/binario scripts/probe_factorio_binary.sh

set -uo pipefail   # sin -e: el script debe terminar con 0 aunque falte algo

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-${FACTORIO_BIN:-$PROJECT_ROOT/factorio_arm64/bin/arm64/factorio}}"

hr() { printf '%s\n' "------------------------------------------------------------------"; }
sec() { hr; printf '  %s\n' "$1"; hr; }

echo "=== Sondeo del binario de Factorio (ARM64) ==="
echo "Binario: $BIN"

if [ ! -f "$BIN" ]; then
    echo
    echo "AVISO: el binario no existe en esa ruta."
    echo "       Descarga el headless ARM64 publico con:"
    echo "         curl -L -o factorio-headless_arm64.tar.xz \\"
    echo "           https://factorio.com/get-download/experimental/headless/linux-arm64"
    echo "       y descomprimelo en factorio_arm64/."
    echo "       Continuando con la parte que no necesita el binario."
    hr
    exit 0
fi

command -v objdump >/dev/null 2>&1 || { echo "AVISO: objdump no disponible."; }
command -v nm      >/dev/null 2>&1 || { echo "AVISO: nm no disponible."; }
command -v strings >/dev/null 2>&1 || { echo "AVISO: strings no disponible."; }

SIZE_H=$(du -h "$BIN" 2>/dev/null | cut -f1)
echo "Tamano:   $SIZE_H"
echo "SHA-256:  $(shasum -a 256 "$BIN" 2>/dev/null | cut -d' ' -f1)"
echo "Formato:  $(objdump -f "$BIN" 2>/dev/null | grep -m1 'file format' | sed 's/.*file format //')"

# ---------------------------------------------------------------------------
sec "1. DEPENDENCIAS DINAMICAS (DT_NEEDED)"
# El binario headless solo enlaza con glibc: SDL2 va ESTATICO dentro.
# ---------------------------------------------------------------------------
if command -v objdump >/dev/null 2>&1; then
    objdump -p "$BIN" 2>/dev/null | grep -E "NEEDED|SONAME|RPATH|RUNPATH" \
        | sed 's/^/  /'
    echo
    echo "  -- Versiones de glibc exigidas --"
    objdump -p "$BIN" 2>/dev/null | grep -E "GLIBC_2\.[0-9]+" \
        | grep -oE "GLIBC_2\.[0-9]+" | sort -u -V | tr '\n' ' ' | sed 's/^/  /'
    echo; echo
    echo "  -- Simbolos importados dinamicamente (muestra) --"
    if command -v nm >/dev/null 2>&1; then
        nm -D --undefined-only "$BIN" 2>/dev/null | head -15 | sed 's/^/  /'
        echo "  ... total importados dinamicos: $(nm -D --undefined-only "$BIN" 2>/dev/null | wc -l | tr -d ' ')"
    fi
fi

# ---------------------------------------------------------------------------
sec "2. SDL2 / SDL3 Y SUBSISTEMA DE VIDEO"
# ---------------------------------------------------------------------------
if command -v nm >/dev/null 2>&1; then
    SDL_COUNT=$(nm -C --defined-only "$BIN" 2>/dev/null | grep -cE "[TtW] SDL_")
    echo "  Simbolos SDL definidos en el binario: $SDL_COUNT"
    echo
    echo "  -- Version de SDL: se deduce del patron de nombres --"
    echo "  SDL2 usa el patron dynamic-API  <fn>_REAL / <fn>_DEFAULT"
    echo "  SDL3 elimino _REAL y SDL_GL_* a cambio de SDL_ClaimWindowForSurface."
    if nm -C --defined-only "$BIN" 2>/dev/null | grep -cE "[TtW] SDL_[A-Za-z0-9]+_REAL$" > /dev/null 2>&1; then
        echo "  -> SDL2 CONFIRMADO (patron _REAL/_DEFAULT presente)"
    else
        echo "  -> Sin patron _REAL/_DEFAULT: revisar a mano"
    fi
    echo
    echo "  -- Drivers de video SDL presentes (nombres exactos) --"
    if command -v strings >/dev/null 2>&1; then
        strings -a "$BIN" 2>/dev/null \
            | grep -xE "x11|X11|wayland|offscreen|dummy|kmsdrm|android|directfb|vivante|risc-v" \
            | sort -u | sed 's/^/  /' || echo "  (ninguno)"
    fi
    echo
    echo "  -- Librerias que SDL2 carga con dlopen en runtime --"
    if command -v strings >/dev/null 2>&1; then
        strings -a "$BIN" 2>/dev/null \
            | grep -xE "lib(X11|X11-xcb|wayland-client|wayland-egl|wayland-cursor|EGL|GL|GLESv2|GLESv1_CM|GLES_CM|drm|udev|asound|pulse)\.so[0-9.]*" \
            | sort -u | sed 's/^/  /' || echo "  (ninguna)"
    fi
    echo
    echo "  -- Puntos de entrada SDL de video/GL (con direccion) --"
    nm -C --defined-only "$BIN" 2>/dev/null \
        | grep -E "[TtW] SDL_(CreateWindow|GL_CreateContext|GL_GetProcAddress|GL_MakeCurrent|GL_SwapWindow|GL_SetAttribute|VideoInit|GetNumVideoDrivers)_REAL$" \
        | sed 's/^/  /' || echo "  (ninguno)"
fi

# ---------------------------------------------------------------------------
sec "3. VERSION DE OPENGL EXIGIDA"
# ---------------------------------------------------------------------------
if command -v nm >/dev/null 2>&1; then
    echo "  -- Banderas de version del loader GLAD --"
    nm -C --defined-only "$BIN" 2>/dev/null | grep -E "GLAD_GL_VERSION" | sort -k3 \
        | sed 's/^/  /'
    echo
    echo "  -> La version maxima que declara el loader es el MINIMO que pide el juego."
    echo
    echo "  -- Carga de funciones GL --"
    nm -C --defined-only "$BIN" 2>/dev/null | grep -E "[TtW] gladLoadGL" | sed 's/^/  /'
fi
if command -v strings >/dev/null 2>&1; then
    echo
    echo "  -- Version de los shaders GLSL embebidos --"
    strings -a "$BIN" 2>/dev/null | grep -E "^#version [0-9]+" | sort -u | sed 's/^/  /'
    echo
    echo "  -- Mensajes de error sobre OpenGL --"
    strings -a "$BIN" 2>/dev/null \
        | grep -iE "OpenGL [0-9]|not supported by this system|requires OpenGL" \
        | sort -u | head -8 | sed 's/^/  /'
fi

# ---------------------------------------------------------------------------
sec "4. PUNTOS DE ENGANCHE DE RENDER (direccion real en el ELF)"
# Estos son los simbolos que hookearia factorio-3d-mod con Frida/LD_PRELOAD.
# Se resuelven POR NOMBRE (el ELF no esta stripped), no por offset.
# ---------------------------------------------------------------------------
if command -v nm >/dev/null 2>&1; then
    PROBE_SYMS=(
        "GameRenderer::render"
        "GameRenderer::prepare"
        "GameRenderer::postPrepare"
        "GameView::getMapPosition"
        "GameView::prepareRender"
        "RenderParameters::centerOn"
        "DrawEngine::render"
        "DrawEngine::drawEntities"
        "DrawEngine::drawPrimitives"
        "DrawQueue::drawScaledTinted"
        "DrawQueue::drawScaled"
        "ItemStackPainter::drawItemStack"
        "TransportBelt::drawItems"
        "WorkingVisualisations::drawMainAnimation"
        "Entity::~Entity"
        "Entity::changePosition"
        "Entity::setupPosition"
        "Entity::onChangePosition"
        "CameraBase::CameraBase"
        "CameraBase::paint"
        "GraphicsInterfaceOpenGL::initialize"
        "GraphicsInterfaceOpenGL::swapBuffers"
        "GraphicsInterfaceOpenGL::drawIndexed"
        "GraphicsInterfaceOpenGL::bindFramebuffer"
        "GraphicsInterfaceOpenGL::createFramebuffer"
        "GraphicsInterfaceOpenGL::createVideoBitmap"
        "FramebufferOpenGL::generateFramebufferObject"
        "FramebufferOpenGL::blitTo"
        "FramebufferOpenGL::copyTo"
        "VideoBitmapOpenGL::updateMipmapPixels"
        "EngineFramebuffers::initializeCommon"
        "GlobalContext::swapRenderBuffers"
        "GlobalContext::loadSprites"
        "GlobalContext::headlessMode"
        "SDLWindow::SDLWindow"
        "SDLWindow::swap"
        "SDLWindow::presentEngineMainFramebuffer"
        "HeadlessGraphicsInterface::swapBuffers"
        "HeadlessWindow"
    )
    echo "  -- Simbolos clave (direccion + nombre demangled) --"
    for s in "${PROBE_SYMS[@]}"; do
        hit=$(nm -C --defined-only "$BIN" 2>/dev/null | grep -F "$s" | head -1)
        if [ -n "$hit" ]; then
            printf '  %s  %s\n' "$(echo "$hit" | awk '{print $1}')" "$(echo "$hit" | cut -d' ' -f3-)"
        else
            printf '  %-18s %s\n' "(ausente)" "$s"
        fi
    done
fi

# ---------------------------------------------------------------------------
sec "5. CLASES DE GRAFICO: REALES vs STUBS DEL BUILD HEADLESS"
# ---------------------------------------------------------------------------
if command -v nm >/dev/null 2>&1; then
    echo "  -- Vtables de la jerarquia grafica --"
    nm -C --defined-only "$BIN" 2>/dev/null \
        | grep -E "vtable for (GraphicsInterface|GraphicsInterfaceOpenGL|HeadlessGraphicsInterface|Window|SDLWindow|HeadlessWindow)$" \
        | sed 's/^/  /'
    echo
    echo "  -- Los stubs Headless* son metodos vacios: mira el espaciado --"
    nm -C --defined-only "$BIN" 2>/dev/null \
        | grep -E "[TtW] HeadlessGraphicsInterface::" | sort | head -12 \
        | sed 's/^/  /'
    echo
    echo "  -> Todos esos metodos estan a 8 bytes de distancia = un solo 'ret':"
    echo "     el build headless sustituye la clase real en TIEMPO DE COMPILACION."
fi

# ---------------------------------------------------------------------------
sec "6. INVENTARIO DE ASSETS GRAFICOS EN EL PAQUETE"
# ---------------------------------------------------------------------------
DATA_DIR="$(dirname "$(dirname "$(dirname "$BIN")")")/data"
if [ -d "$DATA_DIR" ]; then
    echo "  Data dir: $DATA_DIR"
    echo "  PNG:      $(find "$DATA_DIR" -name '*.png' 2>/dev/null | wc -l | tr -d ' ')"
    echo "  JPG:      $(find "$DATA_DIR" -name '*.jpg' 2>/dev/null | wc -l | tr -d ' ')"
    echo "  Shaders:  $(find "$DATA_DIR" -path '*graphics/shaders*' -type f 2>/dev/null | wc -l | tr -d ' ')"
else
    echo "  (no se encontro directorio data/ junto al binario)"
fi

hr
echo "Sondeo completado."
exit 0
