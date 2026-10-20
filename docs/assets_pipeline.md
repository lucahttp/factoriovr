# Pipeline de assets de Factorio para el motor VR

Este pipeline extrae los assets **reales** del juego (sprites de las entidades,
iconos de items, textos en ingles y sonidos) de una instalacion de Factorio y
los deja listos para que el motor VR (`src/`) los consuma.

La fuente de verdad es el directorio `data/` de una instalacion de Factorio.
El mod del proyecto declara `factorio_version: 2.1`, asi que la fuente
recomendada es una instalacion **2.1.x con graficos** (la misma version que
usa el servidor headless ARM64 del visor).

---

## 1. Como ejecutarlo

```bash
# lo que consume el motor VR: solo los tipos de la fabrica, a media
# resolucion, sin sombras ni luces -> 5 paginas de 2048x2048 (84 MB)
python3 scripts/extract_factorio_assets.py \
    --data /Volumes/X10Pro/SteamLibrary/steamapps/common/Factorio/factorio.app/Contents/data \
    --out build/factorio_assets --only-listed --scale 0.5 --capas cuerpo

# equivalente, con la lista explicita de tipos
python3 scripts/extract_factorio_assets.py --data <dir_data> --out <salida> \
    --types "assembling-machine-1,...,stone,crude-oil,tree" \
    --scale 0.5 --capas cuerpo

# todos los sprites del juego, a resolucion completa
python3 scripts/extract_factorio_assets.py --data <dir_data> --out <salida>

# solo informar de la version detectada
python3 scripts/extract_factorio_assets.py --data <dir_data> --version
```

No hace falta instalar nada: el script solo usa la biblioteca estandar de
Python 3 (no usa Pillow, ni numpy, ni red).

### Opciones

| Opcion | Por defecto | Que hace |
|---|---|---|
| `--data DIR` | (obligatorio) | directorio `data/` de la instalacion de Factorio |
| `--out DIR` | (obligatorio salvo con `--version`) | directorio de salida |
| `--types a,b,c` | todas | filtra por nombre o por tipo de entidad |
| `--only-listed` | - | extrae solo los tipos que el motor VR dibuja |
| `--version` | - | detecta y reporta la version de Factorio |
| `--scale N` | `0.5` | escala de los pixeles del atlas (no del frame logico) |
| `--capas cuerpo\|todas` | `cuerpo` | que capas meter: cuerpo y animacion, o todas |
| `--page-size N` | `2048` | lado de cada pagina del atlas (limite de OpenGL ES) |
| `--atlas-size N` | `2048` | ancho maximo del atlas en pixeles |
| `--max-pages N` | `32` | numero maximo de paginas del atlas |
| `--frames-per-layer N` | `0` (todos) | frames por capa; recorta el atlas |
| `--no-raw` | - | no escribir los `atlas_N.rgba` |
| `--skip-icons` | - | no copiar los iconos de items |
| `--skip-locale` | - | no extraer `strings.json` |
| `--skip-sounds` | - | no listar los sonidos |
| `--skip-terrain` | - | no copiar los tiles de terreno |
| `--strict` | - | sale con codigo 2 si falta algun PNG referenciado |
| `--quiet` | - | silencia el resumen |

### Codigos de salida

| Codigo | Significado |
|---|---|
| `0` | todo correcto |
| `1` | error fatal: no existe `--data`, no parece Factorio, no hay prototipos, **o no se encontro ni un PNG de sprite** (instalacion headless) |
| `2` | modo `--strict`: faltaba algun PNG referenciado |

---

## 2. Que produce

```
<out>/
  atlas_0.rgba, atlas_1.rgba, ...   paginas del atlas en RGBA8 crudo
  atlas_0.png,  atlas_1.png,  ...   las mismas paginas en PNG (inspeccion)
  manifest.json                     indice de entidades, capas, frames, iconos y sonidos
  icons/<nombre>.png                iconos de items copiados tal cual
  terrain/<n>.png                   tiles de terreno copiados tal cual
  strings.json                      texto EN del juego
```

### Por que paginas

OpenGL ES garantiza `GL_MAX_TEXTURE_SIZE = 2048` y en Adreno suele ser 4096.
Una textura mas alta que eso **no se puede subir a la GPU**, asi que el atlas
se parte en paginas de `--page-size` (2048 por defecto). Cada pagina es un
`atlas_N.rgba` de exactamente `page_size * page_size * 4` bytes, listo para
subirlo tal cual a la GPU sin decodificar nada (el motor Android no tiene
decodificador de PNG).

Los `atlas_N.png` se escriben tambien: sirven para inspeccionar el resultado
y para las texturas que **no** van en el atlas (iconos, terreno).

---

## 3. Formato de `manifest.json` (el que consume el motor)

```jsonc
{
  "version": "2.1.22",
  "graphics_layout": "2.1",
  "atlas": {
    "page_size": 2048,
    "width": 2048,
    "height": 2048,
    "page_count": 5,
    "pixel_format": "RGBA8",
    "scale": 0.5,
    "total_pixels": 20971520,
    "raw_files": ["atlas_0.rgba", "atlas_1.rgba", "..."],
    "png_files": ["atlas_0.png", "atlas_1.png", "..."],
    "pages": [
      { "file": "atlas_0.rgba", "width": 2048, "height": 2048 },
      { "file": "atlas_1.rgba", "width": 2048, "height": 2048 }
    ]
  },
  "entities": {
    "assembling-machine-1": {
      "name": "assembling-machine-1",
      "type": "assembling-machine",
      "draw_order": 4,
      "variants": { "N": 0, "E": 1, "S": 2, "W": 3 },
      "layers": [
        {
          "filename": "base/graphics/entity/assembling-machine-1/assembling-machine-1-base.png",
          "priority": "high",
          "frame_width": 198,          // tamano LOGICO (no se reescala)
          "frame_height": 184,
          "frame_count": 1,            // frames totales de la capa
          "source_frame_count": 1,     // el valor que declara Factorio
          "direction_count": 1,
          "animation_frames": 1,       // frames de animacion por direccion
          "animation": { "frames": 64, "speed": 0.0 },
          "cols": 1, "rows": 1,
          "source_cols": 1, "source_rows": 1,
          "layer_order": 0, "z_order": 0,
          "is_body": true,
          "scale": 0.5,                // escala del sprite en el juego
          "page": 4,                   // pagina de la capa
          "atlas_x": 0, "atlas_y": 0,  // origen (0: los rects son absolutos)
          "frames": [
            { "index": 0, "page": 4, "x": 635, "y": 444, "w": 99, "h": 92,
              "sheet_x": 0, "sheet_y": 0 }
          ]
        }
      ]
    }
  },
  "icons":  { "iron-gear-wheel": "icons/iron-gear-wheel.png" },
  "sounds": { "base/sound/accumulator-idle": "base/sound/accumulator-idle.ogg" },
  "terrain": { "grass-1": "terrain/grass-1.png" }
}
```

### Capas

- `filename`: ruta del PNG **relativa al directorio `data/`**.
- `frame_width` / `frame_height`: tamano **logico** del frame. **No** se
  reescala aunque el atlas este a `--scale`: el motor lo usa para calcular el
  tamano en el mundo (`celdas = frame_width * scale / 32`).
- `cols` / `rows`: la rejilla de la capa. **siempre `len(frames) == cols*rows`**.
- `frame_count`: los frames que hay en el atlas (los de `frames`).
- `source_cols` / `source_rows` / `source_frame_count`: la hoja original completa.
- `direction_count`: variantes direccionales (1 si no es direccional).
- `animation_frames`: frames de animacion **por direccion**.
- `animation.frames` / `animation.speed`: frames que se mueven y a que velocidad.
- `layer_order` / `z_order`: el orden original de la capa dentro de la entidad.
  El motor las dibuja en ese orden (base, anim, shadow, luces...).
- `is_body`: `true` en la capa que define el cuerpo de la entidad. El motor
  elige esa; si ninguna lo tiene, la primera con frames.
- `frames`: un rectangulo **por celda de la rejilla**, en orden de fila. Cada
  rect dice en que **pagina** esta y donde dentro de ella:
  - `page`: indice de la pagina del atlas.
  - `x`, `y`: posicion **absoluta dentro de su pagina** (el motor suma
    `atlas_x`/`atlas_y` de la capa, que se dejan a 0).
  - `w`, `h`: tamano **real** del frame dentro del atlas (ya reescalado).
  - `sheet_x`, `sheet_y`: posicion del frame dentro de la hoja original.

Una capa **sin frames** va siempre marcada con `"omitted": "..."`; el motor la
salta y usa la siguiente con frames.

### `variants` y `flipped`

Una entidad puede tener varias capas de cuerpo, una por direccion (por ejemplo
la perforadora: `-N`, `-E`, `-S`, `-W`). La entidad lleva:

- `variants`: `{ "N": <indice de capa>, "E": ..., ... }` -> que capa usar en cada
  direccion.
- `flipped`: idem, para las variantes reflejadas (`-N-flipped.png`). Con
  `--capas cuerpo` las reflejadas se descartan (el motor puede espejar la
  normal).

### `draw_order`

Se deriva de `priority`:

| priority | draw_order |
|---|---|
| `extra-low` | 0 |
| `low` | 1 |
| `terrain` | 2 |
| `terrain-high` | 3 |
| `object` (y `normal`) | 4 |
| `high` | 5 |
| `extra-high` | 6 |

El `draw_order` de la entidad es el de su capa principal (la de mayor area).

### `strings.json`

Objeto plano `{clave: texto}`. La clave es `"<seccion>.<clave>"` de
`base/locale/en/base.cfg`, tal cual (las claves que ya tienen punto se
guardan sin tocar):

```json
{ "item-name.iron-gear-wheel": "Iron gear wheel",
  "entity-name.assembling-machine-1": "Assembling machine 1" }
```

---

## 4. Como se resuelven los sprites

El extractor incluye un **parser de un subconjunto de Lua** (solo stdlib) que
lee `base/prototypes/entity/*.lua`. Soporta cadenas, numeros, booleanos, `nil`,
tablas (con claves con o sin comillas), funciones, `require(...)`,
`data:extend{...}`, `util.sprite_load(...)`, `util.by_pixel(...)` y funciones de
usuario que devuelven una tabla (`gun_turret_extension{...}`).

Despues recorre el prototipo buscando toda tabla con un `filename` `.png`, que
es exactamente lo que Factorio llama *sprite layer*. Como el recorrido es
recursivo, cubre las tres formas que usa Factorio 2.1:

- `graphics_set = { base = {...}, animation = {...}, shadow = {...} }`
- `graphics_set = { animation = { layers = { ... } } }` (la lista se expande)
- `sprite = {...}` / `sprites = { ... }` (estilo antiguo)

El **cuerpo** de la entidad (`is_body`) es la primera capa de dibujo (ni
sombra, ni luz, ni reflejo) que vive dentro del contenedor principal de
graficos (`graphics_set`, `pictures`, `sprite`...). Asi la ensambladora marca
`assembling-machine-1-base.png` (198x184) y no la de la conexion electrica.

### Las dos estructuras de Factorio

| | 2.0.x | 2.1.x |
|---|---|---|
| ficheros | `base/graphics/entity/<tipo>/<tipo>.png` | `<tipo>-<capa>.png` + `<tipo>-<capa>.lua` |
| referencia en el prototipo | `filename = "...png"` | `util.sprite_load(".../base", { ... })` |
| geometria | en el prototipo: `width`, `height`, `line_length`, `frame_count` | en el `.lua` de tira: `width`, `height`, `line_length` |

Los dos formatos se detectan solos (`graphics_layout` en el manifest) y ambos
funcionan con el mismo codigo.

### Como se deduce la disposicion de los frames

1. El tamano de frame sale de, en este orden: la tabla `frames`
   (`frame_width`/`frame_height`), el `.lua` de tira de 2.1, o el
   `width`/`height`/`size` de la capa.
2. Con el tamano del PNG se calcula la rejilla:
   `columnas = ancho_png / frame_width`, `filas = alto_png / frame_height`.
3. Si la capa declara `width_in_frames`/`height_in_frames` (2.1), mandan esos.
4. Si el PNG no encaja exacto en la rejilla, se usa el `line_length` de la tira
   y la capa se marca con `"grid_mismatch": true` (no bloquea nada).

Ejemplo real (2.1.22): `assembling-machine-1-anim.png` mide 1120x1264 y su
tira declara `width = 140`, `height = 158`, `line_length = 8` -> 8 columnas x
8 filas = 64 frames de 140x158.

---

## 5. Limitaciones y decisiones

### `--scale`: el atlas a media resolucion

El arte de Factorio 2.1 son hojas enormes (la cinta transportadora son 320
frames de 128x128 = 5,2 Mpx) y a resolucion completa el atlas no cabe en un
movil. Con `--scale 0.5` los **pixeles** del atlas se reducen a la mitad (con
filtro de caja 2x2, o el equivalente general cuando el frame tiene ancho impar)
pero el **tamano logico** del frame (`frame_width`/`frame_height`) **no cambia**:
el motor sigue calculando el tamano en el mundo igual, y usa `w`/`h` del rect
(ya reescalados) para las UV.

### `--capas cuerpo`: sin sombras ni luces

Por defecto solo se mete el cuerpo y la animacion. Se descartan las sombras
(el motor puede simularlas con un quad oscuro), las luces de conexion, la luz de
estado, los reflejos y las mascaras, y tambien las variantes `-flipped`
(duplicadas: el motor puede espejar la normal). Con `--capas todas` se meten
todas.

### El atlas no cabe entero

El arte completo son **~1000 millones de pixeles**. El extractor descarta
**entidades enteras** si no caben, empezando por las menos importantes
(efectos, enemigos), nunca las marcadas como imprescindibles
(`PROTECTED_ENTITIES`). Lo reporta en el resumen.

### `--frames-per-layer`: atlas mas pequeno todavia

Recorta las primeras `N` celdas de cada hoja (en orden de fila). La rejilla
sigue siendo rectangular, asi que **`len(frames) == cols*rows` se sigue
cumpliendo**.

Medidas reales con `--only-listed --capas cuerpo` (2.1.22):

| `--scale` | paginas | `.rgba` | `.png` |
|---|---|---|---|
| 1.0 | 31 | 520 MB | 63 MB |
| 0.5 | 5 | 84 MB | 14 MB |

### PNG admitidos

- 8 y 16 bits por canal.
- Canales: gris, gris+alpha, RGB, RGBA y **paleta** (con `tRNS`).
- Tambien 1, 2 y 4 bits (los usa Factorio en fondos de fluidos).
- **Los PNG entrelazados (Adam7) se saltan** con un aviso claro y se sigue con
  el resto. Factorio no los usa.

El decodificador solo decodifica las filas que hacen falta, asi que las hojas
grandes (4096x8192) no ralentizan la extraccion.

### Instalaciones headless

Una instalacion *headless* (la del servidor ARM64) **no tiene los PNG**: solo
los `.lua`. El extractor avisa y sale con codigo `1`:

```
ERROR: no se encontro ningun PNG de sprite en <dir>
       Si es una instalacion headless (sin graficos) los sprites
       no existen: usa una instalacion de Factorio con graficos.
```

### Otras cosas que conviene saber

- Los sprites de 2.1 tienen **variantes de calidad**
  (`locomotive-1.png` .. `locomotive-8.png`). Si el prototipo cita el nombre
  base y ese fichero no existe, se busca la variante y se avisa.
- `strings.json` contiene **todo** el locale en ingles.
- Los iconos en subdirectorios de `base/graphics/icons/` (flechas, categorias
  de municion) **no** se copian: el manifest solo lista los iconos de primer
  nivel.
- Los tiles de terreno son PNG de 4096x4096 (~4-9 MB cada uno): ocupan unos
  75 MB. Se pueden saltar con `--skip-terrain`.
- `sounds` lista los `.ogg` del juego como
  `{"<ruta sin extension>": "ruta/con/extension.ogg"}`.

---

## 6. Tests

```bash
python3 -m unittest discover -s scripts/tests -v
```

35 tests, solo stdlib y **sin datos externos** (todos los fixtures se generan
en memoria o en un directorio temporal):

- **Parser de Lua**: tablas, `frames` numerico y como tabla, funciones que
  devuelven tabla, `util.sprite_load`, `util.by_pixel`, `data:extend`,
  concatenacion, comentarios.
- **Geometria de frames**: rejilla deducida del PNG, `width_in_frames`,
  `direction_count`.
- **PNG**: ida y vuelta encoder -> decoder comprobando pixeles, CRC32 de cada
  chunk, PNG RGB sin alpha, rechazo del entrelazado.
- **Atlas**: los rectangulos no se solapan, caben en el ancho maximo y las
  paginas nunca exceden `page_size`.
- **Reescalado**: el promedio SWAR coincide con el de Python, el 2x2 promedia
  cuatro pixeles y el filtro general promedia la caja correcta.
- **Variantes**: deteccion de direccion (`-N`, `-E`...) y de `-flipped`, y
  que las sombras/luces/reflejos no son el cuerpo de la entidad.
- **Locale**: secciones, claves con punto y serializacion a JSON.
- **Varios**: version detectada, rutas de assets, tabla de prioridades.
