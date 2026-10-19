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
# instalacion 2.1.22 con graficos (fuente principal)
python3 scripts/extract_factorio_assets.py \
    --data /Volumes/X10Pro/SteamLibrary/steamapps/common/Factorio/factorio.app/Contents/data \
    --out build/factorio_assets

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
| `--version` | - | detecta y reporta la version de Factorio |
| `--atlas-size N` | `2048` | ancho maximo del atlas en pixeles |
| `--max-atlas-height N` | `12288` | alto maximo del atlas en pixeles |
| `--max-frames-per-layer N` | `0` (automatico) | frames por capa; 0 = el que quepa |
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
  atlas.png          atlas RGBA8 con los frames de todos los sprites
  manifest.json      indice de entidades, capas, frames, iconos y sonidos
  icons/<nombre>.png iconos de items copiados tal cual
  terrain/<n>.png    tiles de terreno copiados tal cual
  strings.json       texto EN del juego
```

El resumen que imprime el script dice cuantos tipos, cuantos frames, el tamano
del atlas y donde esta cada salida.

---

## 3. Formato de `manifest.json`

```jsonc
{
  "version": "2.1.22",
  "graphics_layout": "2.1",
  "atlas": { "file": "atlas.png", "width": 2048, "height": 12282 },
  "entities": {
    "assembling-machine-1": {
      "name": "assembling-machine-1",
      "type": "assembling-machine",
      "draw_order": 4,
      "layers": [
        {
          "filename": "base/graphics/entity/assembling-machine-1/assembling-machine-1-base.png",
          "priority": "high",
          "frame_width": 198,
          "frame_height": 184,
          "frame_count": 1,
          "direction_count": 1,
          "animation": { "frames": 64, "speed": 0.0 },
          "frames": [ { "index": 0, "x": 0, "y": 0, "w": 198, "h": 184 } ],
          "flags": [], "shift": [0, 4], "scale": 0.5
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

Una entidad puede tener **varias capas** (base, anim, shadow, status-light,
lights, connection, reflection...). Cada capa tiene su propio PNG y su propia
geometria de frames:

- `filename`: ruta del PNG **relativa al directorio `data/`**
  (`base/graphics/...`, `core/graphics/...`).
- `priority`: la prioridad de dibujado de Factorio.
- `frame_width` / `frame_height`: tamano en pixeles de **un** frame.
- `frame_count`: frames de la animacion por direccion.
- `direction_count`: variantes direccionales (1 si no es direccional).
- `animation.frames` / `animation.speed`: frames que se mueven y a que velocidad.
- `frames`: lista de rectangulos `{index, x, y, w, h}` **dentro de `atlas.png`**.
  `index` es el indice de frame dentro de la tira; el rectangulo dice donde
  copiarlo de la textura del atlas.

### `draw_order`

Se deriva de `priority`, para poder ordenar el render en el motor:

| priority | draw_order |
|---|---|
| `terrain-low` | 0 |
| `terrain` | 1 |
| `low` | 2 |
| `normal` | 3 |
| `high` | 4 |
| `train` | 5 |
| `extra-high` | 6 |
| `extra-high-no-glow` | 7 |
| `ghost` | 8 |
| `selection-box` | 9 |
| `highlight` | 10 |
| `alert-icon` | 11 |

Una entidad sin `priority` usa `normal` (3). El `draw_order` de la entidad es
el de su capa principal (la de mayor area).

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
es exactamente lo que Factorio llama *sprite layer*.

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
4. Los frames se recorren **por filas** (de arriba abajo y de izquierda a
   derecha), que es como Factorio los guarda.

Ejemplo real (2.1.22): `assembling-machine-1-anim.png` mide 1120x1264 y su
tira declara `width = 140`, `height = 158`, `line_length = 8` -> 8 columnas x
8 filas = 64 frames de 140x158.

---

## 5. Limitaciones y decisiones

### El atlas no cabe entero (importante)

El arte completo de Factorio 2.1 son **~3.000 millones de pixeles** (unos
2.500 PNG en `base/graphics/entity/`), y un PNG esta limitado a 65535 px de
alto. Un unico atlas de 2048 de ancho solo puede guardar ~134 Mpx, es decir
**menos del 5%** del arte real.

Por eso el extractor:

1. Elige la mayor cantidad de frames por capa que aun quepa en el atlas
   (`--max-frames-per-layer`; por defecto automatico).
2. Si aun asi no cabe, **descarta entidades enteras**, empezando por las menos
   importantes: primero las prescindibles (explosiones, proyectiles, humo,
   enemigos), luego la maquinaria de fabrica, y **nunca** las entidades
   marcadas como imprescindibles (`PROTECTED_ENTITIES` en el script: las 11
   entidades basicas de fabrica mas la maquinaria mas comun).
3. Lo reporta en el resumen: `Descartadas: N entidades (M capas)`.

Lo que queda en el manifest siempre esta completo: una entidad esta **entera**
en el atlas o marcada con `"omitted": "atlas-pleno"`.

Para un atlas pequeno y rapido (solo lo que se usa en una partida) conviene:

```bash
python3 scripts/extract_factorio_assets.py --data <dir> --out <salida> \
    --types assembling-machine-1,transport-belt,inserter,stone-furnace
```

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

Para extraer sprites hay que usar una instalacion **con graficos**.

### Otras cosas que conviene saber

- Los sprites de 2.1 tienen **variantes de calidad**
  (`locomotive-1.png` .. `locomotive-8.png`). Si el prototipo cita el nombre
  base y ese fichero no existe, se busca la variante y se avisa.
- `strings.json` contiene **todo** el locale en ingles (no solo las categorias
  que interesan).
- Los iconos en subdirectorios de `base/graphics/icons/` (flechas, categorias
  de municion) **no** se copian: el manifest solo lista los iconos de primer
  nivel.
- Los tiles de terreno son PNG de 4096x4096 (~4-9 MB cada uno): se copian tal
  cual y ocupan unos 75 MB. Se pueden saltar con `--skip-terrain`.
- `sounds` lista los `.ogg` del juego como
  `{"<ruta sin extension>": "ruta/con/extension.ogg"}`; el motor los carga
  desde la instalacion de Factorio.

---

## 6. Tests

```bash
python3 -m unittest discover -s scripts/tests -v
```

25 tests, solo stdlib y **sin datos externos** (todos los fixtures se generan
en memoria o en un directorio temporal):

- **Parser de Lua**: tablas, `frames` numerico y como tabla, funciones que
  devuelven tabla, `util.sprite_load`, `util.by_pixel`, `data:extend`,
  concatenacion, comentarios.
- **Geometria de frames**: rejilla deducida del PNG, `width_in_frames`,
  `direction_count`.
- **PNG**: ida y vuelta encoder -> decoder comprobando pixeles, CRC32 de cada
  chunk, PNG RGB sin alpha, rechazo del entrelazado.
- **Atlas**: los rectangulos no se solapan y caben en el ancho maximo.
- **Locale**: secciones, claves con punto y serializacion a JSON.
- **Varios**: version detectada, rutas de assets, tabla de prioridades.
