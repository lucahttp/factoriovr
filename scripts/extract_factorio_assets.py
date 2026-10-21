#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Extractor de assets reales de Factorio para el motor VR de XFactorVR.

Extrae de una instalacion de Factorio (2.1.x o 2.0.x) los sprites reales de
las entidades, los iconos de items, los textos EN del juego y el listado de
sonidos, y los deja listos para que el motor VR los consuma:

    <out>/atlas.png        atlas RGBA8 con todos los frames de sprite
    <out>/manifest.json    indice de entidades/capas/frames + iconos + sonidos
    <out>/icons/<n>.png    iconos de items copiados tal cual
    <out>/terrain/<n>.png  tiles de terreno (copiados tal cual)
    <out>/strings.json     texto EN del juego

Solo biblioteca estandar de Python 3.  No usa Pillow, numpy ni red.

Uso:
    python3 scripts/extract_factorio_assets.py --data <dir_data> --out <dir>

Soporta las dos estructuras de sprites de Factorio:

  * 2.0.x  ->  <tipo>.png en base/graphics/entity/<tipo>/, con la geometria de
               frames en el propio prototipo (width/height/line_length/frame_count
               o la tabla `frames = { frame_width = ..., frame_height = ... }`).
  * 2.1.x  ->  <tipo>-<capa>.png + <tipo>-<capa>.lua (definicion de tira) al
               lado, referenciados desde el prototipo via
               `util.sprite_load("ruta", { ... })`.

Los prototipos de Factorio son tablas Lua.  Este script incluye un parser de
un subconjunto de Lua (solo stdlib) suficiente para leerlos: cadenas, numeros,
booleanos, nil, tablas, funciones, y llamadas a funciones (incluidas las
`util.sprite_load` / `util.by_pixel` y las funciones de usuario que devuelven
una tabla).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import struct
import sys
import zlib

# ---------------------------------------------------------------------------
# 0. Constantes
# ---------------------------------------------------------------------------

# Orden de dibujado derivado de `priority`.  Escala de Factorio:
# extra-low < low < terrain < terrain-high < object < high < extra-high
PRIORITY_DRAW_ORDER = {
    "extra-low": 0,
    "low": 1,
    "terrain": 2,
    "terrain-high": 3,
    "object": 4,
    "normal": 4,
    "high": 5,
    "extra-high": 6,
    # valores adicionales que aparecen en algunos prototipos
    "terrain-low": 1,
    "train": 5,
    "extra-high-no-glow": 6,
    "ghost": 7,
    "selection-box": 8,
    "highlight": 9,
    "alert-icon": 10,
}
DEFAULT_DRAW_ORDER = 4

# Altura maxima del atlas.  65535 es el limite de la especificacion PNG que
# respetan los decodificadores habituales.
PNG_MAX_DIMENSION = 65535

# Una capa cuyo frame individual supere esto se considera no apta para el
# atlas y se descarta con aviso (evita decodificar hojas de 4096x8192 enteras).
MAX_FRAME_PIXELS = 4 * 1024 * 1024

# Tipos de prototipo de Factorio que son entidades renderizables.
ENTITY_TYPE_HINTS = {
    "entity", "assembling-machine", "furnace", "mining-drill", "transport-belt",
    "underground-belt", "splitter", "inserter", "burner-inserter", "electric-pole",
    "container", "logistic-container", "logistic-chest", "turret", "ammo-turret",
    "fluid-turret", "radar", "roboport", "logistic-robot-port", "burner-generator",
    "boiler", "reactor", "generator", "storage-tank", "pipe", "pipe-to-ground",
    "loader", "loader-1x1", "pump", "offshore-pump", "valve", "lab", "rocket-silo",
    "agricultural-tower", "character", "spider-vehicle", "spidertron", "car",
    "locomotive", "cargo-wagon", "fluid-wagon", "artillery-wagon", "train-stop",
    "rail-planner", "wall", "gate", "landfill", "fish", "biter", "spitter",
    "spawner", "small-biter", "medium-biter", "big-biter", "boss-biter",
    "flying-robot", "combat-robot", "construction-robot", "logistic-robot",
    "beacon", "buffer", "rail-chain-signal", "rail-signal", "circuit-connector",
    "explosion", "projectile", "fire", "smoke", "burner", "rocket-launcher",
    "explosive-turret", "flamethrower-turret", "laser-turret", "artillery-turret",
    "oil-well", "simple-entity", "cliff", "resource", "tree", "fish-school",
}

# Tipos de entidad que son "maquinaria de fabrica": se conservan siempre en
# el atlas, aunque haya que descartar explosiones, proyectiles o decorators
# para que todo quepa.  El resto se considera prescindible.
CORE_ENTITY_TYPES = {
    "entity", "assembling-machine", "furnace", "mining-drill", "transport-belt",
    "underground-belt", "splitter", "inserter", "burner-inserter", "electric-pole",
    "container", "logistic-container", "logistic-chest", "turret", "ammo-turret",
    "fluid-turret", "radar", "roboport", "logistic-robot-port", "burner-generator",
    "boiler", "reactor", "generator", "storage-tank", "pipe", "pipe-to-ground",
    "loader", "loader-1x1", "pump", "offshore-pump", "valve", "lab", "rocket-silo",
    "agricultural-tower", "character", "spider-vehicle", "spidertron", "car",
    "locomotive", "cargo-wagon", "fluid-wagon", "artillery-wagon", "train-stop",
    "wall", "gate", "landfill", "beacon", "burner", "rocket-launcher",
    "explosive-turret", "flamethrower-turret", "laser-turret", "artillery-turret",
    "oil-well", "rail-planner", "electric-furnace", "agricultural-tower",
}

# Entidades que nunca se descartan del atlas, por ser las imprescindibles
# para representar una fabrica en VR.  El resto se descarta por prioridad
# cuando el atlas se llena.
PROTECTED_ENTITIES = {
    # las que el motor VR necesita siempre
    "assembling-machine-1", "burner-mining-drill", "electric-mining-drill",
    "stone-furnace", "transport-belt", "inserter", "small-electric-pole",
    "iron-chest", "gun-turret", "radar", "roboport",
    # y las mas usadas y pequenas de una fabrica
    "assembling-machine-2", "fast-inserter", "bulk-inserter",
    "burner-inserter", "steel-furnace", "electric-furnace", "steel-chest",
    "big-electric-pole", "boiler", "storage-tank", "oil-refinery",
    "solar-panel", "substation", "power-switch", "electric-energy-interface",
    "beacon", "heat-exchanger", "logistic-chest-passive-provider",
    "logistic-chest-active-provider", "logistic-chest-buffer",
}


# Lado maximo de un bloque de atlas.  Los bloques mas pequenos dejan al
# empaquetador llenar las paginas mucho mejor.
BLOCK_SIZE = 256

# Tipos de entidad que el motor VR dibuja realmente (los que le envia el
# juego).  Con --only-listed el atlas se reduce a lo que hace falta.
LISTED_ENTITY_NAMES = [
    # la maquinaria basica de una fabrica
    "assembling-machine-1", "assembling-machine-2", "assembling-machine-3",
    "burner-mining-drill", "electric-mining-drill",
    "stone-furnace", "steel-furnace", "electric-furnace",
    "transport-belt", "fast-transport-belt", "express-transport-belt",
    "inserter", "fast-inserter",
    "small-electric-pole", "medium-electric-pole", "big-electric-pole",
    "iron-chest", "steel-chest",
    "gun-turret", "radar", "roboport",
    "splitter", "underground-belt", "substation",
    # los recursos
    "iron-ore", "copper-ore", "coal", "uranium-ore", "stone", "crude-oil",
]

# Tipos que se incluyen enteros (todos sus variantes)
LISTED_ENTITY_TYPES = [
    "tree",
]

LISTED_ENTITY_TYPES_ALL = LISTED_ENTITY_NAMES + LISTED_ENTITY_TYPES


def _avg2(a, b):
    """Promedio de dos cadenas de bytes, byte a byte, sin acarreo entre bytes.

    Se hace con aritmetica de entero grande (SWAR): se separan los nibbles
    altos y bajos para que ninguna suma intermediate desborde un byte, de
    modo que el resultado es exactamente (a_i + b_i) // 2 en cada byte."""
    n = len(a)
    if n == 0:
        return b""
    m0f = int.from_bytes(b"\x0f" * n, "little")
    m1f = int.from_bytes(b"\x1f" * n, "little")
    m7f = int.from_bytes(b"\x7f" * n, "little")
    mf8 = int.from_bytes(b"\xf8" * n, "little")
    x = int.from_bytes(a, "little")
    y = int.from_bytes(b, "little")
    high = ((x >> 4) & m0f) + ((y >> 4) & m0f)      # <= 30 por byte
    low = (x & m0f) + (y & m0f)                    # <= 30 por byte
    # (a+b)//2 == 8*high + low//2   (las dos partes caben en un byte)
    hi = ((high & m1f) << 3) & mf8
    lo = (low >> 1) & m7f
    return (hi + lo).to_bytes(n, "little")


def downscale_2x2(row0, row1, dest_width):
    """Reduce una fila RGBA a la mitad con filtro de caja 2x2."""
    out = bytearray(dest_width * 4)
    for c in range(4):
        even = row0[c::8]
        odd = row0[4 + c::8]
        top = _avg2(even, odd)
        if row1 is not None:
            merged = _avg2(top, _avg2(row1[c::8], row1[4 + c::8]))
        else:
            merged = top
        out[c::4] = merged[:dest_width]
    return bytes(out)


def downscale_nearest(row, dest_width, factor):
    """Reduce una fila RGBA tomando un pixel de cada `factor`."""
    out = bytearray(dest_width * 4)
    stride = 4 * factor
    for c in range(4):
        out[c::4] = row[c::stride][:dest_width]
    return bytes(out)


def resample_block_generic(rows, src_x0, src_y0, src_w, src_h, dest_w, dest_h):
    """Filtro de caja general: promedia los pixeles origen de cada rect.

    Es la version correcta para cuando el frame no se puede reducir justo a
    la mitad (anchos impares, escalas que no dividen).  Es mas lenta que el
    camino rapido 2x2 pero solo se usa en esas capas."""
    out = []
    base = src_x0 * 4
    for r in range(dest_h):
        r0 = src_y0 + (r * src_h) // dest_h
        r1 = src_y0 + ((r + 1) * src_h) // dest_h
        if r1 <= r0:
            r1 = r0 + 1
        acc = [0] * (dest_w * 4)
        counts = [0] * dest_w
        for sy in range(r0, r1):
            row = rows.get(sy)
            if row is None:
                continue
            for c in range(dest_w):
                c0 = (c * src_w) // dest_w
                c1 = ((c + 1) * src_w) // dest_w
                if c1 <= c0:
                    c1 = c0 + 1
                counts[c] += c1 - c0
                for k in range(4):
                    acc[c * 4 + k] += sum(
                        row[base + c0 * 4 + k:base + c1 * 4 + k:4])
        for c in range(dest_w):
            if counts[c]:
                for k in range(4):
                    acc[c * 4 + k] //= counts[c]
        out.append(bytes(acc))
    return out


class ExtractError(Exception):
    """Error fatal del extractor."""


# ===========================================================================
# 1. Tokenizador de Lua
# ===========================================================================

LUA_KEYWORDS = {
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function",
    "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then",
    "true", "until", "while",
}

# Ordenados de mas largo a mas corto para que el longest-match funcione.
LUA_OPERATORS = [
    "...", "..", "::", "==", "~=", "<=", ">=", "<<", ">>", "//",
    "+", "-", "*", "/", "%", "^", "#", "&", "~", "|", "<", ">", "=",
    "(", ")", "{", "}", "[", "]", ";", ":", ",", ".",
]

_NAME_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_NUMBER_RE = re.compile(
    r"0[xX][0-9a-fA-F]+"
    r"|(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?"
)


class Token(object):
    __slots__ = ("kind", "value", "line")

    def __init__(self, kind, value, line):
        self.kind = kind        # name | number | string | op | eof
        self.value = value
        self.line = line

    def __repr__(self):  # pragma: no cover - solo depuracion
        return "Token(%s, %r, line=%d)" % (self.kind, self.value, self.line)


class LuaSyntaxError(Exception):
    pass


def _read_long_bracket(src, i):
    """Lee un bracket largo [[...]] / [=[...]=].  Devuelve (texto, indice_siguiente)."""
    start = i
    if src[i] != "[":
        raise LuaSyntaxError("long bracket expected")
    i += 1
    level = 0
    while i < len(src) and src[i] == "=":
        level += 1
        i += 1
    if i >= len(src) or src[i] != "[":
        raise LuaSyntaxError("long bracket malformed at %d" % start)
    i += 1
    if i < len(src) and src[i] == "\n":     # salta el primer salto de linea
        i += 1
    close = "]" + "=" * level + "]"
    end = src.find(close, i)
    if end < 0:
        raise LuaSyntaxError("unterminated long bracket at %d" % start)
    return src[i:end], end + len(close)


def _read_quoted(src, i):
    quote = src[i]
    i += 1
    out = []
    while i < len(src):
        c = src[i]
        if c == "\\":
            i += 1
            if i >= len(src):
                break
            e = src[i]
            mapping = {
                "n": "\n", "t": "\t", "r": "\r", "a": "\a", "b": "\b",
                "f": "\f", "v": "\v", "\\": "\\", '"': '"', "'": "'", "\n": "\n",
            }
            if e in mapping:
                out.append(mapping[e])
                i += 1
            elif e == "x":
                hexs = src[i + 1:i + 3]
                try:
                    out.append(chr(int(hexs, 16)))
                except ValueError:
                    out.append("x")
                i += 3
            elif e.isdigit():
                j = i
                while j < len(src) and j < i + 3 and src[j].isdigit():
                    j += 1
                try:
                    out.append(chr(int(src[i:j], 10)))
                except ValueError:
                    pass
                i = j
            elif e == "z":
                i += 1
                while i < len(src) and src[i] in " \t\r\n":
                    i += 1
            else:
                out.append(e)
                i += 1
            continue
        if c == quote:
            return "".join(out), i + 1
        out.append(c)
        i += 1
    raise LuaSyntaxError("unterminated string at %d" % i)


def tokenize(src):
    """Convierte codigo Lua en una lista de tokens."""
    tokens = []
    i = 0
    line = 1
    n = len(src)
    while i < n:
        c = src[i]
        if c == "\n":
            line += 1
            i += 1
            continue
        if c in " \t\r\f\v":
            i += 1
            continue
        # comentarios
        if src.startswith("--", i):
            i += 2
            if i < n and src[i] == "[":
                j = i + 1
                level = 0
                while j < n and src[j] == "=":
                    level += 1
                    j += 1
                if j < n and src[j] == "[":
                    _, i = _read_long_bracket(src, i)
                    line += 1
                    continue
            while i < n and src[i] != "\n":
                i += 1
            continue
        # cadenas largas
        if c == "[" and i + 1 < n and (src[i + 1] == "[" or src[i + 1] == "="):
            j = i + 1
            level = 0
            while j < n and src[j] == "=":
                level += 1
                j += 1
            if j < n and src[j] == "[":
                text, i = _read_long_bracket(src, i)
                tokens.append(Token("string", text, line))
                line += src.count("\n", 0, 0)  # no-op, las lineas ya contadas
                continue
        # cadenas normales
        if c in "\"'":
            text, i = _read_quoted(src, i)
            tokens.append(Token("string", text, line))
            continue
        # numeros
        if c.isdigit() or (c == "." and i + 1 < n and src[i + 1].isdigit()):
            m = _NUMBER_RE.match(src, i)
            if m:
                raw = m.group(0)
                i = m.end()
                if raw.lower().startswith("0x"):
                    tokens.append(Token("number", float(int(raw, 16)), line))
                else:
                    if "." in raw or "e" in raw.lower():
                        tokens.append(Token("number", float(raw), line))
                    else:
                        tokens.append(Token("number", int(raw), line))
                continue
        # nombres
        if c.isalpha() or c == "_":
            m = _NAME_RE.match(src, i)
            name = m.group(0)
            i = m.end()
            tokens.append(Token("name", name, line))
            continue
        # operadores
        for op in LUA_OPERATORS:
            if src.startswith(op, i):
                tokens.append(Token("op", op, line))
                i += len(op)
                break
        else:
            raise LuaSyntaxError("unexpected character %r at line %d" % (c, line))
    tokens.append(Token("eof", None, line))
    return tokens


# ===========================================================================
# 2. AST
# ===========================================================================

class Node(object):
    __slots__ = ()


class TConst(Node):
    __slots__ = ("value",)

    def __init__(self, value):
        self.value = value


class TVar(Node):
    __slots__ = ("name",)

    def __init__(self, name):
        self.name = name


class TIndex(Node):
    __slots__ = ("obj", "key")

    def __init__(self, obj, key):
        self.obj = obj
        self.key = key


class TCall(Node):
    __slots__ = ("func", "args", "method")

    def __init__(self, func, args, method=None):
        self.func = func
        self.args = args
        self.method = method


class TFunc(Node):
    __slots__ = ("params", "body", "env")

    def __init__(self, params, body):
        self.params = params
        self.body = body
        self.env = None


class TBin(Node):
    __slots__ = ("op", "left", "right")

    def __init__(self, op, left, right):
        self.op = op
        self.left = left
        self.right = right


class TUn(Node):
    __slots__ = ("op", "operand")

    def __init__(self, op, operand):
        self.op = op
        self.operand = operand


class TTable(Node):
    __slots__ = ("entries",)

    def __init__(self, entries):
        self.entries = entries      # lista de (key_expr|None, value_expr)


class TLocal(Node):
    __slots__ = ("names", "exprs")

    def __init__(self, names, exprs):
        self.names = names
        self.exprs = exprs


class TAssign(Node):
    __slots__ = ("targets", "exprs")

    def __init__(self, targets, exprs):
        self.targets = targets
        self.exprs = exprs


class TIf(Node):
    __slots__ = ("clauses", "orelse")

    def __init__(self, clauses, orelse):
        self.clauses = clauses        # [(cond, body)]
        self.orelse = orelse


class TLoop(Node):
    """for / while / do genericos."""
    __slots__ = ("kind", "vars", "cond", "body", "step")

    def __init__(self, kind, body, vars=None, cond=None, step=None):
        self.kind = kind
        self.body = body
        self.vars = vars
        self.cond = cond
        self.step = step


class TReturn(Node):
    __slots__ = ("exprs",)

    def __init__(self, exprs):
        self.exprs = exprs


class TExprStat(Node):
    __slots__ = ("expr",)

    def __init__(self, expr):
        self.expr = expr


class TFuncDef(Node):
    __slots__ = ("name", "params", "body", "is_method", "is_local")

    def __init__(self, name, params, body, is_method, is_local):
        self.name = name
        self.params = params
        self.body = body
        self.is_method = is_method
        self.is_local = is_local


class TBreak(Node):
    __slots__ = ()


class TGoto(Node):
    __slots__ = ("label",)

    def __init__(self, label):
        self.label = label


# ===========================================================================
# 3. Parser de Lua (subconjunto)
# ===========================================================================

_BIN_PRIORITY = {
    "or": (1, 1),
    "and": (2, 2),
    "<": (3, 3), ">": (3, 3), "<=": (3, 3), ">=": (3, 3), "~=": (3, 3), "==": (3, 3),
    "|": (4, 4), "~": (5, 5), "&": (6, 6), "<<": (7, 7), ">>": (7, 7),
    "..": (8, 8),  # asociativo por la derecha
    "+": (9, 9), "-": (9, 9),
    "*": (10, 10), "/": (10, 10), "//": (10, 10), "%": (10, 10),
    "^": (12, 12),  # asociativo por la derecha, mas fuerte que unario
}
_UNARY_PRIORITY = 11
_RIGHT_ASSOC = {"..", "^"}


class LuaParser(object):
    def __init__(self, tokens, filename="<lua>"):
        self.toks = tokens
        self.pos = 0
        self.filename = filename

    # -- utilidades ------------------------------------------------------
    def peek(self, offset=0):
        idx = self.pos + offset
        if idx < 0:
            return self.toks[0]
        if idx >= len(self.toks):
            return self.toks[-1]
        return self.toks[idx]

    def next(self):
        tok = self.peek()
        self.pos += 1
        return tok

    def check(self, kind, value=None):
        tok = self.peek()
        if tok.kind != kind:
            return False
        if value is not None and tok.value != value:
            return False
        return True

    def check_op(self, *values):
        tok = self.peek()
        return tok.kind == "op" and tok.value in values

    def check_name(self, *values):
        tok = self.peek()
        return tok.kind == "name" and tok.value in values

    def accept_op(self, value):
        if self.check_op(value):
            self.pos += 1
            return True
        return False

    def accept_name(self, value):
        if self.check_name(value):
            self.pos += 1
            return True
        return False

    def expect_kw(self, value):
        if not self.accept_name(value):
            tok = self.peek()
            raise LuaSyntaxError("%s:%d: expected %r, got %r" %
                                 (self.filename, tok.line, value, tok.value))

    def expect_op(self, value):
        if not self.accept_op(value):
            tok = self.peek()
            raise LuaSyntaxError("%s:%d: expected %r, got %r" %
                                 (self.filename, tok.line, value, tok.value))

    def expect_name(self):
        tok = self.peek()
        if tok.kind != "name":
            raise LuaSyntaxError("%s:%d: expected name, got %r" %
                                 (self.filename, tok.line, tok.value))
        self.pos += 1
        return tok.value

    def error(self, msg):
        tok = self.peek()
        raise LuaSyntaxError("%s:%d: %s (got %r)" %
                             (self.filename, tok.line, msg, tok.value))

    # -- bloque ----------------------------------------------------------
    def parse_chunk(self):
        body = []
        while not self.check("eof"):
            stmt = self.parse_statement()
            if stmt is not None:
                body.append(stmt)
            if self.accept_op(";"):
                continue
        return body

    BLOCK_ENDERS = ("end", "else", "elseif", "until")

    def parse_block(self):
        body = []
        while True:
            tok = self.peek()
            if tok.kind == "eof":
                break
            if tok.kind == "name" and tok.value in self.BLOCK_ENDERS:
                break
            if tok.kind == "op" and tok.value == ";":
                self.pos += 1
                continue
            stmt = self.parse_statement()
            if stmt is not None:
                body.append(stmt)
        return body

    # -- sentencias ------------------------------------------------------
    def parse_statement(self):
        tok = self.peek()
        if tok.kind == "name":
            kw = tok.value
            if kw == "local":
                return self.parse_local()
            if kw == "function":
                return self.parse_function_stat()
            if kw == "if":
                return self.parse_if()
            if kw == "for":
                return self.parse_for()
            if kw == "while":
                return self.parse_while()
            if kw == "do":
                self.pos += 1
                body = self.parse_block()
                self.expect_kw("end")
                return TLoop("do", body)
            if kw == "return":
                self.pos += 1
                exprs = []
                if not (self.check("eof") or
                        (self.peek().kind == "name" and self.peek().value in self.BLOCK_ENDERS) or
                        self.check_op(";")):
                    exprs = self.parse_exprlist()
                self.accept_op(";")
                return TReturn(exprs)
            if kw == "break":
                self.pos += 1
                return TBreak()
            if kw == "goto":
                self.pos += 1
                return TGoto(self.expect_name())
            if kw == "repeat":
                self.pos += 1
                body = self.parse_block()
                self.expect_kw("until")
                self.parse_expr()
                return TLoop("repeat", body)
        if tok.kind == "op" and tok.value == ";":
            self.pos += 1
            return None
        if tok.kind == "op" and tok.value == "::":
            self.pos += 1
            self.expect_name()
            self.expect_op("::")
            return None
        return self.parse_expr_statement()

    def parse_local(self):
        self.pos += 1                      # 'local'
        if self.check_name("function"):
            self.pos += 1
            name = self.expect_name()
            params, body = self.parse_func_body()
            return TFuncDef(name, params, body, False, True)
        names = [self.expect_name()]
        while self.accept_op(","):
            names.append(self.expect_name())
        exprs = []
        if self.accept_op("="):
            exprs = self.parse_exprlist()
        return TLocal(names, exprs)

    def parse_function_stat(self):
        self.pos += 1                      # 'function'
        name = self.expect_name()
        is_method = False
        full = name
        while self.check_op(".") or self.check_op(":"):
            sep = self.next().value
            part = self.expect_name()
            full = full + sep + part
            if sep == ":":
                is_method = True
                break
        params, body = self.parse_func_body()
        return TFuncDef(full, params, body, is_method, False)

    def parse_func_body(self):
        self.expect_op("(")
        params = []
        if not self.check_op(")"):
            while True:
                if self.check_op("..."):
                    self.pos += 1
                    params.append("...")
                    break
                params.append(self.expect_name())
                if not self.accept_op(","):
                    break
        self.expect_op(")")
        body = self.parse_block()
        self.expect_kw("end")
        return params, body

    def parse_if(self):
        self.pos += 1                      # 'if'
        clauses = []
        cond = self.parse_expr()
        self.expect_kw("then")
        clauses.append((cond, self.parse_block()))
        orelse = None
        while self.check_name("elseif"):
            self.pos += 1
            c = self.parse_expr()
            self.expect_kw("then")
            clauses.append((c, self.parse_block()))
        if self.accept_name("else"):
            orelse = self.parse_block()
        self.expect_kw("end")
        return TIf(clauses, orelse)

    def parse_for(self):
        self.pos += 1                      # 'for'
        names = [self.expect_name()]
        while self.accept_op(","):
            names.append(self.expect_name())
        start = stop = step = None
        if self.accept_op("="):
            start = self.parse_expr()
            self.expect_op(",")
            stop = self.parse_expr()
            if self.accept_op(","):
                step = self.parse_expr()
        elif self.accept_name("in"):
            # for k, v in explist do ... end
            self.parse_exprlist()
        self.expect_kw("do")
        body = self.parse_block()
        self.expect_kw("end")
        return TLoop("for", body, vars=names, cond=(start, stop, step))

    def parse_while(self):
        self.pos += 1                      # 'while'
        cond = self.parse_expr()
        self.expect_kw("do")
        body = self.parse_block()
        self.expect_kw("end")
        return TLoop("while", body, cond=cond)

    def parse_expr_statement(self):
        first = self.parse_suffixedexp()
        if self.check_op("=") or self.check_op(","):
            targets = [first]
            while self.accept_op(","):
                targets.append(self.parse_suffixedexp())
            self.expect_op("=")
            exprs = self.parse_exprlist()
            return TAssign(targets, exprs)
        return TExprStat(first)

    # -- expresiones -----------------------------------------------------
    def parse_exprlist(self):
        exprs = [self.parse_expr()]
        while self.accept_op(","):
            exprs.append(self.parse_expr())
        return exprs

    def parse_expr(self, limit=0):
        tok = self.peek()
        if (tok.kind == "op" and tok.value in ("-", "#", "~")) or \
           (tok.kind == "name" and tok.value == "not"):
            op = self.next().value
            operand = self.parse_expr(_UNARY_PRIORITY)
            left = TUn(op, operand)
        elif tok.kind == "name" and tok.value in ("nil", "true", "false", "function"):
            left = self.parse_simpleexp()
        elif (tok.kind == "name") or (tok.kind == "op" and tok.value == "("):
            # expresion con sufijos: a.b, a[1], f(x), f{...}, a:m(x)
            left = self.parse_suffixedexp()
        else:
            left = self.parse_simpleexp()
        while True:
            tok = self.peek()
            op = None
            if tok.kind == "op" and tok.value in _BIN_PRIORITY:
                op = tok.value
            elif tok.kind == "name" and tok.value in ("and", "or"):
                op = tok.value
            if op is None:
                break
            left_prio, right_prio = _BIN_PRIORITY[op]
            if left_prio <= limit:
                break
            self.pos += 1
            right = self.parse_expr(right_prio if op in _RIGHT_ASSOC else left_prio)
            left = TBin(op, left, right)
        return left

    def parse_simpleexp(self):
        tok = self.peek()
        if tok.kind == "number":
            self.pos += 1
            return TConst(tok.value)
        if tok.kind == "string":
            self.pos += 1
            return TConst(tok.value)
        if tok.kind == "name":
            if tok.value == "nil":
                self.pos += 1
                return TConst(None)
            if tok.value == "true":
                self.pos += 1
                return TConst(True)
            if tok.value == "false":
                self.pos += 1
                return TConst(False)
            if tok.value == "function":
                self.pos += 1
                params, body = self.parse_func_body()
                return TFunc(params, body)
            self.pos += 1
            return TVar(tok.value)
        if tok.kind == "op":
            if tok.value == "...":
                self.pos += 1
                return TVar("...")
            if tok.value == "{":
                return self.parse_table()
            if tok.value == "(":
                self.pos += 1
                inner = self.parse_expr()
                self.expect_op(")")
                return inner
        self.error("unexpected token in expression")

    def parse_primaryexp(self):
        tok = self.peek()
        if tok.kind == "name":
            self.pos += 1
            return TVar(tok.value)
        if tok.kind == "op" and tok.value == "(":
            self.pos += 1
            inner = self.parse_expr()
            self.expect_op(")")
            return inner
        self.error("unexpected symbol")

    def parse_suffixedexp(self):
        expr = self.parse_primaryexp()
        while True:
            tok = self.peek()
            if tok.kind == "op" and tok.value == ".":
                self.pos += 1
                key = self.expect_name()
                expr = TIndex(expr, TConst(key))
            elif tok.kind == "op" and tok.value == "[":
                self.pos += 1
                key = self.parse_expr()
                self.expect_op("]")
                expr = TIndex(expr, key)
            elif tok.kind == "op" and tok.value == ":":
                self.pos += 1
                mname = self.expect_name()
                args = self.parse_call_args()
                expr = TCall(expr, args, method=mname)
            elif tok.kind == "op" and tok.value == "(":
                args = self.parse_args()
                expr = TCall(expr, args)
            elif tok.kind == "op" and tok.value == "{":
                # llamada con tabla:  f{...}
                args = [self.parse_table()]
                expr = TCall(expr, args)
            elif tok.kind == "string" and self.same_line_as_prev():
                # llamada con cadena:  require "modulo"  /  f"texto"
                self.pos += 1
                expr = TCall(expr, [TConst(tok.value)])
            else:
                return expr

    def same_line_as_prev(self):
        """True si el token actual esta en la misma linea que el anterior."""
        if self.pos < 1:
            return True
        return self.peek().line == self.peek(-1).line

    def parse_call_args(self):
        """Argumentos de una llamada: con parentesis, con tabla o con cadena."""
        if self.check_op("("):
            return self.parse_args()
        if self.check_op("{"):
            return [self.parse_table()]
        if self.check("string"):
            tok = self.next()
            return [TConst(tok.value)]
        return []

    def parse_args(self):
        self.expect_op("(")
        args = []
        if not self.check_op(")"):
            args = self.parse_exprlist()
        self.expect_op(")")
        return args

    def parse_table(self):
        self.expect_op("{")
        entries = []
        while not self.check_op("}"):
            if self.check_op("["):
                self.pos += 1
                key = self.parse_expr()
                self.expect_op("]")
                self.expect_op("=")
                value = self.parse_expr()
                entries.append((key, value))
            elif (self.peek().kind == "name" and
                  self.peek(1).kind == "op" and self.peek(1).value == "="):
                key = TConst(self.expect_name())
                self.expect_op("=")
                value = self.parse_expr()
                entries.append((key, value))
            else:
                entries.append((None, self.parse_expr()))
            if not (self.accept_op(",") or self.accept_op(";")):
                break
        self.expect_op("}")
        return TTable(entries)


# ===========================================================================
# 4. Valores y evaluador
# ===========================================================================

class LuaTable(object):
    """Tabla Lua: parte array (1..n) + parte hash."""

    __slots__ = ("arr", "hash", "metatable")

    def __init__(self):
        self.arr = []
        self.hash = {}
        self.metatable = None

    def get(self, key):
        if isinstance(key, bool):
            return self.hash.get(key)
        if isinstance(key, (int, float)):
            i = int(key)
            if i == int(key) and 1 <= i <= len(self.arr):
                return self.arr[i - 1]
            return self.hash.get(key)
        return self.hash.get(key)

    def set(self, key, value):
        if isinstance(key, (int, float)) and not isinstance(key, bool):
            i = int(key)
            if i == key and i >= 1:
                if i <= len(self.arr):
                    self.arr[i - 1] = value
                elif i == len(self.arr) + 1:
                    self.arr.append(value)
                    self._migrate()
                else:
                    self.hash[key] = value
                return
        self.hash[key] = value

    def _migrate(self):
        nxt = len(self.arr) + 1
        while nxt in self.hash:
            self.arr.append(self.hash.pop(nxt))
            nxt += 1

    def length(self):
        return len(self.arr)

    def items(self):
        for i, v in enumerate(self.arr):
            yield i + 1, v
        for k, v in self.hash.items():
            yield k, v

    def keys(self):
        return [k for k, _ in self.items()]

    def to_plain(self):
        """Convierte a dict/list de Python (para JSON)."""
        if self.length() and len(self.hash) == 0:
            return [v.to_plain() if isinstance(v, LuaTable) else v for v in self.arr]
        out = {}
        for k, v in self.items():
            key = k if isinstance(k, (str, int, float, bool)) else str(k)
            out[key] = v.to_plain() if isinstance(v, LuaTable) else v
        return out


class LuaFunction(object):
    __slots__ = ("params", "body", "env", "name")

    def __init__(self, params, body, env, name="<anonymous>"):
        self.params = params
        self.body = body
        self.env = env
        self.name = name


class LuaOpaque(object):
    """Valor opaco: llamada a funcion desconocida.  Conserva los argumentos
    evaluados para que el recolector de sprites pueda entrar dentro."""

    __slots__ = ("func", "args")

    def __init__(self, func, args):
        self.func = func
        self.args = args


class Env(object):
    __slots__ = ("vars", "parent")

    def __init__(self, parent=None):
        self.vars = {}
        self.parent = parent

    def get(self, name):
        env = self
        while env is not None:
            if name in env.vars:
                return env.vars[name]
            env = env.parent
        return None

    def has(self, name):
        env = self
        while env is not None:
            if name in env.vars:
                return True
            env = env.parent
        return False

    def set(self, name, value):
        self.vars[name] = value

    def declare(self, name, value):
        self.vars[name] = value


class LuaRuntime(object):
    """Entorno de ejecucion con carga perezosa de modulos `require`."""

    def __init__(self, data_dir, warnings):
        self.data_dir = data_dir
        self.warnings = warnings
        self.modules = {}
        self.module_stack = []
        self.extended = []          # payloads de data:extend

    # -- resolucion de require ------------------------------------------
    def resolve_module_path(self, name, current_dir):
        name = name.strip()
        candidates = []
        rest = name
        for prefix in ("__base__/", "__core__/"):
            if rest.startswith(prefix):
                rest = rest[len(prefix):]
                candidates.append(os.path.join(self.data_dir, rest + ".lua"))
        for prefix in ("__base__.", "__core__."):
            if rest.startswith(prefix):
                rest = rest[len(prefix):]
                candidates.append(os.path.join(self.data_dir, rest.replace(".", "/") + ".lua"))
        if not name.startswith("__"):
            rest = name.replace(".", "/")
            candidates.append(os.path.join(self.data_dir, "base", rest + ".lua"))
            candidates.append(os.path.join(self.data_dir, "base", "prototypes", rest + ".lua"))
            candidates.append(os.path.join(self.data_dir, "core", rest + ".lua"))
            candidates.append(os.path.join(self.data_dir, "core", "prototypes", rest + ".lua"))
        if current_dir:
            candidates.append(os.path.join(current_dir, rest.replace(".", "/") + ".lua"))
            candidates.append(os.path.join(current_dir, name.replace(".", "/") + ".lua"))
        for cand in candidates:
            if os.path.isfile(cand):
                return os.path.normpath(cand)
        return None

    def load_module(self, name, current_dir):
        if name in self.modules:
            return self.modules[name]
        if name in self.module_stack:
            return LuaTable()          # ciclo: devolver tabla vacia
        path = self.resolve_module_path(name, current_dir)
        if path is None:
            return None
        self.module_stack.append(name)
        try:
            exports = self.execute_file(path)
        finally:
            self.module_stack.pop()
        self.modules[name] = exports
        return exports

    # -- ejecucion de ficheros -------------------------------------------
    def execute_file(self, path):
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            src = fh.read()
        try:
            tokens = tokenize(src)
            body = LuaParser(tokens, path).parse_chunk()
        except LuaSyntaxError as exc:
            self.warnings.append("lua-parse: %s (%s)" % (os.path.basename(path), exc))
            return LuaTable()
        env = Env()
        env.declare("__file__", path)
        result = self.exec_block(body, env, path)
        exports = LuaTable()
        if isinstance(result, LuaTable):
            for k, v in result.items():
                exports.set(k, v)
        # las globales de nivel superior tambien se exportan
        for k, v in env.vars.items():
            if k not in ("__file__",) and exports.get(k) is None:
                exports.set(k, v)
        return exports

    def exec_block(self, body, env, path):
        result = None
        for stmt in body:
            out = self.exec_stmt(stmt, env, path)
            if out is not None:
                result = out
        return result

    def exec_stmt(self, stmt, env, path):
        if isinstance(stmt, TLocal):
            values = [self.eval(e, env, path) for e in stmt.exprs]
            for i, name in enumerate(stmt.names):
                env.declare(name, values[i] if i < len(values) else None)
            return None
        if isinstance(stmt, TAssign):
            values = [self.eval(e, env, path) for e in stmt.exprs]
            for i, target in enumerate(stmt.targets):
                value = values[i] if i < len(values) else None
                self.assign(target, value, env, path)
            return None
        if isinstance(stmt, TFuncDef):
            func = LuaFunction(stmt.params, stmt.body, env, stmt.name)
            if stmt.is_local:
                env.declare(stmt.name, func)
            else:
                env.set(stmt.name, func)
            return None
        if isinstance(stmt, TIf):
            for cond, body in stmt.clauses:
                if truthy(self.eval(cond, env, path)):
                    return self.exec_block(body, env, path)
            if stmt.orelse is not None:
                return self.exec_block(stmt.orelse, env, path)
            return None
        if isinstance(stmt, TLoop):
            return self.exec_loop(stmt, env, path)
        if isinstance(stmt, TReturn):
            if not stmt.exprs:
                return None
            if len(stmt.exprs) == 1:
                return self.eval(stmt.exprs[0], env, path)
            return LuaTable()
        if isinstance(stmt, TExprStat):
            self.eval(stmt.expr, env, path)
            return None
        if isinstance(stmt, (TBreak, TGoto)):
            return None
        return None

    def exec_loop(self, stmt, env, path):
        if stmt.kind == "for":
            names = stmt.vars
            start, stop, step = stmt.cond
            i = self.eval(start, env, path) if start is not None else 1
            limit = self.eval(stop, env, path) if stop is not None else 0
            inc = self.eval(step, env, path) if step is not None else 1
            if not isinstance(i, (int, float)) or not isinstance(limit, (int, float)):
                return None
            if not isinstance(inc, (int, float)) or inc == 0:
                return None
            guard = 0
            while (inc > 0 and i <= limit) or (inc < 0 and i >= limit):
                guard += 1
                if guard > 100000:
                    break
                for n, name in enumerate(names):
                    env.declare(name, i if n == 0 else None)
                self.exec_block(stmt.body, env, path)
                i += inc
            return None
        if stmt.kind == "while":
            guard = 0
            while truthy(self.eval(stmt.cond, env, path)):
                guard += 1
                if guard > 100000:
                    break
                self.exec_block(stmt.body, env, path)
            return None
        if stmt.kind == "do":
            return self.exec_block(stmt.body, env, path)
        return None

    def assign(self, target, value, env, path):
        if isinstance(target, TVar):
            env.set(target.name, value)
            return
        if isinstance(target, TIndex):
            obj = self.eval(target.obj, env, path)
            key = self.eval(target.key, env, path)
            if isinstance(obj, LuaTable):
                obj.set(key, value)
            return

    # -- evaluacion ------------------------------------------------------
    def eval(self, node, env, path):
        if node is None:
            return None
        cls = node.__class__
        if cls is TConst:
            return node.value
        if cls is TVar:
            name = node.name
            if name == "...":
                return None
            return env.get(name)
        if cls is TIndex:
            obj = self.eval(node.obj, env, path)
            key = self.eval(node.key, env, path)
            if isinstance(obj, LuaTable):
                return obj.get(key)
            if isinstance(obj, LuaOpaque):
                return None
            return None
        if cls is TTable:
            table = LuaTable()
            idx = 1
            for key_expr, value_expr in node.entries:
                value = self.eval(value_expr, env, path)
                if key_expr is None:
                    table.set(idx, value)
                    idx += 1
                else:
                    key = self.eval(key_expr, env, path)
                    table.set(key, value)
            return table
        if cls is TBin:
            return self.eval_bin(node, env, path)
        if cls is TUn:
            operand = self.eval(node.operand, env, path)
            if node.op == "-":
                if isinstance(operand, (int, float)) and not isinstance(operand, bool):
                    return -operand
                return None
            if node.op == "not":
                return not truthy(operand)
            if node.op == "#":
                if isinstance(operand, LuaTable):
                    return operand.length()
                if isinstance(operand, str):
                    return len(operand)
                return None
            return None
        if cls is TFunc:
            return LuaFunction(node.params, node.body, env)
        if cls is TCall:
            return self.eval_call(node, env, path)
        return None

    def eval_bin(self, node, env, path):
        op = node.op
        left = self.eval(node.left, env, path)
        if op == "and":
            if not truthy(left):
                return left
            return self.eval(node.right, env, path)
        if op == "or":
            if truthy(left):
                return left
            return self.eval(node.right, env, path)
        right = self.eval(node.right, env, path)
        if op == "..":
            if isinstance(left, str) and isinstance(right, str):
                return left + right
            if isinstance(left, (int, float)) and isinstance(right, str):
                return lua_tostring(left) + right
            if isinstance(left, str) and isinstance(right, (int, float)):
                return left + lua_tostring(right)
            return None
        if op in ("==", "~="):
            same = lua_equal(left, right)
            return same if op == "==" else (not same)
        if op in ("<", ">", "<=", ">="):
            if isinstance(left, (int, float)) and isinstance(right, (int, float)):
                if op == "<":
                    return left < right
                if op == ">":
                    return left > right
                if op == "<=":
                    return left <= right
                return left >= right
            if isinstance(left, str) and isinstance(right, str):
                if op == "<":
                    return left < right
                if op == ">":
                    return left > right
                if op == "<=":
                    return left <= right
                return left >= right
            return None
        if isinstance(left, (int, float)) and isinstance(right, (int, float)):
            if op == "+":
                return left + right
            if op == "-":
                return left - right
            if op == "*":
                return left * right
            if op == "/":
                return left / right if right != 0 else None
            if op == "//":
                return left // right if right != 0 else None
            if op == "%":
                return left % right if right != 0 else None
            if op == "^":
                return left ** right
            if op == "&":
                return int(left) & int(right)
            if op == "|":
                return int(left) | int(right)
            if op == "~":
                return int(left) ^ int(right)
            if op == "<<":
                return int(left) << int(right)
            if op == ">>":
                return int(left) >> int(right)
        return None

    def eval_call(self, node, env, path):
        method = node.method
        call_name = self.call_name(node.func)

        # caso especial: data:extend({ ... })  ->  registra la carga de prototipos
        if method == "extend" and call_name in ("data", "self.data"):
            args = [self.eval(a, env, path) for a in node.args]
            if args and isinstance(args[0], LuaTable):
                self.extended.append(args[0])
            return None
        if call_name == "data.extend":
            args = [self.eval(a, env, path) for a in node.args]
            if args and isinstance(args[0], LuaTable):
                self.extended.append(args[0])
            return None

        func = self.eval(node.func, env, path)
        if method is not None:
            args = [self.eval(a, env, path) for a in node.args]
            if isinstance(func, LuaTable):
                name = func.get(method)
                if isinstance(name, LuaFunction):
                    return self.call_function(name, args, path)
            return LuaOpaque(method, args)

        # caso especial: require("modulo")
        name = call_name
        if name == "require" and node.args:
            mod_name = self.eval(node.args[0], env, path)
            if isinstance(mod_name, str):
                mod = self.load_module(mod_name, os.path.dirname(path))
                if mod is not None:
                    return mod
                return LuaTable()
            return LuaTable()

        args = [self.eval(a, env, path) for a in node.args]

        # caso especial: util.sprite_load("ruta", { ... })
        if name in ("util.sprite_load", "sprite_load", "util.sprite"):
            return self.make_sprite_load(args)

        # caso especial: util.by_pixel(x, y)  ->  {x, y}
        if name in ("util.by_pixel", "by_pixel"):
            table = LuaTable()
            vals = [a for a in args if isinstance(a, (int, float)) and not isinstance(a, bool)]
            if len(vals) >= 2:
                table.set(1, vals[0])
                table.set(2, vals[1])
            return table

        if isinstance(func, LuaFunction):
            return self.call_function(func, args, path)
        if isinstance(func, LuaTable) and func.metatable is not None:
            return self.call_function(func.metatable, args, path)
        return LuaOpaque(name or "?", args)

    def call_name(self, node):
        """Nombre (punteado) de la funcion llamada, si se puede determinar."""
        if isinstance(node, TVar):
            return node.name
        if isinstance(node, TIndex):
            base = self.call_name(node.obj)
            key = node.key
            if base and isinstance(key, TConst) and isinstance(key.value, str):
                return base + "." + key.value
        return None

    def call_function(self, func, args, path):
        env = Env(func.env)
        for i, param in enumerate(func.params):
            env.declare(param, args[i] if i < len(args) else None)
        if "..." in func.params:
            env.declare("...", LuaTable())
        try:
            return self.exec_block(func.body, env, path)
        except RecursionError:
            return None
        except LuaSyntaxError:
            return None

    def make_sprite_load(self, args):
        """util.sprite_load("ruta/strip", { props }) -> dict de capa."""
        layer = LuaTable()
        if not args:
            return layer
        path = args[0]
        if not isinstance(path, str):
            return layer
        layer.set("filename", path + ".png")
        layer.set("strip_path", path)
        if len(args) > 1 and isinstance(args[1], LuaTable):
            for k, v in args[1].items():
                layer.set(k, v)
        return layer


def truthy(value):
    return not (value is None or value is False)


def lua_equal(left, right):
    if left is None and right is None:
        return True
    if isinstance(left, bool) or isinstance(right, bool):
        return left is right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return left == right
    if isinstance(left, str) and isinstance(right, str):
        return left == right
    return left is right


def lua_tostring(value):
    if isinstance(value, float) and value == int(value):
        return str(int(value))
    return str(value)


# ===========================================================================
# 5. Recoleccion de prototipos
# ===========================================================================

def _looks_like_prototype(value):
    if not isinstance(value, LuaTable):
        return False
    ptype = value.get("type")
    pname = value.get("name")
    return (isinstance(ptype, str) and isinstance(pname, str) and pname != "")


def collect_prototypes(runtime, proto_dir, warnings):
    """Recorre todos los .lua del directorio de prototipos y devuelve la lista
    de prototipos de entidad deduplicada por (type, name)."""
    found = []
    seen = {}
    if not os.path.isdir(proto_dir):
        return found
    lua_files = []
    for dirpath, _dirnames, filenames in os.walk(proto_dir):
        for filename in sorted(filenames):
            if filename.endswith(".lua"):
                lua_files.append(os.path.join(dirpath, filename))
    lua_files.sort()

    for path in lua_files:
        before = len(runtime.extended)
        exports = runtime.execute_file(path)
        # 1) prototos añadidos con data:extend(...)
        for payload in runtime.extended[before:]:
            for proto in iter_prototypes(payload):
                _add_proto(found, seen, proto)
        # 2) prototipos alcanzables desde las variables de nivel superior
        for value in list(exports.items()):
            for proto in iter_prototypes(value[1]):
                _add_proto(found, seen, proto)
    runtime.extended = []
    return found


def _add_proto(found, seen, proto):
    key = (proto.get("type"), proto.get("name"))
    if key in seen:
        # conserva el que tenga mas capas de sprite
        old = seen[key]
        if count_layers(proto) > count_layers(old):
            found[found.index(old)] = proto
            seen[key] = proto
        return
    seen[key] = proto
    found.append(proto)


def iter_prototypes(value, depth=0):
    """Genera los dicts con type+name alcanzables desde `value`."""
    if depth > 12:
        return
    if isinstance(value, LuaTable):
        if _looks_like_prototype(value):
            yield value
            return
        for _k, v in value.items():
            for proto in iter_prototypes(v, depth + 1):
                yield proto
    elif isinstance(value, LuaOpaque):
        for a in value.args:
            for proto in iter_prototypes(a, depth + 1):
                yield proto


def count_layers(value, depth=0):
    if depth > 12:
        return 0
    total = 0
    if isinstance(value, LuaTable):
        if is_layer_table(value):
            total += 1
        for _k, v in value.items():
            total += count_layers(v, depth + 1)
    elif isinstance(value, LuaOpaque):
        for a in value.args:
            total += count_layers(a, depth + 1)
    return total


def is_layer_table(table):
    filename = table.get("filename")
    return isinstance(filename, str) and filename.lower().endswith(".png")


def _layer_score(table):
    """Puntuacion de una variante de capa: cuantos frames aporta.

    Se usa para elegir, entre varias capas que apuntan al mismo PNG, la que
    tiene la animacion mas completa."""
    score = 0
    for key in ("frame_count", "width_in_frames", "repeat_count"):
        value = _as_int(table.get(key)) or 0
        if value > score:
            score = value
    frames = table.get("frames")
    if isinstance(frames, LuaTable):
        value = _as_int(frames.get("count")) or 0
        if value > score:
            score = value
    return score


# ===========================================================================
# 6. Extraccion de capas de sprite
# ===========================================================================

ICON_PATH_MARKER = "/graphics/icons/"


def extract_layers(proto, warnings):
    """Devuelve la lista de capas de sprite de un prototipo, en orden."""
    layers = []
    _walk_layers(proto, layers, warnings, set(), ())
    return layers


# Contenedores de graficos "principales" de un prototipo: la capa que vive
# dentro de ellos es la que define el cuerpo de la entidad.
MAIN_GRAPHICS_KEYS = ("graphics_set", "pictures", "sprite", "sprites")


def _walk_layers(value, out, warnings, visited, path, depth=0):
    if depth > 14:
        return
    if isinstance(value, LuaTable):
        if id(value) in visited:
            return
        visited.add(id(value))
        if is_layer_table(value):
            filename = value.get("filename")
            if ICON_PATH_MARKER not in filename.replace("\\", "/"):
                out.append((value, path))
                return
        for key, v in value.items():
            sub = path + (key,) if isinstance(key, str) else path + ("[]",)
            _walk_layers(v, out, warnings, visited, sub, depth + 1)
    elif isinstance(value, LuaOpaque):
        for a in value.args:
            _walk_layers(a, out, warnings, visited, path, depth + 1)


# ===========================================================================
# 7. Geometria de frames
# ===========================================================================

class LayerGeometry(object):
    __slots__ = ("frame_width", "frame_height", "cols", "rows", "cells",
                 "frame_count", "direction_count", "anim_frames", "anim_speed",
                 "offset_x", "offset_y", "strip_path", "sheet_width",
                 "sheet_height", "grid_mismatch", "line_length")

    def __init__(self):
        self.frame_width = 0
        self.frame_height = 0
        self.cols = 1
        self.rows = 1
        self.cells = 1
        self.frame_count = 1
        self.direction_count = 1
        self.anim_frames = 0
        self.anim_speed = 0.0
        self.offset_x = 0
        self.offset_y = 0
        self.strip_path = None
        self.sheet_width = 0
        self.sheet_height = 0
        self.grid_mismatch = False
        self.line_length = 0

    @property
    def frame_pixels(self):
        return self.frame_width * self.frame_height


def read_strip_definition(strip_path, data_dir):
    """Lee el .lua de definicion de tira de Factorio 2.1 (width/height/...)."""
    if not strip_path:
        return None
    rel = strip_path
    for prefix, repl in (("__base__/", "base/"), ("__core__/", "core/")):
        if rel.startswith(prefix):
            rel = repl + rel[len(prefix):]
            break
    else:
        rel = rel.lstrip("/")
    lua_path = os.path.join(data_dir, rel + ".lua")
    if not os.path.isfile(lua_path):
        return None
    try:
        with open(lua_path, "r", encoding="utf-8", errors="replace") as fh:
            src = fh.read()
        body = LuaParser(tokenize(src), lua_path).parse_chunk()
    except (LuaSyntaxError, OSError):
        return None
    table = None
    for stmt in body:
        if isinstance(stmt, TReturn) and stmt.exprs:
            env = Env()
            try:
                table = LuaRuntime(data_dir, []).eval(stmt.exprs[0], env, lua_path)
            except Exception:
                table = None
            break
    if not isinstance(table, LuaTable):
        return None
    return table


def compute_geometry(layer, data_dir, png_width, png_height, warnings):
    """Deduce la disposicion de frames de una capa."""
    geo = LayerGeometry()

    strip_path = layer.get("strip_path")
    strip = read_strip_definition(strip_path, data_dir) if strip_path else None

    frames = layer.get("frames")
    fw = fh = 0
    if isinstance(frames, LuaTable):
        fw = _as_int(frames.get("frame_width"))
        fh = _as_int(frames.get("frame_height"))
        count = _as_int(frames.get("count"))
        if count:
            geo.frame_count = count
        anim = frames.get("animation")
        if isinstance(anim, LuaTable):
            geo.anim_frames = _as_int(anim.get("frames")) or geo.anim_frames
            geo.anim_speed = _as_float(anim.get("speed")) or 0.0
    elif isinstance(frames, (int, float)) and not isinstance(frames, bool):
        geo.frame_count = int(frames)

    if (not fw or not fh) and strip is not None:
        fw = fw or _as_int(strip.get("width"))
        fh = fh or _as_int(strip.get("height"))

    if (not fw or not fh):
        fw = _as_int(layer.get("width"))
        fh = _as_int(layer.get("height"))
    if (not fw or not fh):
        size = _as_int(layer.get("size"))
        if size:
            fw = fw or size
            fh = fh or size

    declared_count = _as_int(layer.get("frame_count"))
    if declared_count:
        geo.frame_count = declared_count
    repeat_count = _as_int(layer.get("repeat_count"))
    geo.direction_count = _as_int(layer.get("direction_count")) or 1
    anim_speed = _as_float(layer.get("animation_speed"))
    if anim_speed is not None:
        geo.anim_speed = anim_speed
    if strip is not None:
        geo.line_length = _as_int(strip.get("line_length")) or 0

    # Factorio 2.1: el PNG es una rejilla y el prototipo dice cuantos frames
    # caben en horizontal y en vertical (width_in_frames / height_in_frames).
    wif = _as_int(layer.get("width_in_frames"))
    hif = _as_int(layer.get("height_in_frames"))

    if wif and hif and wif > 0 and hif > 0:
        geo.frame_width = max(1, png_width // wif)
        geo.frame_height = max(1, png_height // hif)
        geo.cols = wif
        geo.rows = hif
    elif fw and fh and fw > 0 and fh > 0:
        geo.frame_width = fw
        geo.frame_height = fh
        geo.cols = max(1, png_width // fw)
        geo.rows = max(1, png_height // fh)
        # el PNG deberia encajar exacto en la rejilla de frames
        if png_width % fw or png_height % fh:
            geo.grid_mismatch = True
            # si la definicion de tira dice cuantas columnas tiene, se usa
            if geo.line_length and geo.line_length > 0:
                geo.cols = geo.line_length
    else:
        # sin informacion de frame: la imagen entera es un frame
        geo.frame_width = png_width
        geo.frame_height = png_height
        geo.cols = 1
        geo.rows = 1
    geo.cells = geo.cols * geo.rows

    # la definicion de tira debe concordar con las filas reales del PNG
    if geo.line_length and not wif and not hif:
        if geo.line_length != geo.cols:
            geo.grid_mismatch = True
            if geo.line_length > 0:
                geo.cols = geo.line_length
                geo.cells = geo.cols * geo.rows

    # posicion absoluta dentro del PNG (sprites de acantilado etc.)
    ox = _as_int(layer.get("x"))
    oy = _as_int(layer.get("y"))
    if ox is not None and oy is not None:
        geo.offset_x = ox
        geo.offset_y = oy
        geo.cols = 1
        geo.rows = 1
        geo.cells = 1

    geo.sheet_width = png_width
    geo.sheet_height = png_height

    if geo.frame_count <= 0:
        geo.frame_count = geo.cells
    if geo.anim_frames <= 0:
        # sin tabla de animacion: los frames de la animacion son los declarados
        geo.anim_frames = geo.frame_count
    if repeat_count:
        # repeat_count: una sola imagen repetida N veces
        geo.anim_frames = repeat_count
    return geo


def _as_int(value):
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return int(value)
    return None


def _as_float(value):
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    return None


# ===========================================================================
# 8. PNG: lectura y escritura (sin Pillow)
# ===========================================================================

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


class PngError(Exception):
    pass


class PngHeader(object):
    __slots__ = ("width", "height", "bit_depth", "color_type", "interlace", "palette", "trns")

    def __init__(self):
        self.width = 0
        self.height = 0
        self.bit_depth = 8
        self.color_type = 6
        self.interlace = 0
        self.palette = None
        self.trns = None


def _iter_chunks(data):
    pos = 8
    length = len(data)
    while pos + 8 <= length:
        (chunk_len,) = struct.unpack(">I", data[pos:pos + 4])
        ctype = data[pos + 4:pos + 8]
        payload = data[pos + 8:pos + 8 + chunk_len]
        yield ctype, payload
        pos += 12 + chunk_len


def read_png_header(path):
    """Lee solo la cabecera IHDR (y PLTE/tRNS) sin decodificar."""
    with open(path, "rb") as fh:
        data = fh.read()
    if data[:8] != PNG_SIGNATURE:
        raise PngError("no es un PNG: %s" % path)
    header = PngHeader()
    for ctype, payload in _iter_chunks(data):
        if ctype == b"IHDR":
            (header.width, header.height, header.bit_depth, header.color_type,
             _comp, _filt, header.interlace) = struct.unpack(">IIBBBBB", payload[:13])
            break
    for ctype, payload in _iter_chunks(data):
        if ctype == b"PLTE":
            header.palette = payload
        elif ctype == b"tRNS":
            header.trns = payload
        elif ctype == b"IEND":
            break
    return header


def _channels(color_type):
    return {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}.get(color_type, 0)


def _paeth(a, b, c):
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def _unfilter(ftype, cur, prev, bpp):
    if ftype == 0:
        return cur
    if ftype == 1:
        for i in range(bpp, len(cur)):
            cur[i] = (cur[i] + cur[i - bpp]) & 0xFF
    elif ftype == 2:
        if prev:
            for i in range(len(cur)):
                cur[i] = (cur[i] + prev[i]) & 0xFF
    elif ftype == 3:
        for i in range(len(cur)):
            left = cur[i - bpp] if i >= bpp else 0
            up = prev[i] if prev else 0
            cur[i] = (cur[i] + ((left + up) >> 1)) & 0xFF
    elif ftype == 4:
        for i in range(len(cur)):
            left = cur[i - bpp] if i >= bpp else 0
            up = prev[i] if prev else 0
            upleft = prev[i - bpp] if (prev and i >= bpp) else 0
            cur[i] = (cur[i] + _paeth(left, up, upleft)) & 0xFF
    else:
        raise PngError("filtro PNG desconocido: %d" % ftype)
    return cur


def decode_png_rows(path, wanted_rows=None):
    """Genera (indice_fila, bytes_rgba) de un PNG.

    `wanted_rows` es un conjunto de indices de fila; si es None se devuelven
    todas.  La decodificacion es secuencial y se detiene en cuanto se han
    servido las filas pedidas (importante con hojas de 4096x8192)."""
    with open(path, "rb") as fh:
        data = fh.read()
    if data[:8] != PNG_SIGNATURE:
        raise PngError("no es un PNG: %s" % path)

    header = PngHeader()
    idat = bytearray()
    palette = None
    trns = None
    for ctype, payload in _iter_chunks(data):
        if ctype == b"IHDR":
            (header.width, header.height, header.bit_depth, header.color_type,
             _comp, _filt, header.interlace) = struct.unpack(">IIBBBBB", payload[:13])
        elif ctype == b"PLTE":
            palette = payload
        elif ctype == b"tRNS":
            trns = payload
        elif ctype == b"IDAT":
            idat += payload
        elif ctype == b"IEND":
            break

    if header.interlace != 0:
        raise PngError("PNG entrelazado (Adam7) no soportado: %s" % path)
    if header.bit_depth not in (1, 2, 4, 8, 16):
        raise PngError("profundidad de bits no soportada (%d): %s" %
                       (header.bit_depth, path))
    nch = _channels(header.color_type)
    if nch == 0:
        raise PngError("tipo de color PNG no soportado (%d): %s" %
                       (header.color_type, path))
    if header.color_type == 3 and palette is None:
        raise PngError("PNG con paleta sin PLTE: %s" % path)
    if header.bit_depth < 8 and header.color_type not in (0, 3):
        raise PngError("PNG de %d bits solo valido en gris o paleta: %s" %
                       (header.bit_depth, path))

    width = header.width
    height = header.height
    sample_bytes = header.bit_depth // 8
    if header.bit_depth < 8:
        # muestras empaquetadas: varios pixels por byte
        stride = (width * header.bit_depth + 7) // 8
        bpp = 1
    else:
        bpp = nch * sample_bytes
        stride = width * bpp

    # paleta -> RGBA
    pal_rgba = None
    if header.color_type == 3:
        pal_rgba = bytearray((len(palette) // 3) * 4)
        for i in range(len(palette) // 3):
            pal_rgba[i * 4] = palette[i * 3]
            pal_rgba[i * 4 + 1] = palette[i * 3 + 1]
            pal_rgba[i * 4 + 2] = palette[i * 3 + 2]
            pal_rgba[i * 4 + 3] = trns[i] if (trns and i < len(trns)) else 255

    decomp = zlib.decompressobj()
    pending = bytearray()
    prev = bytearray(stride)
    row_index = 0
    input_pos = 0
    idat_len = len(idat)

    while row_index < height:
        # completa `pending` hasta tener una scanline completa
        while len(pending) < stride + 1:
            if input_pos < idat_len:
                chunk = idat[input_pos:input_pos + 65536]
                input_pos += len(chunk)
                pending += decomp.decompress(chunk)
            elif not decomp.eof:
                pending += decomp.flush()
                break
            else:
                break
        if len(pending) < stride + 1:
            break
        ftype = pending[0]
        cur = bytearray(pending[1:stride + 1])
        del pending[:stride + 1]
        _unfilter(ftype, cur, prev, bpp)
        prev = cur
        if wanted_rows is None or row_index in wanted_rows:
            yield row_index, _row_to_rgba(cur, header, nch, sample_bytes, width,
                                          pal_rgba)
        row_index += 1


def _row_to_rgba(cur, header, nch, sample_bytes, width, pal_rgba):
    color_type = header.color_type
    if header.bit_depth < 8:
        # muestras empaquetadas (1, 2 o 4 bits por pixel)
        bits = header.bit_depth
        per_byte = 8 // bits
        mask = (1 << bits) - 1
        maxval = mask
        out = bytearray(width * 4)
        for i in range(width):
            byte = cur[i // per_byte]
            shift = 8 - bits * (i % per_byte + 1)
            value = (byte >> shift) & mask
            if color_type == 3:
                out[i * 4:i * 4 + 4] = pal_rgba[value * 4:value * 4 + 4]
            else:
                gray = value * 255 // maxval if maxval else 0
                out[i * 4] = gray
                out[i * 4 + 1] = gray
                out[i * 4 + 2] = gray
                out[i * 4 + 3] = 255
        return bytes(out)
    if color_type == 6:
        if sample_bytes == 1:
            return bytes(cur)
        out = bytearray(width * 4)
        for i in range(width):
            base = i * 4 * sample_bytes
            out[i * 4:i * 4 + 4] = cur[base:base + 4]
        return bytes(out)
    out = bytearray(width * 4)
    if color_type == 2:
        for i in range(width):
            base = i * 3 * sample_bytes
            out[i * 4] = cur[base]
            out[i * 4 + 1] = cur[base + sample_bytes]
            out[i * 4 + 2] = cur[base + 2 * sample_bytes]
            out[i * 4 + 3] = 255
    elif color_type == 0:
        for i in range(width):
            v = cur[i * sample_bytes]
            out[i * 4] = v
            out[i * 4 + 1] = v
            out[i * 4 + 2] = v
            out[i * 4 + 3] = 255
    elif color_type == 4:
        for i in range(width):
            v = cur[i * 2 * sample_bytes]
            a = cur[i * 2 * sample_bytes + sample_bytes]
            out[i * 4] = v
            out[i * 4 + 1] = v
            out[i * 4 + 2] = v
            out[i * 4 + 3] = a
    elif color_type == 3:
        for i in range(width):
            idx = cur[i]
            out[i * 4:i * 4 + 4] = pal_rgba[idx * 4:idx * 4 + 4]
    return bytes(out)


def png_encode_rgba(width, height, pixels):
    """Codifica un buffer RGBA8 en un PNG (filtro 0 + zlib)."""
    if len(pixels) != width * height * 4:
        raise PngError("buffer RGBA con tamano inesperado")
    raw = bytearray()
    stride = width * 4
    for y in range(height):
        raw.append(0)
        raw += pixels[y * stride:(y + 1) * stride]

    def chunk(ctype, payload):
        return (struct.pack(">I", len(payload)) + ctype + payload +
                struct.pack(">I", zlib.crc32(ctype + payload) & 0xFFFFFFFF))

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    compressor = zlib.compressobj(6)
    idat = compressor.compress(bytes(raw)) + compressor.flush()
    return (PNG_SIGNATURE + chunk(b"IHDR", ihdr) +
            chunk(b"IDAT", idat) + chunk(b"IEND", b""))


# ===========================================================================
# 9. Empaquetado del atlas (shelf packer)
# ===========================================================================

def pack_shelf(rects, max_width):
    """Coloca rectangulos (w, h, key) en un atlas de ancho <= max_width.

    Devuelve (placements, width, height) donde placements es
    {key: (x, y)}.  Garantiza que ningun rectangulo se solapa."""
    ordered = sorted(rects, key=lambda r: (-r[1], -r[0], str(r[2])))
    placements = {}
    x = y = row_h = 0
    max_used = 0
    for w, h, key in ordered:
        if w > max_width:
            raise ExtractError("frame %r (%dx%d) no cabe en el atlas de ancho %d"
                              % (key, w, h, max_width))
        if x + w > max_width:
            x = 0
            y += row_h
            row_h = 0
        placements[key] = (x, y)
        x += w
        if h > row_h:
            row_h = h
        if x > max_used:
            max_used = x
    height = y + row_h
    width = max(max_used, 1)
    return placements, width, height


def pack_skyline(rects, max_width):
    """Empaquetador 'skyline bottom-left' de una sola pagina.

    Mucho mas compacto que el shelf clasico cuando hay muchos frames de
    alturas distintas (en Factorio hay ~250 alturas distintas), que es
    justamente el caso de un atlas de sprites.  Garantiza que ningun
    rectangulo se solapa."""
    packer = PagePacker(max_width)
    for w, h, key in rects:
        packer.add(key, w, h)
    return packer.placements(), packer.width(), packer.height()


class PagePacker(object):
    """Empaqueta rectangulos en paginas de 'page_size' x 'page_size'.

    Cada pagina es un rectangulo del tamaño maximo de textura que la GPU
    soporta (GL_MAX_TEXTURE_SIZE suele ser 2048 o 4096).  Cuando una pagina
    se llena se abre la siguiente.
    """

    def __init__(self, page_size):
        self.page_size = max(1, int(page_size))
        self._pages = []          # [{skyline, placements, width, height}]

    # -- interno ---------------------------------------------------------
    def _new_page(self):
        page = {
            "skyline": [[0, 0, self.page_size]],
            "placements": {},
            "sizes": {},
            "width": 0,
            "height": 0,
        }
        self._pages.append(page)
        return page

    def _find_position(self, page, w, h):
        """Mejor posicion (y minima, luego x minima) dentro de la pagina."""
        best = None
        for i, node in enumerate(page["skyline"]):
            nx, ny, _nw = node
            yy = ny
            rem = w
            j = i
            while rem > 0 and j < len(page["skyline"]):
                if page["skyline"][j][1] > yy:
                    yy = page["skyline"][j][1]
                rem -= page["skyline"][j][2]
                j += 1
            if rem > 0:
                continue              # no cabe horizontalmente
            if yy + h > self.page_size:
                continue              # no cabe verticalmente
            if best is None or (yy, nx) < (best[0], best[1]):
                best = (yy, nx, i)
        return best

    def _place(self, page, w, h, key):
        best = self._find_position(page, w, h)
        if best is None:
            return None
        y, x, i = best
        page["placements"][key] = (x, y)
        page["sizes"][key] = (w, h)
        touched = []
        rem = w
        j = i
        while rem > 0 and j < len(page["skyline"]):
            node = page["skyline"][j]
            take = min(rem, node[2])
            touched.append((node, take))
            rem -= take
            j += 1
        new = [list(n) for n in page["skyline"][:i]]
        new.append([x, y + h, w])
        for node, take in touched:
            left = node[2] - take
            if left > 0:
                new.append([node[0] + take, node[1], left])
        new.extend([list(n) for n in page["skyline"][j:]])
        page["skyline"] = new
        if y + h > page["height"]:
            page["height"] = y + h
        if x + w > page["width"]:
            page["width"] = x + w
        return (x, y)

    # -- publico ---------------------------------------------------------
    def add(self, key, w, h):
        """Coloca un rectangulo; devuelve (pagina, x, y) o None."""
        if w > self.page_size or h > self.page_size:
            return None
        if not self._pages:
            self._new_page()
        page = self._pages[-1]
        placed = self._place(page, w, h, key)
        if placed is None:
            page = self._new_page()
            placed = self._place(page, w, h, key)
            if placed is None:
                return None
            return len(self._pages) - 1, placed[0], placed[1]
        return len(self._pages) - 1, placed[0], placed[1]

    def page_count(self):
        return len(self._pages)

    def pages(self):
        """Las paginas con su skyline, colocaciones y medidas."""
        return self._pages

    def placements(self):
        """{clave: (pagina, x, y)} de todas las paginas."""
        out = {}
        for index, page in enumerate(self._pages):
            for key, (x, y) in page["placements"].items():
                out[key] = (index, x, y)
        return out

    def width(self):
        return max([p["width"] for p in self._pages] or [0])

    def height(self):
        return max([p["height"] for p in self._pages] or [0])


# ===========================================================================
# 10. Locale (INI de Factorio)
# ===========================================================================

def parse_locale(cfg_path):
    """Parsea base/locale/en/base.cfg -> {clave: texto} plano."""
    strings = {}
    section = ""
    with open(cfg_path, "r", encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith(";") or line.startswith("#"):
                continue
            if line.startswith("[") and "]" in line:
                section = line[1:line.index("]")]
                continue
            if "=" not in line:
                continue
            key, _sep, value = line.partition("=")
            key = key.strip()
            value = value.strip()
            if not key:
                continue
            full = "%s.%s" % (section, key) if section else key
            strings[full] = value
    return strings


# ===========================================================================
# 11.Ayudas varias
# ===========================================================================

def resolve_asset_path(data_dir, ref):
    """'__base__/graphics/...' -> ruta absoluta en el disco."""
    ref = ref.replace("\\", "/")
    for prefix, folder in (("__base__/", "base/"), ("__core__/", "core/")):
        if ref.startswith(prefix):
            return os.path.join(data_dir, folder + ref[len(prefix):])
    if ref.startswith("base/") or ref.startswith("core/"):
        return os.path.join(data_dir, ref)
    return os.path.join(data_dir, ref)


def find_asset_file(data_dir, ref):
    """Localiza el fichero de un asset referenciado.

    En Factorio 2.1 los graficos tienen variantes de calidad
    (<nombre>-1.png .. <nombre>-8.png) y el prototipo puede seguir citando el
    nombre base.  Si el fichero base no existe se prueban las variantes."""
    primary = resolve_asset_path(data_dir, ref)
    if os.path.isfile(primary):
        return primary
    stem, ext = os.path.splitext(primary)
    for index in range(1, 9):
        variant = "%s-%d%s" % (stem, index, ext)
        if os.path.isfile(variant):
            return variant
    return primary


def relative_asset_path(data_dir, ref):
    """'__base__/graphics/...' -> 'base/graphics/...' (relativo al data dir)."""
    ref = ref.replace("\\", "/")
    for prefix, folder in (("__base__/", "base/"), ("__core__/", "core/")):
        if ref.startswith(prefix):
            return folder + ref[len(prefix):]
    if ref.startswith("base/") or ref.startswith("core/"):
        return ref
    return ref.lstrip("/")


def detect_version(data_dir):
    """Detecta la version de Factorio a partir de base/info.json."""
    info = os.path.join(data_dir, "base", "info.json")
    if os.path.isfile(info):
        try:
            with open(info, "r", encoding="utf-8") as fh:
                data = json.load(fh)
            version = data.get("version")
            if version:
                return str(version)
        except (OSError, ValueError):
            pass
    for candidate in ("version.txt",):
        path = os.path.join(data_dir, candidate)
        if os.path.isfile(path):
            try:
                with open(path, "r", encoding="utf-8") as fh:
                    return fh.read().strip()
            except OSError:
                pass
    return "unknown"


def detect_graphics_layout(data_dir, version=None):
    """'2.1' si usa la estructura de tiras con .lua, '2.0' en caso contrario.

    La version es lo mas fiable: 2.1 cambio los sprites a <tipo>-<capa>.png +
    <tipo>-<capa>.lua.  Si no se puede leer la version se mira si hay
    ficheros .lua de definicion de tira junto a los PNG."""
    if version and version[0].isdigit():
        if version.startswith("2.1"):
            return "2.1"
        if version.startswith("2.0"):
            return "2.0"
    entity_dir = os.path.join(data_dir, "base", "graphics", "entity")
    if not os.path.isdir(entity_dir):
        return "unknown"
    for dirpath, _dirnames, filenames in os.walk(entity_dir):
        for filename in filenames:
            if not filename.endswith(".lua"):
                continue
            stem = filename[:-4]
            if os.path.isfile(os.path.join(dirpath, stem + ".png")):
                return "2.1"
    return "2.0"


def list_sounds(data_dir):
    """{'base/sound/xxx': 'base/sound/xxx.ogg'} para todos los .ogg del juego."""
    sounds = {}
    sound_dir = os.path.join(data_dir, "base", "sound")
    if not os.path.isdir(sound_dir):
        return sounds
    for dirpath, _dirnames, filenames in os.walk(sound_dir):
        for filename in filenames:
            if not filename.lower().endswith(".ogg"):
                continue
            full = os.path.join(dirpath, filename)
            rel = os.path.relpath(full, data_dir).replace(os.sep, "/")
            sounds[rel[:-len(".ogg")]] = rel
    return sounds


# ===========================================================================
# 12. Extraccion principal
# ===========================================================================

class EntityAsset(object):
    __slots__ = ("name", "ptype", "layers", "draw_order", "omitted",
                 "direction_count", "variants", "flipped")

    def __init__(self, name, ptype):
        self.name = name
        self.ptype = ptype
        self.layers = []
        self.draw_order = DEFAULT_DRAW_ORDER
        self.omitted = None
        self.direction_count = 0
        self.variants = {}      # {"N": indice_de_capa, ...}
        self.flipped = {}       # {"N": indice_de_capa, ...}


class LayerAsset(object):
    __slots__ = ("entity", "index", "ref", "rel_path", "abs_path", "priority",
                 "geometry", "header", "frames", "omitted", "flags", "shift",
                 "scale", "anim_speed", "frame_count", "direction_count",
                 "table", "is_body", "z_order", "atlas_x", "atlas_y",
                 "animation_frames", "blocks", "grid_cols", "grid_rows", "path")

    def __init__(self):
        self.entity = ""
        self.index = 0
        self.ref = ""
        self.rel_path = ""
        self.abs_path = ""
        self.priority = "normal"
        self.geometry = None
        self.header = None
        self.frames = []
        self.omitted = None
        self.flags = []
        self.shift = None
        self.scale = None
        self.anim_speed = 0.0
        self.frame_count = 1
        self.direction_count = 1
        self.table = None      # tabla Lua original de la capa
        self.is_body = False
        self.z_order = 0
        self.atlas_x = 0
        self.atlas_y = 0
        self.animation_frames = 1
        self.blocks = []
        self.grid_cols = 1
        self.grid_rows = 1
        self.path = ()


def _lua_to_json(value):
    if isinstance(value, LuaTable):
        return value.to_plain()
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    return None


def build_entity_assets(runtime, data_dir, proto_dir, warnings):
    """Fase 1: parseo de prototipos y descubrimiento de capas."""
    prototypes = collect_prototypes(runtime, proto_dir, warnings)
    entities = []
    no_sprite = []
    for proto in prototypes:
        ptype = proto.get("type")
        pname = proto.get("name")
        if not isinstance(ptype, str) or not isinstance(pname, str):
            continue
        layers_tables = extract_layers(proto, warnings)
        if not layers_tables:
            no_sprite.append((ptype, pname))
            continue
        entity = EntityAsset(pname, ptype)
        # deduplica por fichero, quedandose con la variante que mas frames
        # aporta (p.ej. gun-turret usa gun-turret-raising.png en varias fases)
        chosen = {}
        order = []
        for table, path in layers_tables:
            ref = table.get("filename")
            if not isinstance(ref, str):
                continue
            score = _layer_score(table)
            if ref not in chosen:
                chosen[ref] = (score, table, path)
                order.append(ref)
            elif score > chosen[ref][0]:
                chosen[ref] = (score, table, path)
        for ref in order:
            _score, table, path = chosen[ref]
            layer = LayerAsset()
            layer.entity = pname
            layer.ref = ref
            layer.rel_path = relative_asset_path(data_dir, ref)
            layer.abs_path = resolve_asset_path(data_dir, ref)
            layer.path = path
            priority = table.get("priority")
            layer.priority = priority if isinstance(priority, str) else "normal"
            flags = table.get("flags")
            if isinstance(flags, LuaTable):
                layer.flags = [f for f in flags.arr if isinstance(f, str)]
            layer.shift = _lua_to_json(table.get("shift"))
            layer.scale = _as_float(table.get("scale"))
            layer.table = table
            entity.layers.append(layer)
        for i, layer in enumerate(entity.layers):
            layer.index = i
        # direction_count del prototipo (si lo declara)
        entity.direction_count = _as_int(proto.get("direction_count")) or 0
        if entity.layers:
            orders = [PRIORITY_DRAW_ORDER.get(l.priority, DEFAULT_DRAW_ORDER)
                      for l in entity.layers]
            entity.draw_order = max(orders)
            entities.append(entity)
    return entities, no_sprite


# Sufijos de direccion que usa Factorio en el nombre de los ficheros
DIRECTION_SUFFIXES = ("N", "E", "S", "W")

# Marcas de una capa que NO son el cuerpo de la entidad
NON_BODY_MARKS = ("-shadow", "-light", "-lights", "-reflection",
                  "-reflections", "-mask", "-overlay", "-glow")


def detect_variant(ref):
    """Deduce (direccion, flipped) del nombre del fichero.

    'burner-mining-drill-N.png'        -> ('N', False)
    'burner-mining-drill-N-flipped.png'-> ('N', True)
    """
    stem = os.path.basename(ref).rsplit(".", 1)[0]
    parts = stem.split("-")
    flipped = False
    if parts and parts[-1] == "flipped":
        flipped = True
        parts = parts[:-1]
    direction = None
    if len(parts) > 1 and parts[-1] in DIRECTION_SUFFIXES:
        direction = parts[-1]
    return direction, flipped


def is_body_layer(layer):
    """True si la capa define el cuerpo visible de la entidad."""
    name = layer.ref.lower()
    for mark in NON_BODY_MARKS:
        if mark in name:
            return False
    if "shadow" in layer.flags:
        return False
    if "mask" in layer.flags:
        return False
    if "light" in layer.flags:
        return False
    return True


def _classify_layers(entity):
    """Marca is_body y calcula los mapas de variantes por direccion.

    El cuerpo es la primera capa "de dibujo" (ni sombra, ni luz, ni reflejo)
    que vive dentro del contenedor principal de graficos del prototipo
    (graphics_set, pictures, sprite...).  Si no hay ninguno, se usa la
    primera capa de dibujo que haya."""
    candidates = [l for l in entity.layers if is_body_layer(l)]
    main = [l for l in candidates
            if any(key in MAIN_GRAPHICS_KEYS for key in l.path)]
    body = (main or candidates)
    body_ids = set(id(l) for l in body[:1]) if body else set()
    # si hay varias capas de cuerpo del mismo tamano (las 4 direcciones de
    # una perforadora), todas son cuerpo y se distinguen por 'variants'
    if body:
        biggest = max(l.geometry.frame_pixels for l in body) if \
            all(l.geometry is not None for l in body) else 0
        if biggest:
            same = [l for l in body if l.geometry is not None and
                    l.geometry.frame_pixels == biggest]
            if len(same) > 1 and len(main) == 1:
                body_ids = set(id(l) for l in same)
    for layer in entity.layers:
        layer.is_body = id(layer) in body_ids
        # las variantes por direccion: una capa por direccion (N/E/S/W).
        # is_body se queda solo en la capa por defecto, pero el motor puede
        # pedir la variante concreta de cada direccion
        direction, flipped = detect_variant(layer.ref)
        if direction:
            if flipped:
                entity.flipped[direction] = layer.index
            else:
                entity.variants[direction] = layer.index
    if not entity.variants and not entity.flipped:
        # una sola capa de cuerpo: es la variante por defecto
        for layer in entity.layers:
            if layer.is_body:
                entity.variants["default"] = layer.index
                break


def resolve_layer_geometry(entities, data_dir, warnings, missing):
    """Fase 2: lee cabeceras PNG y deduce la geometria de cada capa."""
    for entity in entities:
        kept = []
        for layer in entity.layers:
            actual = find_asset_file(data_dir, layer.ref)
            if not os.path.isfile(actual):
                missing.append((layer.entity, layer.ref))
                continue
            if actual != layer.abs_path:
                # variante de calidad (<nombre>-1.png .. -8.png)
                layer.abs_path = actual
                layer.rel_path = os.path.relpath(actual, data_dir).replace(os.sep, "/")
            try:
                header = read_png_header(layer.abs_path)
            except (PngError, OSError) as exc:
                warnings.append("png: %s (%s)" % (layer.rel_path, exc))
                continue
            if header.interlace != 0:
                warnings.append("png entrelazado omitido: %s" % layer.rel_path)
                continue
            if header.bit_depth not in (1, 2, 4, 8, 16):
                warnings.append("png de %d bits omitido: %s" %
                                (header.bit_depth, layer.rel_path))
                continue
            layer.header = header
            geo = compute_geometry(layer.table, data_dir,
                                   header.width, header.height, warnings)
            if geo.frame_pixels > MAX_FRAME_PIXELS:
                warnings.append("frame demasiado grande omitido: %s (%dx%d)" %
                                (layer.rel_path, geo.frame_width, geo.frame_height))
                continue
            layer.geometry = geo
            layer.frame_count = geo.frame_count
            layer.anim_speed = geo.anim_speed
            # direction_count: el del prototipo manda; si no, el de la capa;
            # si no, se deduce de las filas de la hoja
            if entity.direction_count:
                layer.direction_count = entity.direction_count
            elif geo.direction_count > 1:
                layer.direction_count = geo.direction_count
            else:
                layer.direction_count = geo.rows
            # frames de animacion por direccion
            if layer.direction_count > 1 and geo.cells % layer.direction_count == 0:
                layer.animation_frames = geo.cells // layer.direction_count
            else:
                layer.animation_frames = geo.frame_count
            kept.append(layer)
        entity.layers = kept
        if kept:
            entity.draw_order = max(PRIORITY_DRAW_ORDER.get(l.priority, DEFAULT_DRAW_ORDER)
                                    for l in kept)
        # ahora ya se sabe el tamano de cada capa: se elige el cuerpo
        _classify_layers(entity)
    return entities


# ===========================================================================
# 13. CLI
# ===========================================================================

def build_arg_parser():
    parser = argparse.ArgumentParser(
        description="Extrae los sprites, iconos, textos y sonidos reales de "
                    "Factorio y los deja listos para el motor VR.",
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True,
                        help="directorio data/ de la instalacion de Factorio")
    parser.add_argument("--out", help="directorio de salida")
    parser.add_argument("--types", default="",
                        help="filtro de entidades separadas por comas "
                             "(nombre o tipo de prototipo)")
    parser.add_argument("--version", action="store_true",
                        help="detecta y reporta la version de Factorio")
    parser.add_argument("--atlas-size", type=int, default=2048,
                        help="ancho maximo del atlas en pixeles (por defecto 2048)")
    parser.add_argument("--page-size", type=int, default=2048,
                        help="lado de cada pagina del atlas en pixeles "
                             "(por defecto 2048, el limite de OpenGL ES)")
    parser.add_argument("--max-pages", type=int, default=32,
                        help="numero maximo de paginas del atlas (por defecto 32)")
    parser.add_argument("--frames-per-layer", type=int, default=0,
                        help="frames por capa en el atlas (0 = todos). "
                             "Recorta los sprites para conseguir un atlas "
                             "pequeno; la rejilla sigue siendo rectangular")
    parser.add_argument("--scale", type=float, default=0.5,
                        help="escala de los pixeles del atlas (por defecto 0.5). "
                             "NO cambia el tamano logico del frame")
    parser.add_argument("--capas", choices=("cuerpo", "todas"), default="cuerpo",
                        help="que capas meter: 'cuerpo' (cuerpo y animacion) o "
                             "'todas' (sombra, luces y reflejos tambien)")
    parser.add_argument("--only-listed", action="store_true",
                        help="extraer solo los tipos de entidad que el motor VR "
                             "dibuja (fabrica, recursos y arboles)")
    parser.add_argument("--no-raw", action="store_true",
                        help="no escribir los atlas_N.rgba (el motor los sube "
                             "crudos a la GPU)")
    parser.add_argument("--skip-icons", action="store_true",
                        help="no copiar los iconos de items")
    parser.add_argument("--skip-locale", action="store_true",
                        help="no extraer strings.json")
    parser.add_argument("--skip-sounds", action="store_true",
                        help="no listar los sonidos del juego")
    parser.add_argument("--skip-terrain", action="store_true",
                        help="no copiar los tiles de terreno")
    parser.add_argument("--terrain-variants", type=int, default=16,
                        help="variantes de suelo que se guardan por tipo de "
                             "terreno (por defecto 16). Cada variante es una "
                             "casilla de 32x32")
    parser.add_argument("--strict", action="store_true",
                        help="salir con codigo != 0 si falta algun PNG referenciado")
    parser.add_argument("--quiet", action="store_true", help="silencia el resumen")
    return parser


def _variantes_de_terreno(cab, filas, max_variantes):
    """Elige celdas de 32x32 de suelo plano dentro de una hoja de terreno.

    Las hojas de terreno de Factorio son tiras de 4096x576: 128 columnas de
    32 px (variantes) por 18 bandas de 32 px.  El juego las reparte en
    grupos "main" (ver tile_variations_template en
    base/prototypes/tile/tiles.lua: y = 0, 128 y 320 pixeles de la hoja), y
    las bandas 8-9 estan vacias (los huecos de transicion).

    Para el suelo de una casilla hacen falta celdas COMPLETAMENTE opacas:
    las de las bandas con bordes tienen alfa parcial y al muestrear se ve el
    recorte.  Se cogen de las bandas altas (10-17), que son las que el juego
    usa para el grupo "size 4" y son suelo liso, repartidas por las columnas
    para que las casillas contiguas no se repitan.

    Devuelve [(banda, columna), ...] con max_variantes celdas como mucho."""
    T = 32
    columnas = cab.width // T
    bandas = cab.height // T
    # El grupo "size 4" del juego empieza en la fila 320 de la hoja, que es
    # la banda 10 en celdas de 32 px.
    primera_banda = 10
    candidatas = []
    for b in range(primera_banda, bandas):
        for c in range(columnas):
            candidatas.append((b, c))
    if not candidatas:
        return []
    if len(candidatas) <= max_variantes:
        return candidatas
    # Repartidas por las columnas: se recorre de izquierda a derecha dando
    # un salto, para que las variantes elegidas esten separadas.
    paso = len(candidatas) / float(max_variantes)
    return [candidatas[int(i * paso)] for i in range(max_variantes)]


def _empaquetar_atlas_terreno(src_terrain, out_dir, manifest, args):
    """Empaqueta el suelo de las hojas de terreno en un atlas paginado.

    El motor del visor (y el del juego) suben las texturas crudas a la GPU:
    el PNG no se decodifica en Android.  Ademas una hoja de terreno son
    4096x576 y no cabe como una sola textura en la GPU movil, asi que el
    suelo se recorta a celdas de 32x32 (el tamano de una casilla) y se
    pagina igual que el atlas de sprites.

    Se escribe en manifest["terrain_atlas"]:
      {"page_size": 2048, "tile_size": 32, "pages": [...],
       "tiles": {"grass-1": {"variants": [{"page":0,"x":0,"y":0}, ...]}}}"""
    if not os.path.isdir(src_terrain):
        return 0

    page_size = args.page_size
    tile = 32
    por_pagina = max(1, (page_size // tile) ** 2)

    # 1. Recoger las celdas de cada tipo de terreno.
    eleccion = {}
    for filename in sorted(os.listdir(src_terrain)):
        if not filename.lower().endswith(".png"):
            continue
        src = os.path.join(src_terrain, filename)
        if not os.path.isfile(src):
            continue
        nombre = filename[:-4]
        try:
            cab = read_png_header(src)
            filas = {}
            for idx, fila in decode_png_rows(src, wanted_rows=None):
                filas[idx] = fila
        except (PngError, OSError) as exc:
            sys.stderr.write("AVISO: no se pudo leer el terreno %s: %s\n"
                             % (filename, exc))
            continue
        celdas = _variantes_de_terreno(cab, filas, args.terrain_variants)
        if celdas:
            eleccion[nombre] = (cab, filas, celdas)

    if not eleccion:
        sys.stderr.write("AVISO: no se encontraron celdas de terreno usables\n")
        return 0

    # 2. Repartirlas en paginas (rejilla regular: todos los tiles son 32x32).
    paginas = []
    tiles = []
    for nombre, (cab, filas, celdas) in sorted(eleccion.items()):
        for banda, columna in celdas:
            tiles.append((nombre, banda, columna, filas, cab))

    # 3. Escribir las paginas.
    total_paginas = (len(tiles) + por_pagina - 1) // por_pagina
    paginas_info = []
    Placement = {}
    for p in range(total_paginas):
        trozo = tiles[p * por_pagina:(p + 1) * por_pagina]
        page = bytearray(page_size * page_size * 4)
        for i, (nombre, banda, columna, filas, cab) in enumerate(trozo):
            celdas_por_fila = page_size // tile
            cx = (i % celdas_por_fila) * tile
            cy = (i // celdas_por_fila) * tile
            sx = columna * tile
            sy = banda * tile
            for r in range(tile):
                fila = filas.get(sy + r)
                if fila is None:
                    continue
                off_origen = sx * 4
                off_destino = ((cy + r) * page_size + cx) * 4
                page[off_destino:off_destino + tile * 4] = \
                    fila[off_origen:off_origen + tile * 4]
            Placement.setdefault(nombre, []).append({
                "page": p, "x": cx, "y": cy, "w": tile, "h": tile,
            })

        base = "terrain_atlas_%d" % p
        raw_path = os.path.join(out_dir, base + ".rgba")
        if not args.no_raw:
            with open(raw_path, "wb") as fh:
                fh.write(bytes(page))
        png_path = os.path.join(out_dir, base + ".png")
        with open(png_path, "wb") as fh:
            fh.write(png_encode_rgba(page_size, page_size, bytes(page)))
        paginas_info.append({
            "file": base + (".rgba" if not args.no_raw else ".png"),
            "width": page_size,
            "height": page_size,
        })

    manifest["terrain_atlas"] = {
        "page_size": page_size,
        "tile_size": tile,
        "pixel_format": "RGBA8",
        "pages": paginas_info,
        "tiles": {nombre: {"variants": vs} for nombre, vs in sorted(Placement.items())},
    }
    return len(tiles)


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    args = build_arg_parser().parse_args(argv)

    data_dir = os.path.abspath(os.path.expanduser(args.data))
    if not os.path.isdir(data_dir):
        sys.stderr.write("ERROR: no existe el directorio de datos: %s\n" % data_dir)
        return 1
    if not os.path.isdir(os.path.join(data_dir, "base")):
        sys.stderr.write("ERROR: %s no parece una instalacion de Factorio "
                         "(falta base/)\n" % data_dir)
        return 1

    version = detect_version(data_dir)
    layout = detect_graphics_layout(data_dir, version)
    if args.version:
        print("Factorio data version: %s (layout de sprites: %s)" % (version, layout))
        if not args.out:
            return 0

    if not args.out:
        sys.stderr.write("ERROR: falta --out (directorio de salida)\n")
        return 1

    out_dir = os.path.abspath(os.path.expanduser(args.out))
    try:
        os.makedirs(out_dir, exist_ok=True)
    except OSError as exc:
        sys.stderr.write("ERROR: no se pudo crear %s: %s\n" % (out_dir, exc))
        return 1

    warnings = []
    missing = []
    proto_dir = os.path.join(data_dir, "base", "prototypes", "entity")

    runtime = LuaRuntime(data_dir, warnings)
    entities, no_sprite = build_entity_assets(runtime, data_dir, proto_dir, warnings)
    if not entities:
        sys.stderr.write("ERROR: no se resolvio ninguna entidad con sprite en %s\n"
                         % proto_dir)
        return 1

    # filtro --types / --only-listed
    wanted = [t.strip() for t in args.types.split(",") if t.strip()]
    if args.only_listed:
        # por nombre, y los tipos de la lista entera (los arboles)
        names = set(LISTED_ENTITY_NAMES) | set(wanted)
        types = set(LISTED_ENTITY_TYPES)
        entities = [e for e in entities
                    if e.name in names or e.ptype in types]
        if not entities:
            sys.stderr.write("ERROR: --only-listed no coincide con ninguna "
                             "entidad\n")
            return 1
    elif wanted:
        wset = set(wanted)
        entities = [e for e in entities
                    if e.name in wset or e.ptype in wset]
        if not entities:
            sys.stderr.write("ERROR: el filtro %s no coincide con ninguna "
                             "entidad\n" % args.types)
            return 1

    # segunda pasada: geometria de cada capa (cabeceras PNG + definicion de tira)
    entities = resolve_layer_geometry(entities, data_dir, warnings, missing)

    entities = [e for e in entities if e.layers]
    if not entities:
        sys.stderr.write(
            "ERROR: no se encontro ningun PNG de sprite en %s\n"
            "       Si es una instalacion headless (sin graficos) los sprites\n"
            "       no existen: usa una instalacion de Factorio con graficos.\n"
            "       PNG referenciados que faltan: %d\n"
            % (data_dir, len(missing)))
        for ename, ref in missing[:10]:
            sys.stderr.write("         - %s (en %s)\n" % (ref, ename))
        return 1

    # --- que capas se quedan ----------------------------------------------
    # Por defecto solo el cuerpo y la animacion: la sombra se simula con un
    # quad oscuro en el motor y las luces de conexion/estado no se pintan.
    if args.capas == "cuerpo":
        for entity in entities:
            kept = []
            for layer in entity.layers:
                name = layer.rel_path.lower()
                if any(mark in name for mark in
                       ("-shadow", "-light", "-lights", "-reflection",
                        "-reflections", "-mask", "-glow")):
                    layer.omitted = "capa-no-dibujada"
                    continue
                if "shadow" in layer.flags or "mask" in layer.flags or \
                        "light" in layer.flags:
                    layer.omitted = "capa-no-dibujada"
                    continue
                # las variantes reflejadas se descartan: el motor puede
                # espejar la normal
                _direction, flipped = detect_variant(layer.ref)
                if flipped:
                    layer.omitted = "capa-no-dibujada"
                    continue
                kept.append(layer)
            entity.layers = kept
            entity.flipped = {}
        entities = [e for e in entities if e.layers]

    # --- paginacion del atlas --------------------------------------------
    # Cada pagina mide como maximo page_size x page_size.  OpenGL ES
    # garantiza GL_MAX_TEXTURE_SIZE = 2048, asi que una textura mas alta
    # no se puede subir a la GPU: por eso el atlas se pagina.
    page_size = max(64, args.page_size)
    max_pages = max(1, args.max_pages)
    budget = max_pages * page_size * page_size

    total_cells = 0
    total_sheets = 0
    for entity in entities:
        for layer in entity.layers:
            total_cells += layer.geometry.cells
            total_sheets += (layer.geometry.sheet_width *
                             layer.geometry.sheet_height)

    def used_grid(layer):
        """Rejilla que se mete en el atlas: (columnas, filas).

        Sin limite son todas las celdas de la hoja.  Con --frames-per-layer
        se recortan las primeras celdas en orden de fila, pero la rejilla
        sigue siendo rectangular, de forma que len(frames) == cols*rows
        siempre se cumple."""
        geo = layer.geometry
        cols, rows = geo.cols, geo.rows
        limit = args.frames_per_layer
        if limit <= 0:
            return cols, rows
        if cols * rows <= limit:
            return cols, rows
        used_cols = min(cols, limit)
        used_rows = min(rows, (limit + used_cols - 1) // used_cols)
        return used_cols, used_rows

    def scaled_cell(layer):
        """Tamano del frame DENTRO del atlas (puede estar reescalado)."""
        geo = layer.geometry
        w = max(1, int(round(geo.frame_width * args.scale)))
        h = max(1, int(round(geo.frame_height * args.scale)))
        return w, h

    def blocks_of(layer):
        """Trocea la hoja de la capa en bloques que caben en una pagina."""
        geo = layer.geometry
        fw, fh = geo.frame_width, geo.frame_height
        if fw <= 0 or fh <= 0:
            return []
        cols, rows = used_grid(layer)
        cell_w, cell_h = scaled_cell(layer)
        # los bloques no son mas grandes que BLOCK_SIZE: asi el empaquetador
        # puede llenar las paginas mucho mejor (los sprites de Factorio tienen
        # formas muy dispares)
        cols_per = max(1, min(page_size // cell_w, BLOCK_SIZE // cell_w))
        rows_per = max(1, min(page_size // cell_h, BLOCK_SIZE // cell_h))
        blocks = []
        r0 = 0
        while r0 < rows:
            n_rows = min(rows_per, rows - r0)
            c0 = 0
            while c0 < cols:
                n_cols = min(cols_per, cols - c0)
                blocks.append((c0, r0, n_cols, n_rows,
                               n_cols * cell_w, n_rows * cell_h))
                c0 += n_cols
            r0 += n_rows
        return blocks

    def area_for():
        """Area (en pixeles del atlas) del contenido seleccionado."""
        total = 0
        for entity in entities:
            if entity.omitted:
                continue
            for layer in entity.layers:
                if layer.omitted:
                    continue
                cell_w, cell_h = scaled_cell(layer)
                cols, rows = used_grid(layer)
                total += cols * rows * cell_w * cell_h
        return total

    dropped_entities = 0
    dropped_layers = 0

    def rank_entities():
        """Entidades candidatas a descartarse, de menor a mayor valor."""
        candidates = []
        for entity in entities:
            if entity.omitted:
                continue
            area = 0
            main_prio = DEFAULT_DRAW_ORDER
            main_area = 0
            for layer in entity.layers:
                if layer.omitted:
                    continue
                area += (layer.geometry.sheet_width *
                         layer.geometry.sheet_height) * args.scale * args.scale
                # la capa principal (la de mayor area) define la prioridad real
                # de la entidad: el reflejo o las luces de conexion no deben
                # decidir el destino de la entidad entera
                if layer.geometry.frame_pixels > main_area:
                    main_area = layer.geometry.frame_pixels
                    main_prio = PRIORITY_DRAW_ORDER.get(layer.priority,
                                                        DEFAULT_DRAW_ORDER)
            if not area:
                continue
            if entity.name in PROTECTED_ENTITIES:
                tier = 0                       # imprescindible: nunca se descarta
            elif entity.ptype in CORE_ENTITY_TYPES:
                tier = 1                       # maquinaria de fabrica
            else:
                tier = 2                       # prescindible (efectos, enemigos)
            candidates.append((tier, main_prio, -area, entity.name, entity))
        candidates.sort(key=lambda t: (t[0], t[1], t[2], t[3]))
        return candidates

    def drop_entity(entity):
        entity.omitted = "atlas-pleno"
        for layer in entity.layers:
            layer.omitted = "atlas-pleno"

    # 1) descarte por estimacion de area (sin empaquetar).  El skyline
    #    aprovecha ~78% de la pagina, asi que se estima con ese margen.
    while True:
        droppable = [c for c in rank_entities() if c[0] > 0]
        if not droppable:
            break
        if area_for() <= budget * 0.78:
            break
        victim = droppable[0][4]
        drop_entity(victim)
        dropped_entities += 1
        dropped_layers += len(victim.layers)

    # 2) empaquetado real en paginas
    def pack_all():
        packer = PagePacker(page_size)
        # se recoge todo y se ordena por altura: empaquetar primero los
        # bloques mas altos deja los huecos Pequenios para los pequenos y
        # mejora mucho el aprovechamiento de las paginas
        pending = []
        for entity in entities:
            if entity.omitted:
                continue
            for layer in entity.layers:
                if layer.omitted:
                    continue
                layer.blocks = []
                for (c0, r0, n_cols, n_rows, bw, bh) in blocks_of(layer):
                    pending.append((bh, bw, entity, layer, c0, r0, n_cols, n_rows))
        pending.sort(key=lambda t: (-t[0], -t[1], t[2].name, t[3].index))
        for (_bh, _bw, entity, layer, c0, r0, n_cols, n_rows) in pending:
            if entity.omitted or layer.omitted:
                continue
            key = (entity.name, layer.index, c0, r0)
            cell_w, cell_h = scaled_cell(layer)
            placed = packer.add(key, n_cols * cell_w, n_rows * cell_h)
            if placed is None:
                layer.omitted = "atlas-corta"
                layer.blocks = []
                continue
            page, px, py = placed
            layer.blocks.append({
                "page": page, "atlas_x": px, "atlas_y": py,
                "width": n_cols * cell_w,
                "height": n_rows * cell_h,
                "first_col": c0, "first_row": r0,
                "cols": n_cols, "rows": n_rows,
            })
        for entity in entities:
            for layer in entity.layers:
                layer.blocks.sort(key=lambda b: (b["first_row"], b["first_col"]))
        return packer

    packer = pack_all()
    while packer.page_count() > max_pages:
        droppable = [c for c in rank_entities() if c[0] > 0]
        if not droppable:
            break
        victim = droppable[0][4]
        drop_entity(victim)
        dropped_entities += 1
        dropped_layers += len(victim.layers)
        packer = pack_all()

    # asercion: ninguna pagina puede pasar de page_size en ningun eje, o la
    # GPU no podria subirla (GL_MAX_TEXTURE_SIZE)
    for index, page in enumerate(packer.pages()):
        assert page["width"] <= page_size, (
            "la pagina %d mide %d px de ancho (maximo %d)"
            % (index, page["width"], page_size))
        assert page["height"] <= page_size, (
            "la pagina %d mide %d px de alto (maximo %d)"
            % (index, page["height"], page_size))
        for key, (x, y) in page["placements"].items():
            w, h = page["sizes"][key]
            assert 0 <= x and 0 <= y, "posicion negativa en la pagina %d" % index
            assert x + w <= page_size, \
                "un rect se sale de la pagina %d" % index
            assert y + h <= page_size, \
                "un rect se sale de la pagina %d" % index

    if packer.page_count() == 0:
        sys.stderr.write("ERROR: no se pudo colocar ninguna hoja en el atlas\n")
        return 1
    if packer.page_count() > max_pages:
        sys.stderr.write("ERROR: hacen falta %d paginas de %d px (maximo %d). "
                         "Sube --max-pages o usa --types para filtrar.\n"
                         % (packer.page_count(), page_size, max_pages))
        return 1

    # --- render de las paginas --------------------------------------------
    page_count = packer.page_count()
    pages = [bytearray(page_size * page_size * 4) for _ in range(page_count)]

    for entity in entities:
        for layer in entity.layers:
            layer.grid_cols, layer.grid_rows = used_grid(layer)

    for entity in entities:
        if entity.omitted:
            continue
        for layer in entity.layers:
            if layer.omitted:
                continue
            geo = layer.geometry
            if not layer.blocks:
                continue
            wanted = set()
            for blk in layer.blocks:
                src_y0 = blk["first_row"] * geo.frame_height
                src_h = blk["rows"] * geo.frame_height
                for r in range(src_y0, src_y0 + src_h):
                    wanted.add(r)
            try:
                rows = dict(decode_png_rows(layer.abs_path, wanted))
            except (PngError, OSError) as exc:
                warnings.append("png: %s (%s)" %
                                (os.path.relpath(layer.abs_path, data_dir), exc))
                layer.omitted = "decode-error"
                continue
            for blk in layer.blocks:
                page = pages[blk["page"]]
                src_x0 = blk["first_col"] * geo.frame_width
                src_y0 = blk["first_row"] * geo.frame_height
                src_w = blk["cols"] * geo.frame_width
                src_h = blk["rows"] * geo.frame_height
                dest_w = blk["width"]
                dest_h = blk["height"]
                cell_w, cell_h = scaled_cell(layer)
                exact_2x = (geo.frame_width == 2 * cell_w and
                            geo.frame_height == 2 * cell_h)
                if exact_2x:
                    # camino rapido: filtro de caja 2x2 exacto
                    for r in range(dest_h):
                        sy = src_y0 + 2 * r
                        src = rows.get(sy)
                        if src is None:
                            continue
                        block_row = src[src_x0 * 4:(src_x0 + src_w) * 4]
                        if len(block_row) < src_w * 4:
                            continue
                        other = rows.get(sy + 1)
                        block_row2 = None
                        if other is not None:
                            block_row2 = other[src_x0 * 4:(src_x0 + src_w) * 4]
                        line = downscale_2x2(block_row, block_row2, dest_w)
                        dst_off = ((blk["atlas_y"] + r) * page_size +
                                   blk["atlas_x"]) * 4
                        page[dst_off:dst_off + len(line)] = line
                else:
                    # camino general: promedia las filas y columnas
                    lines = resample_block_generic(rows, src_x0, src_y0,
                                                  src_w, src_h, dest_w, dest_h)
                    for r, line in enumerate(lines):
                        dst_off = ((blk["atlas_y"] + r) * page_size +
                                   blk["atlas_x"]) * 4
                        page[dst_off:dst_off + len(line)] = line
            # frames: un rectangulo por celda de la rejilla, en orden de fila
            fw, fh = geo.frame_width, geo.frame_height
            cell_w, cell_h = scaled_cell(layer)
            grid_cols, grid_rows = used_grid(layer)
            layer.grid_cols = grid_cols
            layer.grid_rows = grid_rows
            for row in range(grid_rows):
                for col in range(grid_cols):
                    blk = None
                    for candidate in layer.blocks:
                        if (candidate["first_col"] <= col <
                                candidate["first_col"] + candidate["cols"] and
                                candidate["first_row"] <= row <
                                candidate["first_row"] + candidate["rows"]):
                            blk = candidate
                            break
                    if blk is None:
                        continue
                    x = blk["atlas_x"] + (col - blk["first_col"]) * cell_w
                    y = blk["atlas_y"] + (row - blk["first_row"]) * cell_h
                    layer.frames.append({
                        "index": len(layer.frames),
                        "page": blk["page"],
                        "x": x,
                        "y": y,
                        "w": cell_w,
                        "h": cell_h,
                        "sheet_x": geo.offset_x + col * fw,
                        "sheet_y": geo.offset_y + row * fh,
                    })

    for entity in entities:
        for layer in entity.layers:
            layer.frames.sort(key=lambda f: f["index"])
            layer.z_order = layer.index
            # una capa sin frames tiene que estar marcada SIEMPRE: el motor
            # la toma como dibujable si no ve el flag
            if not layer.frames and not layer.omitted:
                layer.omitted = "sin-frames"

    # --- escritura de las paginas ----------------------------------------
    # Cada pagina se escribe en RAW (el motor Android no decodifica PNG) y
    # tambien en PNG, que sirve para inspeccion y para las texturas que no
    # van en el atlas.
    raw_files = []
    png_files = []
    pages_info = []
    total_raw = 0
    for index, page in enumerate(pages):
        base = "atlas_%d" % index
        raw_path = os.path.join(out_dir, base + ".rgba")
        png_path = os.path.join(out_dir, base + ".png")
        if not args.no_raw:
            try:
                with open(raw_path, "wb") as fh:
                    fh.write(page)
            except OSError as exc:
                sys.stderr.write("ERROR: no se pudo escribir %s: %s\\n"
                                 % (raw_path, exc))
                return 1
            raw_files.append(base + ".rgba")
            total_raw += len(page)
        try:
            with open(png_path, "wb") as fh:
                fh.write(png_encode_rgba(page_size, page_size, page))
        except (OSError, PngError) as exc:
            sys.stderr.write("ERROR: no se pudo escribir %s: %s\\n"
                             % (png_path, exc))
            return 1
        png_files.append(base + ".png")
        pages_info.append({
            "file": base + (".rgba" if not args.no_raw else ".png"),
            "width": page_size,
            "height": page_size,
        })

    # --- manifest --------------------------------------------------------
    atlas_info = {
        "page_size": page_size,
        "width": page_size,
        "height": page_size,
        "page_count": page_count,
        "pixel_format": "RGBA8",
        "scale": args.scale,
        "total_pixels": page_size * page_size * page_count,
        "pages": pages_info,
    }
    if not args.no_raw:
        atlas_info["raw_files"] = raw_files
    atlas_info["png_files"] = png_files
    manifest = {
        "version": version,
        "graphics_layout": layout,
        "atlas": atlas_info,
        "entities": {},
        "icons": {},
        "sounds": {},
        "terrain": {},
    }

    for entity in entities:
        entry = {
            "name": entity.name,
            "type": entity.ptype,
            "layers": [],
            "draw_order": entity.draw_order,
        }
        if entity.variants:
            entry["variants"] = dict(sorted(entity.variants.items()))
        if entity.flipped:
            entry["flipped"] = dict(sorted(entity.flipped.items()))
        if entity.omitted:
            entry["omitted"] = entity.omitted
        for layer in entity.layers:
            geo = layer.geometry
            layer_entry = {
                "filename": layer.rel_path,
                "priority": layer.priority,
                # tamano LOGICO del frame (no se reescala): el motor lo usa
                # para calcular el tamano en el mundo
                "frame_width": geo.frame_width,
                "frame_height": geo.frame_height,
                # frames totales de la capa (los que hay en el atlas)
                "frame_count": len(layer.frames),
                "source_frame_count": layer.frame_count,
                "direction_count": layer.direction_count,
                "animation_frames": layer.animation_frames,
                "animation": {"frames": geo.anim_frames, "speed": layer.anim_speed},
                "cols": layer.grid_cols,
                "rows": layer.grid_rows,
                "source_cols": geo.cols,
                "source_rows": geo.rows,
                "layer_order": layer.index,
                "z_order": layer.z_order,
                "is_body": layer.is_body,
                # las coordenadas de los rects son absolutas dentro de su
                # pagina, asi que el origen de la capa se deja a 0
                "atlas_x": 0,
                "atlas_y": 0,
                "page": layer.blocks[0]["page"] if layer.blocks else 0,
                "frames": layer.frames,
            }
            if len(layer.blocks) > 1:
                layer_entry["blocks"] = layer.blocks
                layer_entry["pages"] = sorted(set(b["page"] for b in layer.blocks))
            if geo.grid_mismatch:
                layer_entry["grid_mismatch"] = True
            if layer.flags:
                layer_entry["flags"] = layer.flags
            if layer.shift is not None:
                layer_entry["shift"] = layer.shift
            if layer.scale is not None:
                layer_entry["scale"] = layer.scale
            if layer.omitted:
                layer_entry["omitted"] = layer.omitted
            entry["layers"].append(layer_entry)
        manifest["entities"][entity.name] = entry

    # --- iconos ----------------------------------------------------------
    icons_dir = os.path.join(out_dir, "icons")
    icon_count = 0
    if not args.skip_icons:
        src_icons = os.path.join(data_dir, "base", "graphics", "icons")
        if os.path.isdir(src_icons):
            os.makedirs(icons_dir, exist_ok=True)
            for filename in sorted(os.listdir(src_icons)):
                if not filename.lower().endswith(".png"):
                    continue
                src = os.path.join(src_icons, filename)
                if not os.path.isfile(src):
                    continue
                dst_name = filename
                shutil.copyfile(src, os.path.join(icons_dir, dst_name))
                manifest["icons"][filename[:-4]] = "icons/" + dst_name
                icon_count += 1
        else:
            warnings.append("no se encontro base/graphics/icons/")

    # --- terreno ---------------------------------------------------------
    terrain_count = 0
    if not args.skip_terrain:
        src_terrain = os.path.join(data_dir, "base", "graphics", "terrain")
        if os.path.isdir(src_terrain):
            dst_terrain = os.path.join(out_dir, "terrain")
            os.makedirs(dst_terrain, exist_ok=True)
            for filename in sorted(os.listdir(src_terrain)):
                if not filename.lower().endswith(".png"):
                    continue
                src = os.path.join(src_terrain, filename)
                if not os.path.isfile(src):
                    continue
                shutil.copyfile(src, os.path.join(dst_terrain, filename))
                manifest["terrain"][filename[:-4]] = "terrain/" + filename
                terrain_count += 1
            _empaquetar_atlas_terreno(src_terrain, out_dir, manifest, args)

    # --- sonidos ----------------------------------------------------------
    sound_count = 0
    if not args.skip_sounds:
        manifest["sounds"] = dict(sorted(list_sounds(data_dir).items()))
        sound_count = len(manifest["sounds"])

    # --- strings ---------------------------------------------------------
    string_count = 0
    if not args.skip_locale:
        cfg = os.path.join(data_dir, "base", "locale", "en", "base.cfg")
        if os.path.isfile(cfg):
            strings = parse_locale(cfg)
            with open(os.path.join(out_dir, "strings.json"), "w",
                      encoding="utf-8") as fh:
                json.dump(strings, fh, ensure_ascii=False, indent=1, sort_keys=True)
            string_count = len(strings)
        else:
            warnings.append("no se encontro base/locale/en/base.cfg")

    # --- manifest.json ---------------------------------------------------
    with open(os.path.join(out_dir, "manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=1, sort_keys=True)

    # --- resumen ---------------------------------------------------------
    total_layers = sum(len(e.layers) for e in entities)
    total_frames = sum(len(l.frames) for e in entities for l in e.layers)
    included_layers = sum(1 for e in entities for l in e.layers
                          if not l.omitted)
    split_layers = sum(1 for e in entities for l in e.layers
                       if len(l.blocks) > 1)
    mismatch_layers = sum(1 for e in entities for l in e.layers
                          if l.geometry.grid_mismatch)
    png_total = sum(os.path.getsize(os.path.join(out_dir, name))
                    for name in png_files)
    if not args.quiet:
        print("=== Extraccion de assets de Factorio %s ===" % version)
        print("Datos      : %s" % data_dir)
        print("Salida     : %s" % out_dir)
        print("Layout     : sprites %s" % layout)
        print("Entidades  : %d (con sprite)  |  %d sin sprite local"
              % (len(entities), len(no_sprite)))
        print("Capas      : %d (en el atlas: %d)" % (total_layers, included_layers))
        print("Frames     : %d celdas (de %d disponibles en las hojas)"
              % (total_frames, total_cells))
        print("Atlas      : %d paginas de %dx%d px" %
              (page_count, page_size, page_size))
        print("             atlas_N.png  %.1f MB en total" % (png_total / 1e6))
        if args.no_raw:
            print("             atlas_N.rgba omitido (--no-raw)")
        else:
            print("             atlas_N.rgba %.1f MB en total (RGBA8 crudo)"
                  % (total_raw / 1e6))
        if split_layers:
            print("             %d capas troceadas en varios bloques" % split_layers)
        if mismatch_layers:
            print("             %d capas con rejilla no exacta (grid_mismatch)"
                  % mismatch_layers)
        if dropped_entities:
            print("Descartadas: %d entidades (%d capas) por falta de espacio en "
                  "el atlas" % (dropped_entities, dropped_layers))
        print("Iconos     : %d -> %s" % (icon_count, os.path.join(out_dir, "icons")))
        print("Terreno    : %d -> %s" % (terrain_count, os.path.join(out_dir, "terrain")))
        if manifest.get("terrain_atlas"):
            ta = manifest["terrain_atlas"]
            n_tiles = sum(len(v["variants"]) for v in ta["tiles"].values())
            print("Suelo      : %d celdas de %d px en %d paginas de %dx%d "
                  "(%d tipos de terreno)"
                  % (n_tiles, ta["tile_size"], len(ta["pages"]),
                     ta["page_size"], ta["page_size"], len(ta["tiles"])))
        print("Textos     : %d -> %s" % (string_count, os.path.join(out_dir, "strings.json")))
        print("Sonidos    : %d (en manifest.json)" % sound_count)
        print("Manifest   : %s" % os.path.join(out_dir, "manifest.json"))
        if missing:
            print("AVISO      : %d PNG referenciados no existen:" % len(missing))
            for ename, ref in missing[:10]:
                print("             - %s (en %s)" % (ref, ename))
            if len(missing) > 10:
                print("             ... y %d mas" % (len(missing) - 10))
        if warnings:
            print("Avisos     : %d" % len(warnings))
            for warning in warnings[:10]:
                print("             - %s" % warning)
            if len(warnings) > 10:
                print("             ... y %d mas" % (len(warnings) - 10))

    if missing and args.strict:
        sys.stderr.write("ERROR: %d PNG referenciados no existen (modo --strict)\n"
                         % len(missing))
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
