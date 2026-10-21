# ADR-006 — Dibujar una entidad en 3D desde el inyector

* Estado: **implementado** (el cubo se dibuja y se ve en la captura)
* Fecha: 2026-10-20
* Módulo: `injector-linux/src/modelo3d.rs`
* Banco: `scripts/test_modelo3d.sh`

## 1. Qué queremos

Que el Factorio 2.1.22 ARM64 dibuje una **entidad en 3D** (un cubo) en lugar de
su sprite 2D, usando los datos **reales** de la entidad (posición, orientación y
tamaño), y que se pueda **verificar con una captura** del framebuffer volcada a
PNG.

Es el mismo objetivo que el mod 3D de GDani31 portado al proyecto GZH-XR, pero
dentro del motor: aquí no hay un renderizador externo, el cubo lo pinta el propio
juego con su propio GL y su propia cámara.

## 2. Por qué no vale con el `DrawQueue` del motor

El `DrawQueue` (`DrawQueue::drawScaledTinted` y familia) solo sabe pintar
*sprites*: quads con textura de un atlas. No hay geometría 3D arbitraria ni
shaders propios. Para un cubo hace falta:

* un VBO con las 8 esquinas y 12 triángulos,
* un vertex shader propio,
* la matriz de la entidad.

Pero **todo lo demás sí se puede heredar del juego**:

| Pieza | De donde sale |
|---|---|
| Contexto GL | el del motor: estamos dentro de su llamada de render |
| Funciones GL | las suyas: `glad_glCreateShader`, `glad_glDrawElements`, … (symtab) |
| Matriz de proyección | la del juego: bloque `vsConstants` (`{ mat4 projection; }` en std140, ver `data/core/graphics/shaders/sprite.vert`) |
| Posición y tamaño | los que el motor calcula para el sprite de la entidad |
| Dónde se dibuja | el framebuffer del mundo, justo antes de componerlo en la ventana |

## 3. Cómo dibuja el motor un frame (lo que reverseamos)

1. `GameViewWidgetLogic::renderGameView()` llama a `GameRenderer::render()`, que
   **encola** las órdenes de dibujo del mundo.
2. El motor agrupa esas órdenes en un `DrawCommandBatch` y las emite al final.
3. `GraphicsInterfaceOpenGL::bindFramebuffer(f)` **hace flush de lo pendiente
   ANTES de cambiar de framebuffer**, y luego hace
   `glBindFramebuffer(GL_FRAMEBUFFER, f->[0x34])`.
4. El mundo se pinta en un FBO propio; ese FBO se copia a una textura y se
   compone en la ventana.
5. `GraphicsInterfaceOpenGL::swapBuffers()` hace el flush final (y al final
   borra el framebuffer de la ventana con `glClear`).

**Consecuencia clave**: el sitio donde el mundo queda pintado en su framebuffer
es *dentro* de un flush. Por eso el cubo se dibuja **justo después de
`flushSpritesBatch()`** (y de los demás flush): el mundo ya está en su FBO y ese
FBO sigue enlazado. Lo que se pinte ahí se compone en la ventana.

## 4. Los hooks

| Hook | Para qué |
|---|---|
| `AssemblingMachine::draw(DrawQueue&)` | saber qué entidades se están dibujando (posición de mapa y dirección) |
| `DrawQueue::draw(Sprite const&, MapPosition const&, …)` | saber qué sprite es el de nuestra entidad, y sacar la cámara y la escala |
| `GraphicsInterfaceOpenGL::bindUniformBuffer(uint, …)` | saber qué buffer y qué punto de binding usa el motor para `vsConstants` (su matriz de proyección) |
| `DrawCommandBatch::flushSpritesBatch(bool)` (y los otros flush) | **dibujar el cubo** y **capturar** |

### 4.1 La posición y el tamaño, sin adivinar

La cuenta del motor para pasar de coordenadas del mapa a coordenadas del mundo
está en `SpriteDrawOrder::setup` (desensamblado):

```
mundo = (mapa / 256) * (32 * escala) - camara
```

* `mapa` = la `MapPosition` de la entidad (2 × i32 en 1/256 de tile, en
  `Entity+0x50`),
* `escala` = `RenderParameters+0x190` (el zoom),
* `camara` = `RenderParameters+0x1b8` (2 doubles).

Los `RenderParameters` están en el primer puntero del `DrawQueue`, así que el
hook de `DrawQueue::draw` los tiene a mano. El tamaño del sprite está en el
propio sprite (`+0x38`, dos floats) y se multiplica por la misma escala.

El resultado se comprobó contra el log: la entidad del mapa `(0.5, 45.5)` con
escala `0.75` y cámara `(-148, 978)` da mundo `(160, 114)` — y el motor la
pinta exactamente ahí.

### 4.2 La orientación

`Entity+0x280` (u8, 4 bits) es lo que lee `CraftingMachine::getDirection()`:
16 pasos de 22,5°. Se aplica como rotación en Z de la matriz del cubo.

### 4.3 Por qué el cubo se calcula en la CPU

La proyección del motor es ortográfica y solo tiene eje X e Y (sus sprites son
2D). Si el cubo se construyera en el shader, su componente Z en espacio de
clip se saldría de `[-1, 1]` y los triángulos se recortarían. Así que la
transformación 3D (inclinación de 31° para que se vea el volumen, giro por la
dirección y escala por el tamaño real) se hace en la CPU y al shader solo le
llega la proyección 2D del motor.

## 5. Un fallo del detour que costó la mitad del trabajo

El motor usa pointer authentication y, sobre todo, **saltos condicionales de 19
bits** (`tbz`, `b.cond`) en sus prólogos. Al reubicar el prólogo en el trampolín,
un salto condicional **no se puede rebasar** a su destino original: el
trampolín puede estar a cientos de MB y su alcance es de ±1 MB.

La primera versión los copiaba tal cual → el salto apuntaba a un sitio
imposible y el juego moría con **SIGILL** (ejecutaba basura).

**Arreglo** (`injector-linux/src/detour.rs`): antes de reubicar, se calcula
cuántos bytes hay que copiar para que **todos** los saltos condicionales del
bloque apunten a código también reubicado (el bloque se copia entero y en
orden, así las distancias relativas se conservan). Si el bloque no cabe en la
función, el inyector **renuncia** en vez de generar código roto.

## 6. La captura

* Se engancha el **flush**, que es cuando el framebuffer del mundo ya tiene la
  imagen del frame.
* Se lee con `glReadPixels(0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, …)` del
  framebuffer enlazado, con `w`, `h` del viewport.
* Se escribe un PNG propio (zlib "stored" + CRC32, sin dependencias).
* El motor **no voltea** la imagen del framebuffer del mundo (su `readFramebufferToMemory`
  marca el flip solo para el framebuffer de la ventana), así que sale derecha.

**El driver offscreen de SDL no sirve**: no hay framebuffer de ventana real y la
captura sale con basura. El banco usa **Xvfb + x11**, que es lo que funciona
(también lo usa `scripts/test_autostart.sh`).

## 7. Estado

* El cubo se dibuja en el framebuffer del mundo, con los datos reales de la
  entidad.
* La captura del framebuffer es válida (se ve la escena, no basura ni negro).
* El banco `scripts/test_modelo3d.sh` compila para aarch64, arranca el cliente
  real dentro del contenedor podman, carga una partida, camina hasta que una
  máquina entra en pantalla y comprueba que hay captura con contenido y que el
  log registra una caja 3D.

## 8. Lo que queda pendiente

* La **deformación de cámara** (warp) es un paso posterior: aquí no se toca.
* El cubo tapa el sprite de la máquina pero el motor sigue encolándolo: si se
  quiere que desaparezca el sprite, hay que tocar `DrawQueue::draw`.
* La captura sale del framebuffer del mundo; para el framebuffer de la ventana
  (con la GUI) habría que capturar en otro punto del frame.
* El tamaño del cubo es el del sprite (la "caja" de la máquina), no el de su
  volumen real: una máquina de 3×3 tiles se ve como un cubo de 3×3×3.
