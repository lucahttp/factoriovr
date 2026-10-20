#!/usr/bin/env python3
"""preview_scene.py — Render offline de una escena de Factorio, sin visor.

Replica EXACTAMENTE la logica del motor (src/assets.rs + openxr_engine.cpp):
mismo orden de ids de sprite, mismo calculo de fila/animacion, mismo tamaño
en casillas y mismas UV. Sirve para comprobar que los sprites reales se
colocan bien antes de depender de la Pico.

Uso:
    python3 scripts/preview_scene.py --assets build/factorio_assets \
        --snapshot tmp/scene.json --out build/preview.png

El snapshot es el JSON que devuelve el mod por RCON:
    {"entities":[{"t":"assembling-machine-1","x":..,"y":..,"o":..,"f":..,"d":..}, ...],
     "patches":[...]}
"""

import argparse
import importlib.util
import json
import os
import sys

RAIZ = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def cargar_extractor():
    """Reutiliza el decodificador PNG y el codificador del extractor."""
    ruta = os.path.join(RAIZ, "scripts", "extract_factorio_assets.py")
    spec = importlib.util.spec_from_file_location("extractor_assets", ruta)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def ids_sprite(manifiesto):
    """Mismo criterio que sprite_ids_for() en Rust: orden alfabetico, 1-based."""
    nombres = sorted(manifiesto.get("entities", {}).keys())
    return {n: i + 1 for i, n in enumerate(nombres)}


def frame_de_la_capa(capa, frame_anim, direccion):
    """Mismo calculo que sprite_frame_in() en Rust."""
    anim = max(1, (capa.get("animation") or {}).get("frames", 1))
    dirs = max(1, capa.get("direction_count", 1))
    fila = 0 if dirs == 1 else min((direccion * dirs) // 16, dirs - 1)
    por_fila = max(1, capa.get("frame_count", 1) // dirs)
    idx_anim = (frame_anim % anim) if (anim > 0 and por_fila > 1) else 0
    indice = min(fila * por_fila + idx_anim, max(0, capa.get("frame_count", 1) - 1))
    rects = capa.get("frames", [])
    if indice >= len(rects):
        return None
    rect = dict(rects[indice])
    # La pagina puede venir en el rect o en la capa; las coordenadas pueden ser
    # relativas al origen de la capa.
    if not rect.get("page"):
        rect["page"] = capa.get("page", 0)
    rect["x"] = rect.get("x", 0) + capa.get("atlas_x", 0)
    rect["y"] = rect.get("y", 0) + capa.get("atlas_y", 0)
    return rect


VARIANTES = ["N", "E", "S", "W"]


def capa_cuerpo(entidad, direccion=0):
    """Capa a dibujar: la variante cardinal si la hay, si no el cuerpo."""
    capas = entidad.get("layers", [])
    dibujan = [c for c in capas if not c.get("omitted") and c.get("frames")]
    variants = entidad.get("variants") or {}
    if variants and dibujan:
        grupo = min(direccion // 4, 3)
        idx = variants.get(VARIANTES[grupo])
        if idx is not None and 0 <= idx < len(capas):
            c = capas[idx]
            if not c.get("omitted") and c.get("frames"):
                return c
    if not dibujan:
        return None
    for c in dibujan:
        if c.get("is_body"):
            return c
    return dibujan[0]


class Paginas:
    """Paginas del atlas en crudo (RGBA8)."""

    def __init__(self, dir_assets, manifiesto):
        atlas = manifiesto.get("atlas", {})
        self.paginas = atlas.get("pages", [])
        self.datos = {}
        for i, pag in enumerate(self.paginas):
            ruta = os.path.join(dir_assets, pag.get("file", ""))
            if os.path.exists(ruta):
                with open(ruta, "rb") as fh:
                    self.datos[i] = (fh.read(), pag["width"], pag["height"])

    def pixel(self, pagina, x, y):
        datos = self.datos.get(pagina)
        if not datos:
            return None
        raw, w, h = datos
        if x < 0 or y < 0 or x >= w or y >= h:
            return None
        off = (y * w + x) * 4
        return raw[off], raw[off + 1], raw[off + 2], raw[off + 3]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--assets", default=os.path.join(RAIZ, "build", "factorio_assets"))
    ap.add_argument("--snapshot", required=True)
    ap.add_argument("--out", default=os.path.join(RAIZ, "build", "preview.png"))
    ap.add_argument("--tile", type=int, default=10, help="pixeles por casilla")
    ap.add_argument("--center", type=float, nargs=2, default=(0.0, 0.0))
    ap.add_argument("--radius", type=float, default=0.0,
                    help="recorta la vista a este radio en casillas alrededor del centro")
    args = ap.parse_args()

    ext = cargar_extractor()
    with open(os.path.join(args.assets, "manifest.json"), encoding="utf-8") as fh:
        manifiesto = json.load(fh)
    with open(args.snapshot, encoding="utf-8") as fh:
        snap = json.load(fh)
    if "entities" not in snap and "entities" in snap.get("snapshot", {}):
        snap = snap["snapshot"]

    paginas = Paginas(args.assets, manifiesto)
    ids = ids_sprite(manifiesto)

    entidades = snap.get("entities", [])
    if not entidades:
        print("La instantanea no trae entidades.", file=sys.stderr)
        return 1

    # Recorte opcional de la vista.
    if args.radius > 0:
        cx, cy = args.center
        entidades = [e for e in entidades
                     if (e["x"] - cx) ** 2 + (e["y"] - cy) ** 2 <= args.radius ** 2]
        if not entidades:
            print("Ninguna entidad dentro del radio.", file=sys.stderr)
            return 1
        cx, cy = args.center
        min_x = int(cx - args.radius)
        max_x = int(cx + args.radius)
        min_y = int(cy - args.radius)
        max_y = int(cy + args.radius)
    else:
        # Escala: 1 casilla = 32 px de juego; el sprite mide frame_width px * scale.
        xs = [e["x"] for e in entidades]
        ys = [e["y"] for e in entidades]
        min_x, max_x = min(xs), max(xs)
        min_y, max_y = min(ys), max(ys)

    ancho_tiles = max(8, int(max_x - min_x) + 4)
    alto_tiles = max(8, int(max_y - min_y) + 4)
    T = args.tile
    W = ancho_tiles * T
    H = alto_tiles * T

    # Fondo: cesped con rejilla.
    escena = bytearray(W * H * 4)
    for y in range(H):
        for x in range(W):
            checker = 1 if ((x // T) + (y // T)) % 2 == 0 else 0
            g = 96 + checker * 10
            off = (y * W + x) * 4
            escena[off] = 58
            escena[off + 1] = g
            escena[off + 2] = 52
            escena[off + 3] = 255

    def blit(pagina, rect, dx, dy, dw, dh):
        """Pega el frame del atlas escalado a (dx,dy) con tamano (dw,dh)."""
        sx, sy = int(rect["x"]), int(rect["y"])
        sw, sh = int(rect["w"]), int(rect["h"])
        if sw <= 0 or sh <= 0 or dw <= 0 or dh <= 0:
            return
        for py in range(dh):
            ty = dy + py
            if ty < 0 or ty >= H:
                continue
            for px in range(dw):
                tx = dx + px
                if tx < 0 or tx >= W:
                    continue
                sxp = sx + px * sw // dw
                syp = sy + py * sh // dh
                p = paginas.pixel(pagina, sxp, syp)
                if p is None:
                    continue
                r, g, b, a = p
                if a == 0:
                    continue
                off = (ty * W + tx) * 4
                if a == 255:
                    escena[off] = r
                    escena[off + 1] = g
                    escena[off + 2] = b
                else:
                    al = a / 255.0
                    escena[off] = int(escena[off] * (1 - al) + r * al)
                    escena[off + 1] = int(escena[off + 1] * (1 - al) + g * al)
                    escena[off + 2] = int(escena[off + 2] * (1 - al) + b * al)

    # Orden de dibujo: primero lo que esta mas abajo (terreno), luego objetos.
    for e in sorted(entidades, key=lambda z: z.get("y", 0.0)):
        nombre = e.get("t", "")
        sid = ids.get(nombre, 0)
        if sid == 0:
            continue
        ent = manifiesto["entities"][nombre]
        capa = capa_cuerpo(ent, e.get("d", 0))
        if capa is None:
            continue
        rect = frame_de_la_capa(capa, e.get("f", 0), e.get("d", 0))
        if rect is None:
            continue
        # Tamano en casillas: frame_width px * escala / 32 px por casilla.
        casillas = capa.get("frame_width", 32) * capa.get("scale", 0.5) / 32.0
        lado = max(1, int(round(casillas * T)))
        cx = int((e["x"] - min_x + 2) * T)
        cy = int((e["y"] - min_y + 2) * T)
        blit(rect.get("page", 0), rect, cx - lado // 2, cy - lado // 2, lado, lado)

    # Parches de mineral: se pintan como manchas de color.
    COLORES = {1: (120, 120, 130), 2: (200, 140, 90), 3: (60, 60, 60), 4: (90, 200, 120)}
    for p in snap.get("patches", []):
        col = COLORES.get(p.get("t", 1), (150, 150, 150))
        cx = int((p["x"] - min_x + 2) * T)
        cy = int((p["y"] - min_y + 2) * T)
        r = T
        for py in range(-r, r + 1):
            for px in range(-r, r + 1):
                if px * px + py * py > r * r:
                    continue
                tx, ty = cx + px, cy + py
                if 0 <= tx < W and 0 <= ty < H:
                    off = (ty * W + tx) * 4
                    escena[off] = col[0]
                    escena[off + 1] = col[1]
                    escena[off + 2] = col[2]

    with open(args.out, "wb") as fh:
        fh.write(ext.png_encode_rgba(W, H, bytes(escena)))

    dibujadas = sum(1 for e in entidades if ids.get(e.get("t", ""), 0) > 0)
    print(f"Escena: {W}x{H} px, {len(entidades)} entidades, {dibujadas} con sprite real")
    print(f"Salida: {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
