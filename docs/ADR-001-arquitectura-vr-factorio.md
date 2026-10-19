# ADR-001 — Arquitectura del modo VR de Factorio en Pico 4 Ultra

Estado: **propuesta en revisión** · Fecha: 2026-10-19 · Alcance: `src/`, `android/`, `mods/`, `scripts/`

## 1. Resumen

Hoy el proyecto ejecuta el Factorio real (headless ARM64) y **dibuja el mundo entero desde cero**
en el cliente OpenXR. El resultado se ve como una maqueta: no es el juego, es una interpretación.
Este documento explica por qué, qué hace realmente Generals-Zero-Hour-XR (GZH-XR) y cuál es la
arquitectura correcta para Factorio, con los mismos objetivos de producto.

## 2. Diagnóstico: por qué hoy se ve como una demo

Evidencia en el código, no impresión:

| Hecho | Ubicación |
|---|---|
| El render dibuja **primitivas** (cajas y cilindros) con texturas **procedurales** (`concrete`, `hazard`, `belt`, `soil`, `steel`). Ningún asset de Factorio. | `android/src/openxr_engine.cpp:771` (`init_procedural_textures`), `:878` (`add_box_textured`), `:934` (`add_cylinder_textured`) |
| Las ~40 entidades del juego se colapsan a **14 tipos hardcodeados** (`ENTITY_ASSEMBLING_MACHINE`…`ENTITY_PLAYER`). | `android/src/openxr_engine.cpp:510-524` |
| El estado que llega del juego **no permite elegir sprite**: `t` es un `u32` de una tabla fija. | `src/game_bridge.rs:1152` (`SnapshotEntity`) |
| No se transmite frame de animación, capa de sombra, ni ítem sobre la cinta. | `src/game_bridge.rs:1152-1167` |
| El transporte es **JSON por RCON**, consultado por tick. | `src/game_bridge.rs:1104-1127` |

Conclusión: el pipeline no pierde fidelidad *al final*; **nunca la tuvo**. Falta el arte, y el
formato de estado no lo permite recuperar.

## 3. Qué hace GZH-XR de verdad

GZH-XR no es "un juego con un visor VR". Es el motor del juego **recompilado** con una capa XR
adentro:

| Pieza | Fichero | Qué hace |
|---|---|---|
| Motor recompilado para ARM64 Android | `GeneralsMD/Code/Main/Xr*.h` | El bucle del juego, la cámara y el input viven dentro del motor. |
| Traducción DX8 → GLES | `Core/Libraries/Source/d3d8gles/` | El renderer propio del juego corre en el móvil. |
| Estéreo inyectado en el pipeline | `Core/Libraries/Source/d3d8gles/src/gles_pipeline.cpp:2898` (`beginXRStereo`), `:2972` (`endXRStereo`) | El juego dibuja en un **FBO propio**, con planos de recorte por ojo; hay tres modos: *atlas* (una textura de doble ancho), *multiview* (`GL_TEXTURE_2D_ARRAY` de 2 capas) y *separate* (2 FBO). |
| Publicación al XR | `Core/Libraries/Source/d3d8gles/src/d3d8gles.cpp:2233-2239`, `GeneralsMD/Code/Main/XrGameBoot.cpp:696` (`GX_XR_BeginStereoWorld`) | El atlas estéreo se entrega a OpenXR. |
| UI espacial | `GeneralsMD/Code/Main/XrPanelLayout.h`, `XrInteraction.h` | Ventanas de construcción/comandos colocables en el espacio. |

El principio que lo hace funcionar: **el juego produce los píxeles**. La capa VR no redibuja el
mundo; intercepta el framebuffer y lo publica en estéreo. De ahí el README: *"using the original
Zero Hour game world, not replacement models"*.

### 3.1 Por qué no es replicable literalmente con Factorio

1. **El motor de Factorio es código cerrado.** GZH-XR pudo recompilar porque EA publicó el motor de
   *Generals* bajo GPLv3. Con Factorio no se puede compilar un motor ARM64 Android ni inyectarle
   OpenXR: no hay código fuente.
2. **Factorio es 2D.** Su renderer solo puede producir un plano. Aunque inyectemos estéreo en su
   pipeline, el resultado es una pantalla plana en VR — justo el efecto "demo" que se quiere evitar.
   En GZH-XR el mundo es geometría 3D, así que el estéreo del motor es significativo; en Factorio no.
3. Existe Factorio oficial **ARM64 Linux con gráficos** (2.1, rama experimental de Steam, FFF #446),
   pero su descarga (`get-download/2.1.22/expansion/linux-arm64`) exige cuenta de Factorio con Space
   Age, y dentro de Android habría que montar userland Linux + GL por software.

**Conclusión:** el camino GZH-XR literal (juego renderiza + VR encima) no es alcanzable en Factorio.
Lo que sí es alcanzable —y es lo que propone el usuario— es su **producto**: el juego real como
autoridad de simulación, el arte real del juego, y una capa VR con la calidad de GZH-XR.

## 4. Arquitectura objetivo

Tres capas, cada una con una responsabilidad y una fuente de verdad:

```
┌─────────────────────────────────────────────────────────────────────┐
│ CAPA 3 — PRESENTACIÓN VR (nuestro cliente OpenXR)                  │
│   • Diorama 3D con el atlas REAL de sprites de Factorio            │
│   • UI espacial (construir / inventario / minimapa / tecnología)    │
│   • Entrada de los controllers → órdenes de juego                   │
└───────────────▲──────────────────────────────────▲────────────────┘
                │ estado del juego                 │ órdenes
                │ (sin latencia de red)            │
┌───────────────┴──────────────────────────────────┴────────────────┐
│ CAPA 2 — PUENTE (dos modos, mismo contrato)                        │
│   A) Mod Lua + RCON  → funciona hoy, portable, headless            │
│   B) Inyector nativo → lee el proceso del cliente real (Frida/     │
│      LD_PRELOAD), sin IPC: estado + framebuffer + órdenes          │
└───────────────▲────────────────────────────────────────────────────┘
                │
┌───────────────┴────────────────────────────────────────────────────┐
│ CAPA 1 — SIMULACIÓN (Factorio real)                                │
│   • Hoy: headless ARM64 2.1.22 en el visor                        │
│   • Objetivo: cliente gráfico ARM64 2.1 (Steam) → jugador real,   │
│     campaña, eventos del juego                                     │
└─────────────────────────────────────────────────────────────────────┘
```

El contrato entre capas 2 y 3 no cambia al cambiar de modo A a B: **el renderer consume un
snapshot de entidades y emite órdenes**. Eso permite construir el diorama 3D antes de tener el
inyector nativo, y viceversa.

## 5. Por qué la ruta B (inyector nativo) es la que habilita la campaña

El modo A (headless) tiene una limitación de fondo: **el servidor headless no tiene jugador**.
`game.players[1]` es `nil`; no hay personaje, ni cursor, ni minado manual con personaje, ni
escenarios de campaña con objetivos. El mod lo compensa con un inventario virtual, lo cual es un
"constructor remoto", no un jugador.

En modo B el proceso es el **cliente gráfico real**: hay jugador, hay campaña, y el estado se lee
de la memoria del proceso (sin RCON ni latencia). `factorio-3d-mod` demuestra que el patrón es
viable: hookea el cliente de Factorio con Frida/DLL y dibuja 3D. En nuestro caso el binario
Linux ARM64 **no está stripeado y trae 148.802 símbolos C++ con nombre** (`nm -C`), así que los
hooks se resuelven **por nombre**, sin PDB de Windows.

Símbolos reales del motor 2.1.22 relevantes para el enganche (verificados con `nm -C`):

- `AtlasBuilder::loadSprites`, `AtlasBuilder::cropSpritesMainThread`, `AtlasBuilder::assignBitmapToSprites`
- `SpriteRegistration`, `MemoryBitmap`, `VideoBitmap`
- `Animation4Way::load`, `Animation::load`, `AnimationVariations::loadSheet`
- `ArcDrawOrder::render`, `DrawQueue`

## 6. Plan de trabajo

| # | Fase | Entregable | Depende de |
|---|---|---|---|
| 1 | **Assets reales** | Extractor de Factorio → `atlas.png` + `manifest.json` + iconos + textos | Instalación con gráficos (2.0.77 sirve de prototipo) |
| 2 | **Estado enriquecido** | El mod envía *nombre de tipo*, frame de animación, capa de sombra, ítem en cinta | — |
| 3 | **Diorama 3D** | Render con sprites reales + cámara 3D + luz/sombra de Factorio | 1 + 2 |
| 4 | **UI espacial** | Paneles colocables con iconos y textos reales (equivalente a GZH-XR) | 1 |
| 5 | **Cliente gráfico ARM64** | Factorio 2.1 con gráficos dentro del visor: userland Linux + GL | Cuenta de Factorio / Steam experimental |
| 6 | **Inyector nativo** | Frida/LD_PRELOAD sobre ARM64: estado del cliente + framebuffer | 5 |
| 7 | **Campaña** | Escenarios, objetivos, jugador real | 5 + 6 |

Las fases 1-4 no dependen de las 5-7: se pueden construir y validar ya contra el headless.
Las fases 5-7 son las que desbloquean "jugar la campaña".

## 7. Riesgos

| Riesgo | Mitigación |
|---|---|
| El cliente gráfico ARM64 no arranca dentro de Android (GL por software, SDL, SELinux) | Fase 5 es exploratoria y con salida: si falla, la ruta A sigue sirviendo y el juego se juega como commander remoto |
| Extraer assets de la versión que no coincide con la del servidor (2.0.77 vs 2.1.22 cambia la estructura de sprites) | Usar siempre la misma versión para datos y PNG; el extractor lo reporta |
| El inyector nativo se rompe con cada actualización de Factorio | Resolver hooks por nombre de símbolo, igual que `factorio-3d-mod` con el PDB |
| Latencia del modo A | Ir a modo B; mientras tanto, interpolar en el cliente |

## 8. Decisiones pendientes

1. **Origen de los assets 2.1.22**: el headless no trae PNG y su estructura de sprites es distinta
   de la 2.0.77 del escritorio. Hace falta el cliente 2.1.22 (cuenta de Factorio) o actualizar la
   instalación de Steam a la rama experimental 2.1.
2. **Alcance de la primera versión jugable**: ¿diorama 3D con sprites reales (fiel al juego) o
   modelos 3D por entidad (como `factorio-3d-mod`)? Lo primero usa el arte que ya existe; lo segundo
   necesita assets 3D que no tenemos.
