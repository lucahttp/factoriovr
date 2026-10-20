#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Tests del extractor de assets de Factorio.

Solo biblioteca estandar de Python 3 y sin datos externos: todos los fixtures
se generan en memoria o en un directorio temporal.

Ejecucion:
    python3 -m unittest discover -s scripts/tests -v
"""

import json
import os
import shutil
import struct
import sys
import tempfile
import unittest
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPTS_DIR = os.path.dirname(HERE)
if SCRIPTS_DIR not in sys.path:
    sys.path.insert(0, SCRIPTS_DIR)

import extract_factorio_assets as ex  # noqa: E402


# ===========================================================================
# Utilidades para los tests
# ===========================================================================

def parse_lua(source):
    """Parsea un fragmento Lua y devuelve (cuerpo, evaluador, entorno)."""
    body = ex.LuaParser(ex.tokenize(source), "<test>").parse_chunk()
    runtime = ex.LuaRuntime(".", [])
    env = ex.Env()
    for stmt in body:
        runtime.exec_stmt(stmt, env, "<test>")
    return body, runtime, env


def table_get(table, key):
    """Lee una clave de una LuaTable (None si no existe)."""
    return table.get(key)


# ===========================================================================
# 1. Parser de Lua
# ===========================================================================

class TestLuaParser(unittest.TestCase):

    def test_lua_tables_and_scalars(self):
        source = """
        local sprite = {
          filename = "__base__/graphics/entity/x/x.png",
          width = 106,
          height = 106,
          scale = 0.5,
          draw_as_shadow = true,
          flags = {"mask", "player"},
          shift = util.by_pixel(0, -12),
          nested = { a = 1, [2] = "dos" }
        }
        """
        _body, _runtime, env = parse_lua(source)
        sprite = env.get("sprite")
        self.assertIsInstance(sprite, ex.LuaTable)
        self.assertEqual(sprite.get("filename"),
                         "__base__/graphics/entity/x/x.png")
        self.assertEqual(sprite.get("width"), 106)
        self.assertEqual(sprite.get("height"), 106)
        self.assertEqual(sprite.get("scale"), 0.5)
        self.assertIs(sprite.get("draw_as_shadow"), True)
        self.assertEqual(list(sprite.get("flags").arr), ["mask", "player"])
        shift = sprite.get("shift")
        self.assertEqual(shift.get(1), 0)
        self.assertEqual(shift.get(2), -12)
        self.assertEqual(sprite.get("nested").get("a"), 1)
        self.assertEqual(sprite.get("nested").get(2), "dos")

    def test_lua_frames_numeric(self):
        """`frames` como numero: cuenta de frames."""
        source = """
        local s = { filename = "a.png", frames = 4, frame_count = 4 }
        """
        _body, _runtime, env = parse_lua(source)
        s = env.get("s")
        self.assertEqual(s.get("frames"), 4)
        self.assertEqual(s.get("frame_count"), 4)

    def test_lua_frames_table(self):
        """`frames` como tabla con frame_width/frame_height/count/animacion."""
        source = """
        local s = {
          filename = "a.png",
          frames = {
            frame_width = 53,
            frame_height = 53,
            count = 8,
            animation = { frames = 8, speed = 0.15 }
          }
        }
        """
        _body, _runtime, env = parse_lua(source)
        frames = env.get("s").get("frames")
        self.assertIsInstance(frames, ex.LuaTable)
        self.assertEqual(frames.get("frame_width"), 53)
        self.assertEqual(frames.get("frame_height"), 53)
        self.assertEqual(frames.get("count"), 8)
        animation = frames.get("animation")
        self.assertEqual(animation.get("frames"), 8)
        self.assertEqual(animation.get("speed"), 0.15)

    def test_lua_function_returning_table(self):
        """`f{...}` y `f(...)`: una funcion de usuario que devuelve tabla."""
        source = """
        function gun_turret_extension(inputs)
          return {
            filename = "gun.png",
            width = 130,
            frame_count = inputs.frame_count or 5,
            run_mode = inputs.run_mode or "forward"
          }
        end
        local a = gun_turret_extension{frame_count = 1, line_length = 1}
        local b = gun_turret_extension{}
        """
        _body, _runtime, env = parse_lua(source)
        a = env.get("a")
        self.assertEqual(a.get("filename"), "gun.png")
        self.assertEqual(a.get("frame_count"), 1)
        b = env.get("b")
        self.assertEqual(b.get("frame_count"), 5)
        self.assertEqual(b.get("run_mode"), "forward")

    def test_lua_util_sprite_load(self):
        """util.sprite_load(ruta, props) -> capa con filename y strip_path."""
        source = """
        local layer = util.sprite_load(
          "__base__/graphics/entity/assembling-machine-1/assembling-machine-1-base",
          { priority = "high", frame_count = 1, repeat_count = 64, scale = 0.5 })
        """
        _body, _runtime, env = parse_lua(source)
        layer = env.get("layer")
        self.assertEqual(layer.get("filename"),
                         "__base__/graphics/entity/assembling-machine-1/"
                         "assembling-machine-1-base.png")
        self.assertEqual(layer.get("strip_path"),
                         "__base__/graphics/entity/assembling-machine-1/"
                         "assembling-machine-1-base")
        self.assertEqual(layer.get("priority"), "high")
        self.assertEqual(layer.get("repeat_count"), 64)

    def test_lua_data_extend_captures_prototypes(self):
        """data:extend({...}) registra los prototipos."""
        source = """
        data:extend
        {
          {
            type = "furnace",
            name = "stone-furnace",
            icon = "__base__/graphics/icons/stone-furnace.png"
          }
        }
        """
        body, runtime, _env = parse_lua(source)
        self.assertEqual(len(runtime.extended), 1)
        prototype = runtime.extended[0].get(1)
        self.assertEqual(prototype.get("type"), "furnace")
        self.assertEqual(prototype.get("name"), "stone-furnace")

    def test_lua_string_escapes_and_concat(self):
        source = r"""
        local a = "a" .. "b"
        local b = "x\ny"
        """
        _body, _runtime, env = parse_lua(source)
        self.assertEqual(env.get("a"), "ab")
        self.assertEqual(env.get("b"), "x\ny")

    def test_lua_comments_are_ignored(self):
        source = """
        -- comentario de linea
        --[[ comentario
             multilinea ]]
        local x = 1 -- al final
        """
        _body, _runtime, env = parse_lua(source)
        self.assertEqual(env.get("x"), 1)

    def test_lua_geometry_from_layer(self):
        """La geometria se deduce de la capa y del tamano del PNG."""
        source = """
        local s = { filename = "a.png", width = 140, height = 158,
                    frame_count = 64 }
        """
        _body, _runtime, env = parse_lua(source)
        layer = env.get("s")
        # PNG de 1120x1264 -> 8 columnas de 140x158
        geo = ex.compute_geometry(layer, ".", 1120, 1264, [])
        self.assertEqual(geo.frame_width, 140)
        self.assertEqual(geo.frame_height, 158)
        self.assertEqual(geo.cols, 8)
        self.assertEqual(geo.rows, 8)
        self.assertEqual(geo.cells, 64)
        self.assertEqual(geo.frame_count, 64)
        self.assertEqual(geo.anim_frames, 64)

    def test_lua_width_in_frames(self):
        """Factorio 2.1: width_in_frames / height_in_frames."""
        source = """
        local s = { filename = "a.png", width_in_frames = 2, height_in_frames = 16 }
        """
        _body, _runtime, env = parse_lua(source)
        layer = env.get("s")
        geo = ex.compute_geometry(layer, ".", 264, 2080, [])
        self.assertEqual(geo.frame_width, 132)
        self.assertEqual(geo.frame_height, 130)
        self.assertEqual(geo.cols, 2)
        self.assertEqual(geo.rows, 16)

    def test_lua_direction_count(self):
        """Las cintas direccionales (cintas transportadoras) declaran direccion."""
        source = """
        local s = { filename = "a.png", size = 128, frame_count = 16,
                    direction_count = 20 }
        """
        _body, _runtime, env = parse_lua(source)
        layer = env.get("s")
        geo = ex.compute_geometry(layer, ".", 2048, 2560, [])
        self.assertEqual(geo.frame_width, 128)
        self.assertEqual(geo.cols, 16)
        self.assertEqual(geo.rows, 20)
        self.assertEqual(geo.cells, 320)
        self.assertEqual(geo.direction_count, 20)
        self.assertEqual(geo.frame_count, 16)


# ===========================================================================
# 2. Codificador y decodificador PNG
# ===========================================================================

class TestPngCodec(unittest.TestCase):

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="xfactorvr-png-")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    @staticmethod
    def make_image(width, height):
        """Imagen RGBA con un patron reconocible."""
        pixels = bytearray()
        for y in range(height):
            for x in range(width):
                pixels += bytes([(x * 17) % 256, (y * 29) % 256,
                                 (x + y) * 7 % 256, 255 if (x + y) % 3 else 0])
        return width, height, bytes(pixels)

    def test_encode_decode_roundtrip_pixels(self):
        width, height, pixels = self.make_image(7, 5)
        blob = ex.png_encode_rgba(width, height, pixels)
        path = os.path.join(self.tmp, "roundtrip.png")
        with open(path, "wb") as handle:
            handle.write(blob)

        header = ex.read_png_header(path)
        self.assertEqual(header.width, width)
        self.assertEqual(header.height, height)
        self.assertEqual(header.bit_depth, 8)
        self.assertEqual(header.color_type, 6)
        self.assertEqual(header.interlace, 0)

        rows = dict(ex.decode_png_rows(path))
        self.assertEqual(len(rows), height)
        for y in range(height):
            expected = pixels[y * width * 4:(y + 1) * width * 4]
            self.assertEqual(rows[y], expected,
                             "fila %d no coincide" % y)

    def test_encoded_chunks_have_valid_crc(self):
        width, height, pixels = self.make_image(3, 3)
        blob = ex.png_encode_rgba(width, height, pixels)
        self.assertEqual(blob[:8], ex.PNG_SIGNATURE)

        offset = 8
        seen = []
        while offset < len(blob):
            (length,) = struct.unpack(">I", blob[offset:offset + 4])
            ctype = blob[offset + 4:offset + 8]
            payload = blob[offset + 8:offset + 8 + length]
            (crc,) = struct.unpack(">I", blob[offset + 8 + length:
                                               offset + 12 + length])
            self.assertEqual(crc, zlib.crc32(ctype + payload) & 0xFFFFFFFF,
                             "CRC incorrecto en el chunk %r" % ctype)
            seen.append(ctype)
            offset += 12 + length
        self.assertEqual(seen, [b"IHDR", b"IDAT", b"IEND"])

    def test_encoder_rejects_wrong_buffer_size(self):
        with self.assertRaises(ex.PngError):
            ex.png_encode_rgba(4, 4, b"\x00" * 10)

    def test_decoder_rejects_interlaced_png(self):
        """Un PNG Adam7 debe fallar con un error claro."""
        ihdr = struct.pack(">IIBBBBB", 2, 2, 8, 6, 0, 0, 1)  # interlace = 1
        def chunk(ctype, payload):
            return (struct.pack(">I", len(payload)) + ctype + payload +
                    struct.pack(">I", zlib.crc32(ctype + payload) & 0xFFFFFFFF))
        blob = (ex.PNG_SIGNATURE + chunk(b"IHDR", ihdr) +
                chunk(b"IDAT", zlib.compress(b"\x00" * 20)) + chunk(b"IEND", b""))
        path = os.path.join(self.tmp, "interlaced.png")
        with open(path, "wb") as handle:
            handle.write(blob)
        with self.assertRaises(ex.PngError):
            list(ex.decode_png_rows(path))

    def test_decoder_handles_rgb_without_alpha(self):
        """Un PNG RGB (sin canal alpha) se convierte a RGBA opaco."""
        def chunk(ctype, payload):
            return (struct.pack(">I", len(payload)) + ctype + payload +
                    struct.pack(">I", zlib.crc32(ctype + payload) & 0xFFFFFFFF))
        width, height = 2, 2
        raw = b""
        for _ in range(height):
            raw += b"\x00" + bytes([255, 0, 0, 0, 255, 0])
        ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
        blob = (ex.PNG_SIGNATURE + chunk(b"IHDR", ihdr) +
                chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))
        path = os.path.join(self.tmp, "rgb.png")
        with open(path, "wb") as handle:
            handle.write(blob)
        rows = dict(ex.decode_png_rows(path))
        self.assertEqual(rows[0][:4], bytes([255, 0, 0, 255]))
        self.assertEqual(rows[0][4:8], bytes([0, 255, 0, 255]))


# ===========================================================================
# 3. Empaquetado del atlas
# ===========================================================================

class TestAtlasPacking(unittest.TestCase):

    @staticmethod
    def overlaps(a, b):
        ax, ay, aw, ah = a
        bx, by, bw, bh = b
        return not (ax + aw <= bx or bx + bw <= ax or
                    ay + ah <= by or by + bh <= ay)

    def test_packed_rectangles_do_not_overlap_and_fit(self):
        rects = [
            (64, 64, "a"), (128, 96, "b"), (32, 200, "c"),
            (200, 40, "d"), (48, 48, "e"), (256, 128, "f"),
            (16, 16, "g"), (100, 100, "h"),
        ]
        max_width = 512
        placements, width, height = ex.pack_skyline(rects, max_width)
        self.assertLessEqual(width, max_width)
        self.assertGreater(width, 0)
        self.assertGreater(height, 0)
        self.assertEqual(len(placements), len(rects))
        for w, h, key in rects:
            _page, x, y = placements[key]
            self.assertGreaterEqual(x, 0)
            self.assertGreaterEqual(y, 0)
            self.assertLessEqual(x + w, max_width)
            self.assertLessEqual(y + h, height)
        # ningun par se solapa
        items = [(placements[k][1], placements[k][2], w, h)
                 for w, h, k in rects]
        for i in range(len(items)):
            for j in range(i + 1, len(items)):
                self.assertFalse(self.overlaps(items[i], items[j]),
                                 "los rectangulos %d y %d se solapan" % (i, j))

    def test_shelf_packing_also_does_not_overlap(self):
        rects = [(64, 64, "a"), (128, 96, "b"), (32, 200, "c"), (48, 48, "d")]
        placements, width, height = ex.pack_shelf(rects, 256)
        self.assertLessEqual(width, 256)
        items = [(placements[k][0], placements[k][1], w, h)
                 for w, h, k in rects]
        for i in range(len(items)):
            for j in range(i + 1, len(items)):
                self.assertFalse(self.overlaps(items[i], items[j]))

    def test_rectangle_wider_than_atlas_is_not_placed(self):
        placements, _width, _height = ex.pack_skyline([(500, 10, "ancho")], 256)
        self.assertNotIn("ancho", placements)

    def test_priority_maps_to_draw_order(self):
        # escala de Factorio: extra-low < low < terrain < terrain-high <
        # object < high < extra-high
        self.assertEqual(ex.PRIORITY_DRAW_ORDER["extra-low"], 0)
        self.assertEqual(ex.PRIORITY_DRAW_ORDER["low"], 1)
        self.assertEqual(ex.PRIORITY_DRAW_ORDER["terrain"], 2)
        self.assertEqual(ex.PRIORITY_DRAW_ORDER["terrain-high"], 3)
        self.assertEqual(ex.PRIORITY_DRAW_ORDER["object"], 4)
        self.assertEqual(ex.PRIORITY_DRAW_ORDER["high"], 5)
        self.assertEqual(ex.PRIORITY_DRAW_ORDER["extra-high"], 6)
        self.assertLess(ex.PRIORITY_DRAW_ORDER["terrain"],
                        ex.PRIORITY_DRAW_ORDER["object"])
        self.assertLess(ex.PRIORITY_DRAW_ORDER["object"],
                        ex.PRIORITY_DRAW_ORDER["high"])
        self.assertLess(ex.PRIORITY_DRAW_ORDER["high"],
                        ex.PRIORITY_DRAW_ORDER["extra-high"])


# ===========================================================================
# 3.b Paginacion del atlas
# ===========================================================================

class TestPagePacker(unittest.TestCase):

    def test_pages_never_exceed_page_size(self):
        packer = ex.PagePacker(256)
        for i in range(40):
            packer.add(("k", i), 100, 120)
        self.assertGreater(packer.page_count(), 1)
        for key, (page, x, y) in packer.placements().items():
            self.assertLessEqual(x + 100, 256)
            self.assertLessEqual(y + 120, 256)
            self.assertLess(page, packer.page_count())

    def test_rectangles_do_not_overlap_inside_a_page(self):
        packer = ex.PagePacker(256)
        for i in range(30):
            packer.add(("k", i), 64, 48)
        by_page = {}
        for key, (page, x, y) in packer.placements().items():
            by_page.setdefault(page, []).append((x, y, 64, 48))
        for page, rects in by_page.items():
            for i in range(len(rects)):
                ax, ay, aw, ah = rects[i]
                for j in range(i + 1, len(rects)):
                    bx, by, bw, bh = rects[j]
                    self.assertFalse(
                        not (ax + aw <= bx or bx + bw <= ax or
                             ay + ah <= by or by + bh <= ay),
                        "solape en la pagina %d" % page)

    def test_rect_bigger_than_page_is_rejected(self):
        packer = ex.PagePacker(128)
        self.assertIsNone(packer.add("grande", 256, 256))

    def test_paginated_pack_shelf_equivalent(self):
        rects = [(64, 64, "a"), (128, 96, "b"), (32, 200, "c"), (48, 48, "d")]
        placements, width, height = ex.pack_skyline(rects, 256)
        self.assertLessEqual(width, 256)
        self.assertGreaterEqual(height, 1)
        # el empaquetador pagina: cada clave dice (pagina, x, y)
        for key, (page, x, y) in placements.items():
            self.assertGreaterEqual(x, 0)
            self.assertGreaterEqual(y, 0)
            self.assertLessEqual(x + 64, 256)


# ===========================================================================
# 3.c Variantes de direccion y capas de cuerpo
# ===========================================================================

class TestVariants(unittest.TestCase):

    def test_detect_variant_direction_and_flip(self):
        self.assertEqual(
            ex.detect_variant("__base__/graphics/entity/burner-mining-drill/"
                              "burner-mining-drill-N.png"), ("N", False))
        self.assertEqual(
            ex.detect_variant("__base__/graphics/entity/burner-mining-drill/"
                              "burner-mining-drill-E-flipped.png"), ("E", True))
        self.assertEqual(
            ex.detect_variant("__base__/graphics/entity/assembling-machine-1/"
                              "assembling-machine-1-base.png"), (None, False))
        self.assertEqual(
            ex.detect_variant("__base__/graphics/entity/stone-furnace/"
                              "stone-furnace-shadow.png"), (None, False))

    def test_is_body_detects_shadows_and_lights(self):
        class FakeLayer(object):
            def __init__(self, ref, flags=()):
                self.ref = ref
                self.flags = list(flags)

        self.assertTrue(ex.is_body_layer(
            FakeLayer("base/graphics/entity/a/a-N.png")))
        self.assertFalse(ex.is_body_layer(
            FakeLayer("base/graphics/entity/a/a-N-shadow.png")))
        self.assertFalse(ex.is_body_layer(
            FakeLayer("base/graphics/entity/a/a-status-light.png")))
        self.assertFalse(ex.is_body_layer(
            FakeLayer("base/graphics/entity/a/a-reflection.png")))
        self.assertFalse(ex.is_body_layer(
            FakeLayer("base/graphics/entity/a/a.png", ["mask"])))

    def test_listed_entity_types_cover_the_factory(self):
        for name in ("assembling-machine-1", "transport-belt", "inserter",
                     "iron-chest", "gun-turret", "radar", "roboport",
                     "stone-furnace", "splitter", "underground-belt"):
            self.assertIn(name, ex.LISTED_ENTITY_NAMES)
        # los arboles entran por tipo entero
        self.assertIn("tree", ex.LISTED_ENTITY_TYPES)
        # y el filtro NO debe traer los tipos pesados que no estan en la lista
        for name in ("fast-splitter", "express-splitter",
                     "fast-underground-belt", "express-underground-belt",
                     "small-biter", "boss-biter"):
            self.assertNotIn(name, ex.LISTED_ENTITY_NAMES)

    def test_swar_average_matches_python(self):
        import random
        random.seed(11)
        for size in (1, 3, 16, 100):
            a = bytes(random.randrange(256) for _ in range(size))
            b = bytes(random.randrange(256) for _ in range(size))
            self.assertEqual(ex._avg2(a, b),
                             bytes((x + y) // 2 for x, y in zip(a, b)))

    def test_downscale_2x2_averages_four_pixels(self):
        # dos pixeles de 2x2: R=10 G=20 B=30 A=40 y R=14 G=24 B=34 A=44
        row0 = bytes([10, 20, 30, 40, 14, 24, 34, 44])
        row1 = bytes([12, 22, 32, 42, 16, 26, 36, 46])
        out = ex.downscale_2x2(row0, row1, 1)
        self.assertEqual(list(out), [13, 23, 33, 43])

    def test_resample_block_generic_matches_box_average(self):
        # un bloque de 4x2 pixeles reducido a 2x1
        rows = {0: bytes([0, 0, 0, 0, 10, 10, 10, 10,
                          20, 20, 20, 20, 30, 30, 30, 30]),
                1: bytes([0, 0, 0, 0, 10, 10, 10, 10,
                          20, 20, 20, 20, 30, 30, 30, 30])}
        out = ex.resample_block_generic(rows, 0, 0, 4, 2, 2, 1)
        self.assertEqual(len(out), 1)
        # los dos primeros pixeles valen 0 y los dos ultimos 10 -> 5
        self.assertEqual(list(out[0][:4]), [5, 5, 5, 5])


# ===========================================================================
# 4. Locale -> strings.json
# ===========================================================================

class TestLocale(unittest.TestCase):

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="xfactorvr-locale-")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def write_cfg(self, text):
        path = os.path.join(self.tmp, "base.cfg")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
        return path

    def test_parse_locale_sections_and_entries(self):
        path = self.write_cfg(
            "; un comentario\n"
            "parameter-x=Parameter __1__\n"
            "\n"
            "[item-name]\n"
            "iron-gear-wheel=Iron gear wheel\n"
            "iron-plate=Iron plate\n"
            "\n"
            "[entity-name]\n"
            "assembling-machine-1=Assembling machine 1\n"
            "submarine.1=Submarine\n"
        )
        strings = ex.parse_locale(path)
        self.assertEqual(strings["item-name.iron-gear-wheel"], "Iron gear wheel")
        self.assertEqual(strings["item-name.iron-plate"], "Iron plate")
        self.assertEqual(strings["entity-name.assembling-machine-1"],
                         "Assembling machine 1")
        # las claves con punto se guardan tal cual
        self.assertEqual(strings["entity-name.submarine.1"], "Submarine")
        # la seccion vacia deja la clave suelta
        self.assertEqual(strings["parameter-x"], "Parameter __1__")
        self.assertEqual(len(strings), 5)

    def test_strings_json_is_serializable(self):
        path = self.write_cfg("[item-name]\niron-gear-wheel=Iron gear wheel\n")
        strings = ex.parse_locale(path)
        out = os.path.join(self.tmp, "strings.json")
        with open(out, "w", encoding="utf-8") as handle:
            json.dump(strings, handle, ensure_ascii=False, indent=1,
                      sort_keys=True)
        with open(out, "r", encoding="utf-8") as handle:
            loaded = json.load(handle)
        self.assertEqual(loaded["item-name.iron-gear-wheel"], "Iron gear wheel")


# ===========================================================================
# 5. Utilidades varias
# ===========================================================================

class TestHelpers(unittest.TestCase):

    def test_relative_and_absolute_asset_paths(self):
        data_dir = "/juego/data"
        self.assertEqual(
            ex.relative_asset_path(data_dir,
                                   "__base__/graphics/entity/a/a.png"),
            "base/graphics/entity/a/a.png")
        self.assertEqual(
            ex.relative_asset_path(data_dir,
                                   "__core__/graphics/empty.png"),
            "core/graphics/empty.png")
        self.assertEqual(
            ex.resolve_asset_path(data_dir, "__base__/graphics/entity/a/a.png"),
            os.path.join(data_dir, "base/graphics/entity/a/a.png"))

    def test_detect_version_reads_info_json(self):
        tmp = tempfile.mkdtemp(prefix="xfactorvr-version-")
        try:
            os.makedirs(os.path.join(tmp, "base", "graphics", "entity"))
            with open(os.path.join(tmp, "base", "info.json"), "w",
                      encoding="utf-8") as handle:
                json.dump({"name": "base", "version": "2.1.22"}, handle)
            self.assertEqual(ex.detect_version(tmp), "2.1.22")
            # la version manda: 2.1 usa tiras, 2.0 no
            self.assertEqual(ex.detect_graphics_layout(tmp, "2.1.22"), "2.1")
            self.assertEqual(ex.detect_graphics_layout(tmp, "2.0.77"), "2.0")
            # sin version se mira si hay un .lua de tira junto a su PNG
            entity = os.path.join(tmp, "base", "graphics", "entity")
            with open(os.path.join(entity, "x.png"), "wb") as handle:
                handle.write(b"\x89PNG\r\n\x1a\n")
            self.assertEqual(ex.detect_graphics_layout(tmp), "2.0")
            with open(os.path.join(entity, "x.lua"), "w",
                      encoding="utf-8") as handle:
                handle.write("return { width = 1, height = 1 }\n")
            self.assertEqual(ex.detect_graphics_layout(tmp), "2.1")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_layer_score_prefers_more_frames(self):
        source = """
        local a = { filename = "x.png", frame_count = 1 }
        local b = { filename = "x.png", frame_count = 5 }
        local c = { filename = "x.png", width_in_frames = 2 }
        """
        _body, _runtime, env = parse_lua(source)
        self.assertEqual(ex._layer_score(env.get("a")), 1)
        self.assertEqual(ex._layer_score(env.get("b")), 5)
        self.assertEqual(ex._layer_score(env.get("c")), 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
