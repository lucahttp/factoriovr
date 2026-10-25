# ADR-021 — El canal de geometría: la fábrica real en 3D sobre la mesa

> **Estado**: diseño cerrado. Es la pieza que convierte el visor de una
> *maqueta* (la reconstrucción con el estado) en **la fábrica de verdad**.
> Revisa: `ADR-019` (la GPU), `ADR-020` (sin RCON).

---

## 1. Qué falta hoy

El visor tiene un renderizador 3D **completo** (terreno con su atlas, sprites
orientados, modelos GLB reales instanciados con una matriz por máquina,
estéreo por ojo, cámara 6DoF). Lo que no tiene es **los datos del motor**:
los carga del snapshot de RCON, que son una aproximación (posiciones
redondeadas a tile, sin animación, sin escala).

El motor, mientras tanto, **ya dibuja** en la GPU del visor (ADR-019) y el
inyector **ya intercepta** sus draw calls (`injector-linux/src/modelo3d.rs`).
Ahí están los números exactos. **El canal no existe: es transporte.**

## 2. Lo que el inyector ya sabe

`modelo3d.rs` captura, por cada entidad visible, un `Caja`:

| Campo | Significado | Unidad |
|---|---|---|
| `wx, wy` | posición en el mundo del motor | tiles (float) |
| `sx, sy` | tamaño del sprite | tiles |
| `direccion` | orientación (0..15) | 16 direcciones |
| `modelo` | índice del modelo 3D en el registro | i32 (−1 = sin modelo) |
| `escala` | píxeles por casilla (zoom del motor) | px/tile |

Y además, por frame: `desplazamiento_camara()` (el desplazamiento de la
cámara respecto al jugador) y `ojo_actual()` (0 = izquierdo, 1 = derecho).

**Y el inyector ya tiene el socket**: `stream.rs` manda los frames al visor
(protocolo XFV1, TCP, el inyector es cliente y el visor servidor, en el
puerto 27020). El mismo socket, otro tipo de mensaje.

## 3. El protocolo: XFV1, tipo 2 = geometría

El protocolo actual (XFV1) es:

```text
  cabecera (16 bytes)
     0..4   magic   u32 = 0x31564658  ("XFV1")
     4..8   numero de frame  u32
     8..10  ancho   u16
    10..12  alto    u16
    12      flags   u8   (bit 0: ojo: 0 = izquierdo, 1 = derecho)
    13      formato u8   (0 = RGBA8888)
    14..16  reservado u16
  carga: ancho*alto*4 bytes de RGBA
```

**Se añade el flag bit 1 (`0x02`) = "este mensaje es geometría, no píxeles"**,
el formato de carga pasa a 1 (GEOMETRIA) y entonces la carga es:

```text
  cabecera: w (u16) = numero de nombres de prototipo
            h (u16) = numero de entidades
            flags    = ojo | 0x02
            formato  = 1
  carga:
    u32  numero de nombres
      por nombre: u8 longitud + bytes ASCII
    u32  numero de entidades
      por entidad (24 bytes):
         f32  wx
         f32  wy
         f32  sx
         f32  sy
         u32  direccion
         u32  modelo
```

 little-endian, igual que el resto del protocolo.

**El origen NO viaja**: las posiciones son absolutas y el visor las resta
del `view_center` (donde está el jugador), igual que hacía con el snapshot.

**La tabla de nombres viaja entera en cada mensaje** (unos 300 bytes): los
dos lados numeran los modelos de forma distinta, así que el visor recibe
el NOMBRE y lo convierte con su propio `id_de_prototipo`. Sin estado que
sincronizar y sin possibility de desajuste.

**Por qué no un puerto nuevo**: el inyector ya está conectado al visor y
tiene el estado de la escena en memoria justo cuando compone el frame.
Un tipo nuevo en el mismo socket evita un segundo ciclo de conexión y
mantiene el orden (píxeles y geometría del mismo frame).

## 4. El lado del visor

El renderizador ya sabe colocar una entidad 3D: `draw_tabletop_scene` pinta
las máquinas con `s_dibujar_modelos_3d` (una matriz por máquina, instanciada
por prototipo GLB). Lo que hay que añadir:

1. **Recibir** el mensaje de geometría en el hilo que ya escucha el 27020
   (`android/src/openxr_engine.cpp`, el que ya valida la cabecera XFV1) y
   volcarlo en una lista propia (`std::vector<GeoEntidad>`), **sin tocar el
   hilo de render**.
2. **Pasarlo a Rust** con un FFI nuevo, `xfactorvr_geometria(n, ...)`,
   que rellene las mismas estructuras que `apply_snapshot` rellena
   (`tile_x`, `tile_y`, `orientacion`, `model_id`).
3. **Render**: para cada entidad, la matriz de la máquina sale de
   `(wx - origen_x, wy - origen_y)` escalado por `escala`, con el yaw de
   `direccion * TAU/16`. **Cada ojo por su lado** (el mismo camino que usa
   hoy el diorama) ⇒ estéreo de verdad.

**El renderer no se reescribe**: se le cambia la fuente de los datos. Es el
mismo truco que el del canal de estado (ADR-020): el parser ya existe.

## 5. Los dos huecos que quedan dentro

### 5.1 El terreno

El terreno **no** lo captura el inyector hoy: es una capa de sprites del
motor, no entidades. Opciones:

- **(a) Capturarlo en el inyector**: el terreno se dibuja con el sprite
  batcher; los hooks de `DrawQueue`/`bindUniformBuffer` ya ven los rects
  del atlas, así que se pueden filtrar los que son del terreno y quedarse
  con (tile, rect del atlas). Es lo que hace la mod 3D de referencia.
- **(b) Dejarlo al snapshot**: el terreno se queda viniendo de RCON (o del
  fichero del ADR-020) mientras las entidades pasan al canal nuevo.

**(b) es el primer paso**: el suelo es "poco" y lo que marea es que las
máquinas estén en su sitio de verdad. (a) es el siguiente.

### 5.2 El sprite 2D y su animación

`Caja` **no** guarda el rect del atlas ni el frame de animación: el inyector
sabe *dónde* y *hacia dónde*, pero no *con qué sprite*. Para el MR con
modelos 3D no hace falta (la máquina es su modelo). Para las entidades sin
modelo ( chests, belts, ítems) el sprite sigue viniendo del snapshot.

**⇒ El primer corte del canal lleva las máquinas con modelo 3D** (que es
justo lo que se ve "mal" hoy: son pegatinas) y deja el resto como está.

## 6. Por qué esto es "lo ultimate" y no un parche

Con el canal de geometría:
- Las máquinas están **donde el motor las dibuja**, no donde el snapshot las
  estima (posición exacta, orientación exacta, escala exacta).
- Se dibujan con el **modelo 3D real** (los GLB), **en estéreo**, sobre la
  mesa, con la cámara en la **cabeza** (6DoF): se puede **rodear la fábrica**
  con la cabeza.
- Y el juego **real** (el cliente gráfico) sigue corriendo aparte: su
  imagen es el panel, y el ratón va a él. Los dos conviven: el panel es el
  HUD de control, la mesa es la fábrica.

## 7. Orden de trabajo

1. **Protocolo**: el tipo 2 en `stream.rs` (emisor) y en
   `openxr_engine.cpp` (receptor) + el FFI `xfactorvr_geometria`.
2. **Prueba mínima**: el inyector envía 10 entidades falsas; el visor las
   coloca. Banco: que aparezcan en la mesa.
3. **Datos reales**: el inyector manda las `Caja` capturadas; el visor las
   usa. Banco: las máquinas coinciden con las del panel del juego real.
4. **Estéreo**: verificar que se ven distintas desde cada ojo.
5. **Terreno** (opción (a) del §5.1) y **sprites sin modelo**.
6. Entonces sí: apagar RCON del camino del mundo (el terreno y el
   inventario seguirían needing-lo, pero el grueso no).
