# ADR-006 — Dibujar una entidad en 3D desde el inyector

* Estado: **implementado** (el cubo se dibuja y se ve en la captura)
* Fecha: 2026-10-20 (revisado el 2026-10-21)
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
| Matriz de proyección | **calculada aquí** (sección 4.1): la del motor es ortográfica y se deduce de su propia cuenta |
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
| `GraphicsInterfaceOpenGL::bindUniformBuffer(uint, …)` | **solo diagnóstico**: se apunta qué buffer y qué punto de binding usa el motor, **sin tocar la GL** |
| `DrawCommandBatch::flushSpritesBatch(bool)` (y los otros flush) | **dibujar el cubo** y **capturar** |

### 4.1 La proyección: se calcula, no se pregunta

El motor dibuja el mundo con una **proyección ortográfica** que vive en el
bloque de uniforms `vsConstants` (`{ mat4 projection; }` en std140, según
`data/core/graphics/shaders/sprite.vert`).

**La versión anterior de este módulo la pedía al motor**: enganchaba
`GraphicsInterfaceOpenGL::bindUniformBuffer` y llamaba a
`glGetUniformBlockIndex` **sobre el programa del motor** para averiguar qué
bloque era `vsConstants`, y enlazaba ese buffer en nuestro programa. **Eso
hacía que el juego se cayera con SIGSEGV**: en cuanto el motor cambiaba de
programa se repetía la consulta y algo se rompía. Ese hook ya **no hace ninguna
llamada a la GL**: solo apunta lo que ve (ver la tabla de arriba).

La matriz se calcula con la cuenta del propio motor, desensamblada de
`SpriteDrawOrder::setup`:

```
mundo = (mapa / 256) * (32 * escala) - camara
```

* `mapa` = la `MapPosition` de la entidad (2 × i32 en 1/256 de tile, en
  `Entity+0x50`),
* `escala` = `RenderParameters+0x190` (el zoom),
* `camara` = `RenderParameters+0x1b8` (2 doubles).

Los `RenderParameters` están en el primer puntero del `DrawQueue`, así que el
hook de `DrawQueue::draw` los tiene a mano.

**Ojo con las unidades**: `mundo` **no** es una posición relativa a la cámara,
son **píxeles del framebuffer del mundo** (la cámara va en la resta de
arriba). Se comprobó creando una máquina en el juego: su `mundo` era
`(856, 384)` y en la captura salía en esos mismos píxeles. Por eso la
proyección es:

```
ndc_x = mundo_x * 2 / W - 1
ndc_y = mundo_y * 2 / H - 1
```

con W y H los del viewport del framebuffer del mundo. La Y crece **hacia
abajo**: el motor dibuja el mundo con la Y invertida y lo voltea al componerlo
en la ventana, así que al volcar el framebuffer con `glReadPixels` la imagen
sale derecha (con la GUI abajo).

### 4.2 El tamaño del sprite

El `Sprite` que el motor pasa a `DrawQueue::draw` guarda su tamaño en **dos
`u16` en `+0x16` y `+0x18`** (desensamblado de `DrawQueue::draw`:
`ldrh w8, [x23, #0x16]`). Ese tamaño es el del **atlas**, que va al doble que
el tamaño pintado, así que el tamaño en pantalla es la mitad de ese valor
multiplicado por la escala.

> Antes se leía `+0x38` (dos floats): ahí no está el tamaño (era un 0 y un
> 1.75), y el sprite quedaba con tamaño 0 y no se pintaba nada.

### 4.3 La orientación

`Entity+0x280` (u8, 4 bits) es lo que lee `CraftingMachine::getDirection()`:
16 pasos de 22,5°. Se aplica como rotación en Z de la matriz del cubo.

### 4.4 Por qué el cubo se calcula en la CPU

La proyección del motor es ortográfica y solo tiene eje X e Y (sus sprites son
2D). Si el cubo se construyera en el shader, su componente Z en espacio de
clip se saldría de `[-1, 1]` y los triángulos se recortarían. Así que la
transformación 3D (inclinación de 31° para que se vea el volumen, giro por la
dirección y escala por el tamaño real) se hace en la CPU **y la proyección
también**: al vertex shader solo le llega el punto ya proyectado a coordenadas
de clip (`gl_Position = vec4(position.xy, 0, 1)`), sin bloques de uniforms.

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

### 5.1 `SpriteDrawOrder::setup` NO se puede enganchar

La forma más limpia de tener la posición y el tamaño exactos del motor sería
enganchar `SpriteDrawOrder::setup` (ahí quedan en `+0x10` y `+0x24`). **Pero
su prólogo no se puede reubicar**: el detour queda instalado sin error y el
juego se cae con **SIGSEGV dentro de `cfree`** (heap corrupto) al primer
sprite. Se probó y se descartó: la posición y el tamaño se sacan del hook de
`DrawQueue::draw`, que sí aguanta.

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

### 6.1 El PNG tenía el checksum roto

El Adler-32 se escribía con dos `u32` (8 bytes) en vez de dos `u16` (4 bytes):
el PNG salía con 4 bytes de más y el checksum no cuadtaba, así que ningún
lector estándar lo podía abrir. Corregido (`as u16`).

### 6.2 El cubo es magenta (a propósito)

El cubo se pinta de **magenta** (`0.90, 0.10, 0.75`). En la escena del Factorio
no hay nada de ese color (las llamas son amarillas, la tierra marrón, el jugador
azul, la GUI gris), así que se pueden **contar los píxeles del cubo** en el PNG
y comprobar automáticamente que el cubo **se ve**, y no solo que el juego no se
ha caído. El banco hace exactamente eso.

## 7. El banco

`scripts/test_modelo3d.sh`:

1. compila el inyector para aarch64,
2. arranca el cliente real dentro del contenedor podman (Xvfb + x11) con la
   partida guardada,
3. espera a que el motor esté **en juego** (el inyector lo avisa por el log:
   `cima_tipo=4`),
4. **crea una máquina al lado del jugador con la consola del juego** (tecla
   `` ` ``): es determinista; andar buscando máquinas con el teclado no lo es
   (la partida no siempre trae fábricas cerca del punto de aparición),
5. espera a que el motor dibuje el cubo y a que haya captura,
6. **valida el PNG** (zlib con el Adler-32 bien puesto) y que **tenga píxeles
   del cubo**,
7. deja la captura en `build/cubo_3d.png`.

## 8. Estado

* El cubo se dibuja en el framebuffer del mundo, con los datos reales de la
  entidad, y **se ve** en la captura.
* La captura es un PNG válido y comprobable.
* El banco `scripts/test_modelo3d.sh` pasa de punta a punta.

## 9. Lo que queda pendiente

* La **deformación de cámara** (warp) es un paso posterior: aquí no se toca.
* El cubo tapa el sprite de la máquina pero el motor sigue encolándolo: si se
  quiere que desaparezca el sprite, hay que tocar `DrawQueue::draw`.
* La captura sale del framebuffer del mundo; para el framebuffer de la ventana
  (con la GUI) habría que capturar en otro punto del frame.
* El tamaño del cubo es el del sprite (la "caja" de la máquina), no el de su
  volumen real: una máquina de 3×3 tiles se ve como un cubo de 3×3×3.
* El cubo es un magenta liso: el mod 3D de verdad pinta la máquina con su
  textura. Aquí lo que se demuestra es que la geometría 3D se dibuja en la
  posición y con el tamaño correctos.
