# ADR-002 — El cliente gráfico real dentro del visor (Zink) y el mod 3D portado

Estado: **propuesta** · Fecha: 2026-10-19 · Depende de: `ADR-001-arquitectura-vr-factorio.md`

## 1. Qué cambia respecto al ADR-001

El ADR-001 llegaba a la conclusión de que la 3D la ponemos nosotros y el juego (headless) es la
autoridad de simulación. Eso ya está **implementado y validado**: el motor dibuja los sprites
reales de Factorio (atlas de 5 páginas, 84 MB) y el visor offline replica el motor para
comprobarlos sin el visor.

Pero el objetivo del usuario es "Factorio Steam Linux ARM64 + modo 3D -> Pico 4 Ultra" y,
sobre todo, **poder jugar la campaña**. La campaña tiene un bloqueo que el headless no
resuelve: **el servidor headless no tiene jugador**. Sin jugador no hay personaje, ni
escenarios de campaña que progresen.

La respuesta es el **cliente gráfico ARM64** de Factorio, que existe de forma oficial
(2.1, rama experimental de Steam, FFF #446) y que el usuario está descargando.

## 2. Zink: la pieza que lo hace viable en Android

El cliente Linux de Factorio pide **OpenGL 3.3 core**. Android no tiene OpenGL de escritorio:
tiene GLES 3.x sobre el driver del fabricante. ahí entra **Zink**, el driver Gallium de Mesa
que emite llamadas Vulkan en lugar de hablar con una GPU concreta:

> "The Zink driver is a Gallium driver that emits Vulkan API calls instead of targeting a
> specific GPU architecture. This can be used to get full desktop OpenGL support on devices
> that only support Vulkan." — docs.mesa3d.org/drivers/zink.html

Con Zink, el cliente gráfico de Factorio corre **sobre el driver Vulkan de Adreno**, es decir
con **aceleración por hardware**, en vez de con llvmpipe por software. Requisitos de Zink para
OpenGL 3.3 (los del doc) que cumple un Adreno de gama media-alta:

- `VkPhysicalDeviceFeatures::dualSrcBlend`
- `timestampComputeAndGraphics`
- `VK_EXT_vertex_attribute_divisor` (o `VK_KHR_...`)
- `VK_FORMAT_A2R10G10B10_UNORM_PACK32` y voisins con soporte de vertex buffer
- `maxPerStageDescriptorSamplers >= 16`, `maxPerStageDescriptorUniformBuffers >= 13`

Para el diagnosis: `ZINK_DESCRIPTORS=auto|lazy|db` (auto es el default) y `ZINK_DEBUG=...` para
depurar.

En Android, Mesa se controla con **propiedades del sistema** en vez de variables de entorno
(`debug.mesa.<opcion>`), aunque las variables siguen funcionando para apps.

## 3. El mod 3D: se puede portar, y hay pruebas

`factorio-3d-mod` (GDani31) es Windows: DLL + `injector.exe` + `factorio.pdb`, y resuelve los
hooks **por nombre de símbolo** del PDB, con RVAs de reserva.

**Nuestro binario Linux ARM64 no necesita PDB**: viene sin stripear y con 148.802 símbolos C++
con nombre, así que los hooks se resuelven por nombre **directamente del symtab del ELF**. Es
incluso mas robusto que el PDB.

Comprobado en `build/factorio_stripped.elf` (copia sin secciones de debug del motor 2.1.22):

| Hook (factorio-3d-mod 2.0.77, Windows/MSVC) | Equivalente en 2.1.22 ARM64 (Itanium) | Estado |
|---|---|---|
| `?render@GameRenderer@@` | `GameRenderer::render` | ✔ |
| — | `GameRenderer::prepare` / `renderInternal` / `postPrepare` | ✔ (nuevos) |
| `?centerOn@RenderParameters@@` | `RenderParameters::centerOn` | ✔ |
| `?getMapPosition@GameView@@...` | `GameView::getMapPosition` | ✔ |
| `?createRenderParameters@GameView@@` | — (reestructurado en 2.1) | buscar |
| `?draw@AssemblingMachine@@UEBAXAEAVDrawQueue@@@Z` | `AssemblingMachine::draw` | ✔ |
| `?draw@Furnace@@...` | `Furnace::draw` | ✔ |
| `?draw@MiningDrill@@...` | `MiningDrill::draw` | ✔ |
| `?draw@Turret@@...` | `Turret::draw` | ✔ |
| `?drawCraftingMachine@WorkingVisualisations@@` | `CraftingMachine::draw(DrawQueue&, Entity const&, UpdatableEntity const&, WorkingVisualisations const&, CraftingMachineAnimationState, bool, ...)` | ✔ **mejor**: recibe el estado de animación explícito |
| `?drawMiningDrill@WorkingVisualisations@@` | `MiningDrill::draw(...)` | ✔ |
| `?drawItemStack@ItemStackPainter@@` | `ItemStackPainter::drawItemStack` | ✔ |
| `?drawItems@TransportBelt@@` | `TransportBelt::drawItems` | ✔ |
| `?drawScaledTinted@DrawQueue@@` | `DrawQueue::drawScaledTinted` | ✔ |
| `??1Entity@@UEAA@XZ` | destructor de Entity | ✔ |
| `?getPrototype@Entity@@...` | `Entity::getPrototype` | ✔ |
| `?getSurface@Entity@@...` | `Entity::getSurface` | ✔ |
| `?getDarkness@DayTime@@` | — (ya no es método; la oscuridad sale de `game.surface.daytime`) | resuelto por el mod |

El port es, por tanto, el mismo trabajo que hizo el autor pero sobre ELF: un `.so` con
`LD_PRELOAD` en vez de una DLL, y resolución de símbolos desde el symtab en vez del PDB.

**Los modelos 3D ya existen**: `models/ENTITIES`, `ENEMIES`, `FOLIAGE`, `ITEMS`, `PLAYER`,
`RESOURCES`, `LEVELS`, `BLUEPRINTS` (formatos `.glb`, con `models.rs`, `model_renderer.rs` y
`warp.rs` — la deformacion de camara que convierte la vista cenital en perspectiva).

## 4. Arquitectura objetivo

```
┌───────────────────────────────────────────────────────────────────────┐
│ PICO 4 ULTRA                                                         │
│                                                                       │
│  ┌── Our APK (OpenXR) ────────────────────────────────────────────┐  │
│  │  · Capa VR: stereo, UI espacial, input de los controllers      │  │
│  │  · Publica el framebuffer del juego en estereo                 │  │
│  │  · (opcional) diorama 3D con el atlas real, ya implementado    │  │
│  └───────────────────────▲────────────────────────────────────────┘  │
│                          │ framebuffer / estado                      │
│  ┌─── Termux (Debian arm64 via proot) ──────────────────────────┐   │
│  │  · Mesa 22.3+/24 con Zink  ──►  driver Vulkan de Adreno      │   │
│  │  · Factorio 2.1.22 graphical ARM64  (el juego real, con        │   │
│  │    jugador real → la campaña funciona)                        │   │
│  │  · factorio-3d-mod portado: .so + LD_PRELOAD, hooks por       │   │
│  │    nombre desde el symtab del ELF, modelos 3D, camara warped   │   │
│  │  · RCON en un puerto → nuestra APK lee el estado y manda        │   │
│  │    órdenes (mod + RCON ya implementado)                       │   │
│  └────────────────────────────────────────────────────────────────┘  │
└───────────────────────────────────────────────────────────────────────┘
```

## 5. Resultados de la validacion en contenedor (ARM64, sin GPU)

Todo lo de abajo esta **verificado**, no supuesto. Contenedor `ubuntu:24.04` arm64 con Mesa
25.2.8 y el cliente grafico ARM64 2.1.22 real.

### 5.1 Zink da OpenGL 4.6 sobre Vulkan

Con `MESA_LOADER_DRIVER_OVERRIDE=zink`, `LIBGL_DRIVERS_PATH=.../dri`, `EGL_PLATFORM=surfaceless`
y el ICD de lavapipe:

```
EGL driver name: zink
OpenGL core profile renderer: zink Vulkan 1.4(llvmpipe (LLVM 20.1.2, 128 bits) (MESA_LLVMPIPE))
OpenGL core profile version: 4.6 (Core Profile) Mesa 25.2.8
```

Factorio pide **OpenGL 3.3 core**: Zink da 4.6. Sobra.

**Detalle que costo tiempo**: Zink rechaza por defecto los dispositivos Vulkan de tipo CPU
(`VK_PHYSICAL_DEVICE_TYPE_CPU`, que es lavapipe) y falla con `ZINK: failed to choose pdev`.
Se acepta con `LIBGL_ALWAYS_SOFTWARE=1`. **En el visor hay que dejar esa variable SIN poner**,
porque con ella Mesa elegiria llvmpipe (software) en vez del driver de Adreno.

### 5.2 El cliente grafico real arranca y renderiza

```
Factorio 2.1.22 (build 87719, linux-arm64, full)      <- "full": no es el headless
Video driver: offscreen
Initialised OpenGL:[1] zink Vulkan 1.4(...); driver: 4.6 (Core Profile) Mesa 25.2.8
  [Version] 4.6
Parallel sprite loader initialized (threads: 1, bitmaps: 4696)
Texture processor created (2048). GPU accelerated compression Supported: yes
Estimated VRAM usage for textures: 329.20 MB
Factorio initialised
```

La cadena completa es: **Factorio (full) -> SDL2 (estatico, video driver offscreen) -> EGL
surfaceless -> Mesa Zink -> Vulkan -> driver**. El cliente trae SDL2 estatico dentro del binario
y carga la capa grafica en runtime con `dlopen` de `libGL.so.1` / `libEGL.so.1` /
`libvulkan.so.1`, asi que basta con el payload de Mesa y las variables de entorno.

El cliente grafico solo enloca `libc`, `libm` y `libresolv`: no necesita nada mas.

### 5.3 El payload de Mesa para el visor

`scripts/prepare_mesa.py` genera `build/mesa/` con glibc de Debian, Mesa (con `zink_dri.so`),
loader de Vulkan, lavapipe y SDL2: **193 MB en disco, 56 MB comprimido**. El launcher
`scripts/start_factorio_client.sh` configura el ICD de Adreno (que se busca en
`/vendor/lib64/hw/vulkan`) y las variables de Mesa.

### 5.4 Lo que NO hay todavia: el jugador de la campana

La campana **necesita un jugador real**. El nivel `base/campaigns/tutorial/level-01/control.lua`
usa `game.players[1]` en `on_init` (linea 11: `game.players[1].surface.create_entity(...)`), asi
que en un servidor headless revienta con "attempt to index a nil value".

Y el cliente grafico **no tiene opcion de linea de comandos para arrancar una partida en
single-player**: `--start-server`, `--start-server-load-scenario` y `--run-replay` pasan todos a
"Running in headless mode" (sin graficos), y sin ellos el cliente se queda en el menu sin
abrir RCON.

De ahi la pieza que falta: **el inyector nativo** (el `.so` del port de factorio-3d-mod), que
ademas de enganchar el render puede **arrancar la partida programaticamente** dentro del
proceso del cliente grafico. Los simbolos existen: `AppManager::restartGame`, `Game::process`,
`Scenario::loadFactory`, `MainMenu::process`.

Por eso el port de factorio-3d-mod no es opcional en la via de la campana: es el unico
componente que vive en el proceso del cliente y por tanto el unico que puede crear la partida
con su jugador.

## 6. Los dos caminos, y por cual vamos

**A. El juego renderiza (GZH-XR puro).** El cliente gráfico corre con Zink, el mod 3D sustituye
las entidades por modelos 3D y deforma la camara; nosotros capturamos el framebuffer y lo
publicamos en estereo. Maxima fidelidad: es literalmente el juego, con su arte, sus modelos 3D
y su camara en perspectiva.

**B. El juego simula y nosotros renderizamos (ADR-001).** Ya implementado y validado: headless
ARM64 + mod + RCON + atlas real de sprites + diorama 3D nuestro.

**B no se tira**: es el camino que funciona hoy, sin GPU, sin userland y sin cliente gráfico.
Sirve de plan B permanente y de banco de pruebas (todo el gameplay ya esta verificado contra
el motor real).

**A es el objetivo**: es el unico que cumple "Factorio Steam Linux Version ARM64" y el unico que
da un jugador real para la campaña.

## 7. Orden de trabajo de A

1. **Validar Zink en el contenedor ARM64** (sin GPU): Mesa + lavapipe (Vulkan por software) +
   `SDL_VIDEODRIVER=offscreen`. Si el cliente arranca y abre RCON, la cadena esta montada y solo
   falta el driver de Adreno.
2. **Cliente grafico ARM64 dentro del contenedor** con el tarball que esta descargando el
   usuario: arranque, RCON, jugador creado, campana cargada.
3. **Portar factorio-3d-mod a Linux ARM64**: `.so` + `LD_PRELOAD`, resolucion de hooks por
   symtab, y el renderer sobre OpenGL en vez de Direct3D (es lo mas costoso: `model_renderer.rs`
   y `renderer.rs` están escritos para D3D11).
4. **Captura del framebuffer**: con Zink, el framebuffer del juego es un pbuffer EGL; se puede
   leer con `glReadPixels` desde un hook de `eglSwapBuffers`/`glReadPixels` y publicarlo por un
   socket con la imagen.
5. **Presentacion stereo en OpenXR** + UI espacial + input.

Los puntos 1-2 no requieren el visor: se validan en el contenedor. El punto 3 es el grueso.

## 8. Riesgos

| Riesgo | Mitigacion |
|---|---|
| Zink no alcanza OpenGL 3.3 en el Adreno concreto | Se valida en el dispositivo con `glxinfo`/`vulkaninfo`; si falla, se baja a llvmpipe (el cliente arranca igual, a costa de rendimiento) |
| El port de `factorio-3d-mod` a OpenGL es grande (D3D11 → GL) | Se puede portar solo el warp de camara y el hook de entidades, y usar el atlas 2D (ADR-001) para el resto |
| Factorio 2.1.22 cambia los hooks en cada parche | Se resuelven por nombre; el upstream ya resuelve asi y sobrevive a actualizaciones |
| El cliente grafico consume GPU en paralelo con nuestra app | El cliente puede renderizar a baja resolucion (720p) o a un buffer fuera de pantalla |
| El tarball del cliente son ~1.1 GB y la SD es `noexec` | Va a `/data/local/tmp`; es el mismo mecanismo que ya usa el motor headless |
