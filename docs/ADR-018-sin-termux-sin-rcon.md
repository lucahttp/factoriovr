# ADR-018 — Sin Termux y sin RCON: el motor habla con el visor a través del inyector

> **Estado**: diseño (2026-10-24). Decisión del usuario: **ni Termux ni RCON**.
> Ejecuta `ADR-013` (el motor dibuja) sin las dos muletas que se habían
> asumido, y sustituye al puente RCON por el inyector.
> Revisa: `ADR-013`, `ADR-015` (el cliente en la Pico), `ADR-017` (el motor no
> bloquea el render).

---

## 1. Lo que se descarta y por qué

| Se descarta | Motivo |
|---|---|
| **RCON** | Es una red de sockets con sondeo: el visor pregunta, el motor responde, y el render espera (ADR-017: 61 ms por consulta). Además el motor decide por Lua, no con sus reglas: sin fantasma de construcción, sin rotación, sin deshacer. |
| **Termux** | Es una app más que instalar y mantener; el runtime que hace falta (glibc + Mesa) son **ficheros**, y ya se despliegan (`runtime/`). Nada de eso necesita la app de Termux. |

## 2. La arquitectura

```
┌──────────── APK (bionic, sin Termux) ─────────────────────────────┐
│  visor OpenXR: dibuja la escena que le llega, passthrough MR      │
│  y Lanza el motor: execve(ld-linux-aarch64.so.1, factorio, …)     │
│       con LD_LIBRARY_PATH = <runtime dentro de la app>            │
└────────────────────────────────────────────────────────────────────┘
              ▲  socket XFV1 (127.0.0.1)          │
              │  escena + órdenes                  ▼
┌──────────── motor ARM64 glibc (en la app) ───────────────────────┐
│  inyector (LD_PRELOAD):                                          │
│    · lee la escena DEL PROPIO MOTOR (hooks de dibujo o Lua)      │
│    · la empuja al visor por el socket                            │
│    · ejecuta en proceso las órdenes que llegan del visor         │
│  mod xfactorvr_bridge: expone las acciones en Lua                │
└────────────────────────────────────────────────────────────────────┘
```

**El inyector es el único canal.** Ni RCON, ni Termux, ni sockets hacia el
puerto 27015.

## 3. Cómo se habla con el motor sin RCON

### 3.1 Desde dentro: la API de Lua del motor

El binario **no está stripped** (291 301 símbolos) y exporta la API de Lua que
hace falta:

| Símbolo | Para qué |
|---|---|
| `lua_pcallk` | llamar a una función Lua (y **capturar el `lua_State*`**) |
| `lua_gettable` + `lua_pushstring` | llegar a `remote.call` por nombre |
| `lua_rawgeti` | sacar la tabla de globales del registro (`LUA_RIDX_GLOBALS`) |
| `lua_tolstring` | leer el resultado (el JSON del snapshot) |
| `lua_settop` | restaurar la pila (acepta índices negativos ⇒ desapilar) |
| `lua_pushinteger`, `lua_pushnumber` | argumentos |

⇒ El inyector puede llamar **en proceso** a
`remote.call('xfactorvr','get_world_snapshot', 32, cx, cy)` y leer el JSON
devuelto, sin red y sin RCON. Lo mismo para las órdenes:
`remote.call('xfactorvr','mine_entity', x, y)`.

**Dónde se ejecuta**: el `lua_State` se captura enganchando las funciones de
Lua que el motor llama desde C++ (`lua_pushstring`, `lua_rawgeti`,
`lua_gettable` — `lua_pcallk` **no** sirve: en Lua 5.4 `lua_call` va a
`luaD_call`, interna) y la llamada se hace **fuera** de la pila Lua, en el hook
de `MainLoop::processEvents` (hilo principal del juego) ⇒ sin reentrada.

#### 3.1.1 Lo medido (contenedor, motor 2.1.22 ARM64 real)

| Paso | Resultado |
|---|---|
| Enganchar los hooks de Lua | ✅ 3 detours instalados |
| Capturar el `lua_State*` | ✅ **funciona**: 38 738 llamadas a `lua_pushstring` y **3 estados de Lua** vistos |
| Llamar a `remote.call` en proceso | ❌ **SIGABRT**: `invalid index` en `lua_rawgeti` (lapi.c:748) desde nuestro propio hook. El motor tiene Lua en varios hilos; filtrar por el `gettid()` del hilo principal **no** lo arregla |

**Estado actual**: el módulo `injector-linux/src/lua_bridge.rs` está escrito y
con tests (33 en verde en el inyector), pero **va APAGADO por defecto**
(`XFACTORVR_LUA_BRIDGE=1` lo activa) porque la llamada en proceso todavía tumba
el motor. Verificado: con el puente apagado el motor arranca, no crashea y el
RCON responde.

**Y para V1 no hace falta**: si el motor dibuja, el estado lo dan los **hooks de
dibujo** (que ya funcionan: 16 tipos de entidad) y las órdenes son el **ratón
inyectado** (que ya funciona: `SDL_PushEvent`). El puente de Lua solo hace
falta para el caso **headless**.

### 3.2 Desde el motor hacia el visor

El protocolo XFV1 (inyector → visor, que ya existe y funciona) admite un tipo
de mensaje nuevo: **escena** (el JSON del estado). El visor consume la escena
del socket en vez de pedirla por RCON.

### 3.3 Las órdenes, hacia el motor

El visor manda la orden al inyector por el mismo socket (sentido inverso) y el
inyector la ejecuta en proceso. El gatillo del VR se traduce a la misma
semántica de ratón que usa el motor (`GameView::getMapPosition`), de modo que
**el motor aplica sus reglas**.

## 4. La GPU sin Termux (y sin root)

El pico 4 Ultra no da acceso a KGSL al usuario `shell` (el kernel registra
`kgsl: illegal gpu render pid`). Dos vías que **no necesitan Termux**:

| Vía | Qué es | Nota |
|---|---|---|
| **Turnip sobre KGSL** | El driver abierto de Adreno de Mesa, con el override correcto: **`MESA_LOADER_DRIVER_OVERRIDE=kgsl`** + `TU_DEBUG=noconform` | **El spike anterior usó `zink` (equivocado)**. Con `kgsl` el driver habla directamente con `/dev/kgsl-3d0` |
| **Android Vulkan Wrapper** | El ICD de Vulkan de Android (bionic) expuesto como ICD para programas glibc, vía libhybris. Es **un `.deb` con ficheros** → se extrae y se despliega como el resto del runtime, sin instalar Termux | Env: `VK_ICD_FILENAMES=…/wrapper_icd.aarch64.json` |

Si el motor consigue un contexto OpenGL 3.3 (Turnip → Zink, o el wrapper →
Zink), ya no necesita ni RCON (el inyector ve la escena por los hooks de
dibujo) ni el mod para el estado.

## 5. Qué hay que hacer

| # | Tarea | Tamaño |
|---|---|---|
| 1 | **Bridge de Lua en el inyector**: capturar el `lua_State`, llamar a `remote.call` en proceso, devolver el resultado | ~250 líneas + banco |
| 2 | **Mensaje de escena en XFV1** (inyector → visor) y su consumo en el visor | ~150 líneas |
| 3 | **Sentido inverso del socket** (visor → inyector) para las órdenes | ~150 líneas |
| 4 | **Quitar el RCON del puente Rust** (`game_bridge.rs`): el estado llega por el socket; las órdenes, por el puente de Lua | refactor |
| 5 | **Runtime dentro de la app**: el motor + glibc + Mesa en el directorio interno de la app, y el `execve` desde el launcher (el `execv → Permission denied` documentado en `launcher.cpp` se resuelve con la ruta interna) | ~1 día |
| 6 | **Turnip con el override correcto** (`kgsl`) y, si hace falta, el wrapper de Vulkan | 1 hora de spike |

## 6. Riesgos

| Riesgo | Mitigación |
|---|---|
| `remote.call` fuera del contexto de un mod podría ser rechazado por el motor | Se prueba en el banco; si falla, se llama a la función del mod por otra vía (p. ej. `script.raise_event` con un evento propio del mod) |
| La API de Lua cambia entre versiones de Factorio | Los nombres se resuelven **por símbolo** en runtime (el inyector ya lo hace) ⇒ si cambia, el banco lo detecta |
| Reentrada de Lua al llamar desde el hook | La llamada se hace fuera de la pila Lua (en `AppManager::process`) y se restaura la pila con `lua_settop` |
| El motor sin ventana no llama a los hooks de dibujo | Por eso el bridge de Lua (3.1) es la vía que funciona **con y sin ventana** |

## 7. Lo que NO cambia

- El visor sigue siendo OpenXR con passthrough y el anclaje a la mesa real.
- El estéreo engine-side (dos ranuras + `s_juego_estereo`) sigue igual.
- El enrutado "gana el más cercano" y el resto de la UX de `ADR-016`.
- La escala de la tabla (1 casilla = 32 px = 0,04 m) y la mesa de 1,80×1,20 m.

## 8. Referencias

- `ADR-013` — el motor dibuja; el inyector es el puente.
- `ADR-015` — el cliente gráfico en la Pico (los seis bugs del payload).
- `ADR-017` — el motor no bloquea el render (90 FPS).
- `ADR-016` — qué dice el exterior (precedentes, GPU en Android, UX).
- Turnip sobre KGSL: `MESA_LOADER_DRIVER_OVERRIDE=kgsl`, `TU_DEBUG=noconform`.
- Android Vulkan Wrapper: `mesa-vulkan-icd-wrapper` (termux-packages#22500).
