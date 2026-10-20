# ADR-004: Arrancar una partida real desde el inyector (cliente grafico)

- **Estado**: parcial (el nivel se carga; la partida no llega a empezar)
- **Fecha**: 2026-10-20
- **Relacionado**: ADR-002 (validacion con Zink en contenedor ARM64)

## 1. El problema

El cliente **grafico** de Factorio 2.1.22 (ARM64) no tiene ninguna opcion de
linea de comandos para abrir una partida en single-player:

- `--start-server` y `--start-server-load-scenario` conmutan el motor a
  *"Running in headless mode"* (sin graficos).
- Sin ellos el cliente se queda en el **menu**, sin abrir RCON.

Y un nivel de campana necesita un **jugador real**: su `control.lua` usa
`game.players[1]`, que en un servidor headless no existe. Por eso el
arranque tiene que hacerse **dentro del proceso del cliente grafico**.

El objetivo es que el inyector llame por dentro al motor para que arranque la
partida, y que se pueda comprobar con tres cosas: (a) el log del cliente
muestra que se cargo el nivel, (b) el RCON del cliente abre y responde, y
(c) el hook sigue vivo y el juego no se cae.

## 2. Como se ha trabajado

El motor **no** esta stripeado (261 MB con 94.939 simbolos de funcion), asi
que el symtab es la fuente de verdad. Ghidra 12.1.4 no puede analizar el
binario completo en un tiempo razonable, asi que:

- Los simbolos se(localizan con `llvm-nm`/`llvm-nm -C` del NDK 27.
- El ABI de las funciones internas se dedujo con `llvm-objdump -d` sobre
  rangos concretos del binario.
- Para encontrar quien llama a quien se escribio un escaner de instrucciones
  `BL` sobre la seccion `.text` (el encoding de `BL` es directo, asi que se
  puede recorrer el binario buscando el destino exacto).

Los nombres **mangled** son la unica forma sin ambiguedad: el nombre
des-mangled no distingue sobrecargas (hay tres
`ScenarioExecutionContext::ScenarioExecutionContext`), y el mangling Itanium
tampoco codifica el tipo de retorno (esto ultimo es una trampa importante:
`AppManager::process()` en realidad **devuelve un `bool`**).

## 3. Simbolos usados

Todos resueltos por nombre mangled exacto, con el metodo nuevo
`Simbolos::resolver_exacto()` (anadirlo fue necesario porque el resolver
existente solo comparaba el nombre cualificado, insuficiente para los
constructores).

| Simbolo (mangled) | Que es |
|---|---|
| `_ZN10AppManager7processEv` | `AppManager::process()` — la maquina de estados de la aplicacion |
| `_ZN10AppManager10createGameERK24ScenarioExecutionContext` | `AppManager::createGame()` — arranca una partida **nueva** desde un escenario |
| `_ZN10AppManager14createTutorialEv` | `AppManager::createTutorial()` |
| `_ZN16ScenarioLocationC2ERKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEES7_S7_` | `ScenarioLocation::ScenarioLocation(string, string, string)` |
| `_ZN24ScenarioExecutionContextC2ERK16ScenarioLocation10Difficulty9NamedBoolI15EnableReplayTagE` | `ScenarioExecutionContext::ScenarioExecutionContext(ScenarioLocation const&, Difficulty, NamedBool<EnableReplayTag>)` |
| `_ZN16InLoadingGameBoxC2ERKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEERKN10Filesystem4PathE8LoadTypeb9NamedBoolI18DoSpaceAgeFixUpTagE` | `InLoadingGameBox::InLoadingGameBox(...)` — **carga una partida guardada** |
| `_ZN10AppManager9pushStateEP15AppManagerState` | `AppManager::pushState()` |
| `_ZN10AppManager8popStateEv` / `_ZN10AppManager16popStateInternalEv` | sacar estados de la pila |
| `_ZN10Filesystem4PathC2Ev` | `Filesystem::Path::Path()` |
| `_Znwm` | `operator new(unsigned long)` |

## 4. Como se dedujo el ABI

### 4.1 `std::__cxx11::basic_string` (32 bytes)

De `ScenarioExecutionContext::getDefault()`, que construye tres cadenas en la
pila:

```asm
str  x20, [x19]          ; _M_p = &buf[0]      (buf en +0x10)
str  x8,  [x19, #8]      ; _M_string_length
str  w9,  [x19, #0x10]   ; texto + NUL
```

Es decir `{char* p; size_t len; char buf[16];}` con SSO (caben 15 chars).

### 4.2 `ScenarioLocation` (0x108 bytes) y `ScenarioExecutionContext`

`ScenarioExecutionContext::ScenarioExecutionContext(ScenarioLocation const&, ...)`
copia la localizacion en el offset 0 y guarda la dificultad en el offset
**0x108**, luego `ScenarioLocation` mide 0x108 bytes. Su
constructor guarda:

- `+0x00`: `std::string` — el 2º argumento del constructor.
- `+0x20`: `std::filesystem::path` — `Path(filename(3º argumento))`.
- `+0x40`: `std::string` — el 1º argumento (el **nombre del modulo**).

### 4.3 Que significan los tres argumentos de `ScenarioLocation`

De `ScenarioLocation::getPrototypePath()`:

```asm
bl  Mod::getCampaignsPath()                  ; data/<mod>/campaigns
bl  std::filesystem::path::path<string>(...)  ; el 2º argumento
bl  Filesystem::operator/(campaigns, arg2)    ; data/<mod>/campaigns/<arg2>
bl  Filesystem::operator/(eso, Path(this+0x20)); + /<3º arg>
```

**Es decir: `ScenarioLocation(modulo, campana, nivel)` resuelve a
`data/<modulo>/campaigns/<campana>/<nivel>`.** Eso es exactamente donde vive
un nivel de campana (`data/base/campaigns/tutorial/level-01`).

Y el 2º argumento tiene un caso especial: si es la cadena `":tutorials:"`
(11 chars) se marca con un flag distinto y la ruta pasa a ser
`<mod>/tutorials/<nivel>` (los tutoriales interactivos de
`data/base/tutorials/`).

### 4.4 `Filesystem::Path` (0x28 bytes)

De `Filesystem::Path::Path()`:

```asm
strb wzr, [x8, #0x10]!     ; el buffer local del std::string
stp x8, xzr, [x0], #0x20   ; _M_p = &buf[0], len = 0
bl  std::filesystem::path::_List::_List()   ; los componentes, en +0x20
```

Es `{std::string _M_pathname (32 bytes); path::_List _M_cmpts (8 bytes)}`.
El `std::filesystem::path` de libstdc++ recalcula los componentes a partir del
texto cuando los necesita (`_M_split_cmpts()`), asi que se puede escribir el
texto directamente en el `std::string` interno.

## 5. Los dos caminos que tiene el motor

Escaneando el binario en busca de los llamadores de `AppManager::createGame`
y de `InLoadingGameBox::InLoadingGameBox` se ve claramente que el motor tiene
**dos** caminos distintos, y que **solo uno abre el RCON**:

| Camino | Estado | Como se llama | RCON |
|---|---|---|---|
| Partida **nueva** desde un escenario | `InCreatingGame` | `AppManager::createGame(ctx)` | **NO** |
| Cargar una **partida guardada** | `InLoadingGameBox` | `new InLoadingGameBox(nombre, ruta, LoadType, false, tag)` + `AppManager::pushState()` | **SI** |

`AppManager::startRconIfEnabled()` (0x2b39270) solo se llama desde
`InLoadingGameBox::process()` (0x2b38ed4) y
`InLoadingMultiplayerGameBox::process()` (0x2b757c8). **El camino de
`createGame` no abre RCON**, asi que para tener RCON hay que usar el camino de
cargar un save.

El motor ya hace exactamente eso en `InMainMenu::process()` (el boton
*"Continue Game"* del menu), y ahi se copio la secuencia:

```asm
mov  w0, #0x60
bl   operator new
add  x1, x20, #0x18      ; &nombre  (un std::string)
add  x2, x20, #0x40      ; &rutaDelSave (un Filesystem::Path)
mov  w3, wzr             ; LoadType = 0
mov  w4, wzr             ; bool = false
mov  x5, xzr             ; NamedBool<DoSpaceAgeFixUpTag> = 0
bl   InLoadingGameBox::InLoadingGameBox
mov  x0, x19             ; el AppManager
mov  x1, x22             ; el estado nuevo
bl   AppManager::pushState
```

## 6. Estado verificado (salidas reales)

### 6.1 El nivel se carga (CASO DE LA PARTIDA GUARDADA)

Con `XFACTORVR_AUTOSTART=/factorio/saves/client_test.zip`:

```
[xfv-injector] info: autostart: partida guardada '/factorio/saves/client_test.zip'
[xfv-injector] info: cargando la partida guardada '/factorio/saves/client_test.zip'
 110.036 Loading map /factorio/saves/client_test.zip: 523809 bytes.
 110.149 Loading level.dat: 620896 bytes.
 110.149 Info Scenario.cpp:152: Map version 2.1.22-1
 110.181 Blueprint storage "blueprint-storage-2.dat" was not found, ...
 110.184 Loading script.dat: 1184 bytes.
 110.188 Checksum for script __level__/control.lua: 3704017257
```

El save **real** se abre y se lee (`level.dat` de 620.896 bytes, map version
2.1.22-1, `script.dat` de 1.184 bytes y el checksum de su `control.lua`).

### 6.2 El nivel se carga (CASO DEL ESCENARIO DE CAMPANA)

Con `XFACTORVR_AUTOSTART=base|tutorial|level-01`:

```
[xfv-injector] info: arrancando escenario: modulo='base' campana='tutorial' nivel='level-01' dificultad=1
  53.776 Loading level.dat: 1413855 bytes.
  53.776 Info Scenario.cpp:152: Map version 1.2.0-93
```

El nivel de campana se carga con el `control.lua` correcto (el mismo checksum
`2743417419` que el escenario `base/xfv-tutorial` del servidor headless).

### 6.3 Lo que **NO** funciona todavia

La carga se queda **justo despues** de leer el `control.lua`: el mapa no llega
a crearse, la partida no empieza y el RCON no abre. El proceso sigue vivo y
el hook sigue vivo, pero el log del cliente se queda en
`Checksum for script __level__/control.lua`.

Salida real del banco (`bash scripts/test_autostart.sh`) con el save del
banco:

```
  lineas del cliente que lo demuestran:
      62.200 Loading map /factorio/saves/client_test.zip: 523809 bytes.
      62.907 Loading level.dat: 620896 bytes.
      62.907 Info Scenario.cpp:152: Map version 2.1.22-1
      63.161 Checksum for script __level__/control.lua: 3704017257

  ultimas lineas del inyector:
    [xfv-injector] info: autostart: partida guardada '/factorio/saves/client_test.zip'
    [xfv-injector] info: pila de estados tras dejar solo el base: 1 estados
    [xfv-injector] info: tipo del estado que queda: 2
    [xfv-injector] info: cargando la partida guardada '/factorio/saves/client_test.zip'

  (a) el nivel se cargo  : SI
  (b) el RCON respondio  : NO
  FALLO la partida arranco pero el RCON NO respondio
```

El banco **no se da por bueno** ese caso: sale con 1.

### 6.4 El camino del escenario de campana si que llega a arrancar el mapa

Con `XFACTORVR_AUTOSTART=base|tutorial|level-01` (usando `createGame`) se
llegó a tener el mapa **tickando** (`Map tick at moment of crash: 157301`)
antes de que el motor reventara al sacar el ultimo estado de una pila vacia
(ver 7.3). Con el arreglo de dejar un solo estado base, ese camino ya no
revienta, pero `createGame` **no abre el RCON** (ver el punto 5), asi que no
sirve para la prueba del RCON.

## 7. Los errores que se han encontrado por el camino (y como se han resuelto)

Estos son importantes: casi todos habrian parecido "el motor falla".

### 7.1 `AppManager::process()` devuelve un `bool` (no se ve en el nombre)

El mangling Itanium **no codifica el tipo de retorno**, asi que
`_ZN10AppManager7processEv` parece un `void`. Su unico llamador hace:

```asm
bl   AppManager::process()
cbnz w0, 0x336b7fc        ; el valor de retorno decide que sigue
```

El primer hook declaraba `void` y se comia ese valor: `MainLoop::prePrepare()`
cortaba la fase de preparacion antes de tiempo y la partida nunca llegaba a
empezar. **El hook devuelve ahora el valor del original.**

### 7.2 `mprotect` exige direcciones alineadas a pagina

El segundo trampolin de la pagina de trampolines empieza en `base + 32`, que no
esta alineado, y `mprotect(addr, 4096, ...)` fallaba con `EINVAL` (el
segundo detour no se instalaba). Ahora se protege **la base** de la pagina.

### 7.3 La pila de estados no se puede vaciar, pero se puede dejar con un solo estado

Un intento de dejar la pila de estados vacia (para que el bucle de estados no
se parase en el menu) provoca un SIGSEGV: el motor da por hecho que siempre
queda un estado base, y su propio `AppManager::popState()` lee `*(fin - 1)`,
que en una pila vacia es memoria basura, y llama a un vtable tartamudo. El
crash sale en `AppManager::changeStateInternal()` (AppManager.cpp:611), que
termina en `InfoBoxManager::acknowledgeResize()`:

```asm
ldr  x8, [x8, #0x260]     ; fin de la pila
ldur x0, [x8, #-0x8]      ; *(fin - 1)  <-- basura si la pila esta vacia
ldr  x8, [x0]             ; vtable basura
ldr  x8, [x8, #0x20]
blr  x8                   ; salta a la basura
```

**=> La pila de estados NO se deja vacia: se deja con un solo estado (el de
abajo, el menu principal, que es el estado base que el motor espera).** Para
ello se usan `AppManager::popStateInternal()` (que saca el estado de la cima
sin tocar el GUI) hasta que queda uno. Asi el motor puede seguir sacando
estados sin leer basura.

### 7.4 `AppManager::popState()` toca el GUI y revienta

`popState()` (a diferencia de `popStateInternal()`) ademas mueve el widget del
estado entre contenedores de `agui`, y llamarlo desde fuera del bucle de la
aplicacion revienta. Para ajustar la pila hay que usar `popStateInternal()`.

### 7.5 El driver `offscreen` de SDL no vale para arrancar una partida

Con `SDL_VIDEODRIVER=offscreen` + `EGL_PLATFORM=surfaceless` el cliente
arranca y el menu se dibuja, pero **la transicion de estado del motor** (que
toca el GUI, `agui::Gui::add()`) provoca un SIGSEGV con el mismo patron. Con
un servidor X de verdad (`Xvfb :99` + `SDL_VIDEODRIVER=x11` + Mesa llvmpipe)
la transicion se hace **sin crashing**. El banco usa ahora Xvfb.

## 8. Lo que queda por hacer

1. **Que la carga termine.** La carga del save se hace en un hilo
   (`ParallelScenarioLoader::load`) y el hilo se queda en
   `Checksum for script __level__/control.lua`: el mapa no se crea. La
   sospecha principal es que el hilo de carga necesita que el hilo principal
   siga **procesando eventos**: en el flujo normal del GUI eso lo hace
   `MainLoop::processEvents()`, que esta en el bucle de menu
   (`MainLoop::run`, 0x333dc5c), y en cuanto el motor deja el menu ese bucle
   no se vuelve a entrar. Habria que:
   - confirmar si `InLoadingGameBox::process()` se esta llamando desde
     `MainLoop::gameUpdateStep()` (que recorre la pila de estados con el slot
     0x68 de la tabla virtual), y
   - ver que callback de `ParallelScenarioLoader::load` se ejecuta y si
     necesita un `MainLoop::processEvents()` explicito.
2. **El camino del escenario de campana** (`createGame`) ya carga el nivel y
   llega a tener el mapa en marcha, pero ese camino **no abre el RCON** (ver
   la tabla del punto 5). Para tener RCON con un nivel de campana haria falta
   convertir el escenario en un save y cargarlo por el camino del save, o
   abrir el RCON a mano.
3. El **`guardado`** (cargar un `.zip`) y el **`escenario`** (crear desde
   `data/base/campaigns/...`) conviven en `XFACTORVR_AUTOSTART`:
   - `XFACTORVR_AUTOSTART=/ruta/al/save.zip` -> carga el save.
   - `XFACTORVR_AUTOSTART=base|<campana>|<nivel>` -> arranca el escenario.
   - `XFACTORVR_AUTOSTART=<campana>/<nivel>` -> igual, con el modulo `base`.

## 9. Variables de entorno

| Variable | Que hace |
|---|---|
| `XFACTORVR_AUTOSTART` | Que arrancar (ver arriba). Si no esta, el inyector se comporta como antes (solo el hook). |
| `XFACTORVR_AUTOSTART_DELAY_S` | Segundos que espera desde que el motor esta listo antes de arrancar (por defecto 3). |
| `XFACTORVR_AUTOSTART_DIFFICULTY` | Dificultad del contexto (0..3, por defecto 1, que es lo que usa `ScenarioExecutionContext::getDefault()`). |
| `XFACTORVR_AUTOSTART_DEBUG=1` | Vuelca la pila de estados del AppManager antes de arrancar. |
| `XFACTORVR_FASE` | `menu` = fuerza el retorno de `AppManager::process()` a "false" (el motor se queda en la fase de preparacion). `natural` (por defecto) = devuelve el valor del motor. |
| `XFACTORVR_ESTADO_APP` | Valor con el que se fuerza `AppManager+0x58` ("estado de la app"). 0 = no se toca (por defecto). |
| `XFACTORVR_ESTADO_INGAME` | 1 = empuja el estado `InGame` encima del `InLoadingGameBox` (por defecto 0: con el `InGame` puesto el juego peta con un SIGSEGV). |
| `XFACTORVR_LLAMAR_CARGA` | 1 = llama a mano al `process()` del `InLoadingGameBox` desde el hook de `processEvents` (por defecto 0: con la llamada manual el juego peta con un SIGSEGV). |

## 10. Como se comprueba

```bash
bash scripts/test_injector.sh          # el hook sigue vivo (no se ha roto nada)
bash scripts/test_autostart.sh          # el juego arranca solo y el RCON responde
```

`scripts/test_autostart.sh` **no se da por bueno un caso que no se ha
comprobado**: sale con 0 solo si el RCON **abre y contesta** **y** el tick del
juego **avanza** (dos consultas a `/time` separadas unos segundos dan valores
distintos); con 1 si no arranca, si el RCON no contesta o si el tick no
avanza; con 2 si no se ha podido ni hacer la prueba.

Ojo: leer "Loading map ..." en el log **no** prueba que la partida arranque: el
motor lee el fichero del save y se puede quedar atascado ahi sin llegar a
crear el mapa. Por eso el banco comprueba el RCON y el tick, no el log.

## 11. La fase del bucle: lo que se ha averiguado con `gdb` (2026-10-20)

Este apartado recoge la investigacion hecha con `gdb` sobre el proceso real
(conteneror `xfactorvr-client`, cliente grafico 2.1.22 ARM64). **La idea que
se iba a probar (forzar `isAppInMenu()` a `false`) resulto ser falsa**, y lo
que se ha encontrado es otra cosa.

### 11.1 La hipotesis de `isAppInMenu()` NO funciona: no se llama

Se puso un punto de ruptura en `AppManager::isAppInMenu()` y otro de control en
`MainLoop::processEvents(bool)`, y se conto en 40 frames:

```
FINAL processEvents=40  InLoadingGameBox::process=0  isAppInMenu=0
```

* `MainLoop::processEvents(bool)` -> **40 de 40** (se llama en cada frame).
* `AppManager::isAppInMenu()` -> **0**.
* `InLoadingGameBox::process()` -> **0**.

**`isAppInMenu()` no se llama ni una vez en el bucle**, asi que engancharlo
para que devuelva `false` no puede cambiar nada. La razon: solo lo llama
`MainLoop::processEvents()` en un camino raro (el de "escapar del menu"), y el
motor decide si esta en el menu con la **misma comprobacion pero enlineada**
dentro de `AppManager::process()` (el tipo del estado de la cima: 2 = menu,
0 y 3 tambien cuentan como menu).

### 11.2 Quien llama al `process()` de los estados

`AppManager::process()` (AppManager.cpp:264) llama al `process(AppManager*)`
**del estado que esta en lo alto de la pila**, en cada frame. Con un punto de
ruptura en esa llamada se vio que el motor estaba llamando a
`InGame::process(AppManager*)` y no a `InLoadingGameBox::process()`.

La tabla virtual de `AppManagerState` (sacada de las relocalizaciones de
`_ZTV10InMainMenu` y `_ZTV16InLoadingGameBox`) es:

```text
  +0x38  getName() const          +0x60  isListeningToEvent() const
  +0x40  getStateType() const      +0x68  isSavingMap() const
  +0x48  process(AppManager*)      +0x70  shouldQueueEvents() const
  +0x50  processAction(...)        +0x78  isLoadingMap() const
  +0x58  processEvent(...)         +0x80  isToggleable() const
```

Los tipos de estado (`getStateType()`) son: 0 = `AppManagerState` (y
`InLoadingGameBox`, `InCreatingGame`, ...), 1 = `InEmptyScreen`, 2 = los menus
(`InMainMenu`, `InSinglePlayerMenu`, ...), 3 = los dialogos, **4 = `InGame`**,
6 = `InTutorial`, 7 = las cajas de guardado, 8 = la pantalla de fin.

### 11.3 El bloqueo de verdad: la caja de carga se autodestruye

`InLoadingGameBox::process()` (AppManagerStates.cpp:1659) tiene un guardia:

```asm
2b38e90: ldr  x8, [x19, #0x260]      ; fin de la pila de estados
2b38e94: ldur x0, [x8, #-0x8]       ; el estado de la CIMA
2b38e98: ldr  x8, [x0]
2b38e9c: ldr  x8, [x8, #0x40]       ; getStateType()
2b38ea0: blr  x8
2b38ea4: cmp  w0, #0x4              ; es un InGame?
2b38ea8: b.eq 0x2b38ed4            ; si: solo startRconIfEnabled()
2b38eac: mov  x0, x19
2b38eb0: bl   AppManager::popStateToMainMenu()   ; si no: ABANDONA la carga
2b38eb4: mov  w0, #0x10
2b38eb8: bl   operator new          ; un InGame de 16 bytes
2b38ec8: stp  x8, xzr, [x1]         ; vtable de InGame
2b38ed0: bl   AppManager::pushState()
2b38ed4: bl   AppManager::startRconIfEnabled()
```

O sea: **si el estado de la cima no es de tipo `InGame` (4), el motor da la
carga por perdida, saca TODOS los estados hasta el menu (y con ello destruye
el `InLoadingGameBox` y su hilo del cargador) y empuja un `InGame` de 16
bytes**. Eso es exactamente lo que pasaba: el log del cliente lo delata con

```
Warning Map.cpp:372: Map loading failed: Loading cancelled.
```

y la pila de estados acababa siendo `[InMainMenu(2), InGame(4)]` en vez de
`[InMainMenu(2), InLoadingGameBox(0)]`.

### 11.4 Que falta para que el mapa llegue a existir

`InGame::process()` (AppManagerStates.cpp:94) es el estado que de verdad
conduce la partida, y para crear el mapa necesita dos cosas:

```asm
2b3d140: ldr  x8, [x22, #0x78]      ; el Scenario que se esta cargando
2b3d144: cbz  x8, 0x2b3d228        ; sin Scenario: se rinde
2b3d148: ldr  x8, [x22, #0x2c0]     ; la vista de control (GameView)
...
2b3d190: ldr  w8, [x19, #0x58]      ; AppManager+0x58 ("estado de la app")
2b3d198: cmp  w8, #0x1
2b3d19c: b.ne 0x2b3d254
2b3d1a0: ldr  w8, [x9, #0x770]      ; Scenario+0x770
2b3d1a4: cmp  w8, #0x2
```

Medido en el proceso real: `MainLoop+0x78` (el Scenario) **si** estaba puesto
(lo pone el propio `InLoadingGameBox::process()`), pero `MainLoop+0x2c0` (la
vista de control) estaba a **NULL**, `AppManager+0x58` valia **0** (y el motor
solo escribe 1, 2 y 3 en el campo, nunca 0) y `Scenario+0x770` valia **1**
(la carga no habia terminado).

### 11.5 Lo que se ha cambiado en el inyector

* Modulo nuevo `injector-linux/src/fase_juego.rs`: la tabla virtual de
  `AppManagerState`, los offsets de la pila de estados del `AppManager`, el
  campo "estado de la app" y los helpers para leer la cima y llamar a su
  `process()`.
* Se engancha `MainLoop::processEvents(bool)` (ademas de
  `AppManager::process()`): se llama **una vez por frame en todas las fases**,
  mientras que `GameRenderer::render` solo se llama cuando se dibuja el
  juego, y en la fase de carga no se dibuja. Es el sitio correcto para
  diagnosticar la carga, y (con `XFACTORVR_LLAMAR_CARGA=1`) para llamar a
  mano al `process()` del `InLoadingGameBox`.
  **Por defecto NO se llama a mano**: el motor ya lo llama (es el estado de la
  cima) y la llamada manual acaba en su rama de "abandono", que hace
  `AppManager::popState()`, y `popState()` toca el GUI y revienta si se llama
  desde fuera del bucle de la aplicacion (ver 7.4). Con la llamada manual el
  juego peta con un SIGSEGV.
* Se empuja un estado `InGame` encima del `InLoadingGameBox` para que el
  guardia de 11.3 no mate la carga. **Va desactivado por defecto**
  (`XFACTORVR_ESTADO_INGAME=1` lo activa): el `InGame` de 16 bytes es un
  objeto a medio construir, y su `process()` lo saca de la pila (hace
  `popStateToMainMenu()`) cuando no encuentra la vista de control; el motor
  sigue llamando a su `process()` sobre memoria ya liberada y el juego peta
  con un SIGSEGV. Con el `InGame` puesto la carga ya no se cancela (desaparece
  el "Map loading failed: Loading cancelled."), pero el mapa no llega a
  crearse.
* Ya **no** se fuerza el retorno de `AppManager::process()` a "false": el
  razonamiento de 7.1 era erroneo, `prePrepare()` llama a
  `AppManager::process()` en cada frame tambien en la fase del juego. Se
  puede volver al comportamiento antiguo con `XFACTORVR_FASE=menu`.
* El parser del ELF ahora guarda tambien los simbolos de tipo objeto
  (`STT_OBJECT`), que hacen falta para resolver `_ZTV6InGame`.

### 11.6 Lo que SIGUE sin funcionar

**El banco sigue en rojo** (`scripts/test_autostart.sh` sale con 1): el RCON no
abre y el tick no avanza. La carga se queda en

```
Loading map /factorio/saves/client_test.zip: 523809 bytes.
Loading level.dat: 620896 bytes.
Info Scenario.cpp:152: Map version 2.1.22-1
Loading script.dat: 1184 bytes.
Checksum for script __level__/control.lua: 3704017257
```

y de ahi no pasa. Con los puntos de ruptura puestos se vio que:

* `InLoadingGameBox::process()` **no se llama** (0 de 40 frames) y
  `InGame::process()` **si** (40 de 40): el motor esta llamando al `process()`
  del `InGame` que el propio motor empujo al abandonar la carga.
* El diagnostico del inyector lo confirma en vivo:

  ```
  carga: tick=1   estado_cargador=0xffffffff flag=0xffffffff cima_tipo=0 estados=2
  carga: tick=960 estado_cargador=0xffffffff flag=0xffffffff cima_tipo=4 estados=2
  ```

  Al principio la cima es el `InLoadingGameBox` (tipo 0) y un frame despues ya
  es un `InGame` (tipo 4) con la caja de carga **destruida** (el puntero al
  cargador se ha puesto a NULL): el motor se ha cargado el mapa a si mismo
  con su rama de abandono de 11.3.
* `MainLoop+0x78` (el `Scenario`) **si** esta puesto, pero `MainLoop+0x2c0`
  (la vista de control) esta a **NULL**, y `InGame::process()` sin vista de
  control no crea el mapa.
* Los hilos de trabajo del motor estan todos parados en
  `WorkerThread::loop()` esperando trabajo: **nadie esta cargando el mapa**.

O sea: falta poner al motor en el estado en que el GUI de verdad pondria las
cosas (la vista de control y el `Scenario` con el estado "cargado"), en vez de
empujar a mano estados a medio construir. La via que queda es la que usa el
propio GUI: `InLoadGameDialog::process()` (que es quien crea la vista de
control, `MainLoop+0x2b8` y `MainLoop+0x2c0`) y despues el
`InLoadingGameBox`, en ese orden.



## 12. La via oficial del motor tambien se atasca: por que (2026-10-20)

Este apartado recoge la investigacion hecha porque la via **oficial** del motor
(`--load-scenario`, `--load-game`, o el boton "Continue" del propio menu)
se queda atascada **exactamente igual** que las vias manipuladas por el
inyector. Con eso cambia el enfoque: el bloqueo no era de nuestro manipuleo
de la pila de estados.

### 12.1 Instrumentos nuevos

* **`injector-linux/src/diagnostico.rs`** (nuevo): vuelca la pila de estados
  del `AppManager` cada N ticks (`XFACTORVR_DIAG_PILA=1`,
  `XFACTORVR_DIAG_CADA=N`). Los estados se identifican **por la tabla
  virtual** (`_ZTV...` del `.symtab`), no llamando a `getStateType()`: en esa
  pila hay punteros a estados ya destruidos y llamar a un metodo virtual
  sobre uno de ellos **SIGSEGV** (suceso que paso al principio y por eso el
  diagnostico es solo-lectura).
* **`elfutils` (`eu-stack`) en el contenedor**: da la pila de todos los hilos
  en segundos; `gdb` tarda 3 minutos en leer el DWARF de este binario de
  261 MB (con `--readnever` arranca rapido pero ya no desenrolla las pilas).
* **`xdotool`** + `imagemagick` + `x11-apps` en el contenedor: para poder
  **conducir el menu del juego con el raton** (apartado 12.7).

### 12.2 Lo que hace el motor con `--load-scenario` (pila de estados medida)

Con el diagnostico del inyector (el motor oficial, sin tocar la pila):

```
diag: tick=1 t=0ms   estados=3
diag:   [0] tipo=InMainMenu
diag:   [1] tipo=InUpdater
diag:   [2] tipo=InGenerateMapDialog
diag: tick=2 t=7ms   estados=4
diag:   [0] tipo=InMainMenu
diag:   [1] tipo=InUpdater
diag:   [2] tipo=InGenerateMapDialog
diag:   [3] tipo=InCreatingGame          <-- el que carga el escenario
        25.772 Applying migration: Base Mod: 2.0.0.lua
diag: tick=6 t=94ms  estados=2
diag:   [0] tipo=InMainMenu
diag:   [1] tipo=InGame                  <-- FIN DE LA CARGA, sin mapa
```

O sea, el camino oficial del cliente grafico es:

1. `InMainMenu` + `InUpdater` (el actualizador).
2. El motor mete el dialogo de **"generar mapa"** (`InGenerateMapDialog`),
   que es la via que usa el GUI cuando pulsas "New Game".
3. Ese dialogo mete `InCreatingGame`, que es quien carga el escenario
   (`level.dat`, scripts, migraciones).
4. En cuanto la carga llega a la fase de scripts ("Applying migration:
   Base Mod: 2.0.0.lua"), el motor **saca el dialogo y toda la pila** y deja
   `[InMainMenu, InGame]`.

Y un `InGame` sin vista de control (`GlobalContext+0x2c0 == NULL`) es un estado
**muerto**: `InGame::process()` (AppManagerStates.cpp:82-94) sin la vista de
control no crea el mapa, asi que el mapa no existe, el RCON no abre y el
juego se queda pintando el menu para siempre.

### 12.3 La correccion importante: el guardia de la seccion 11.3 estaba mal leido

La seccion 11.3 de este ADR decia que `InLoadingGameBox::process()` mira el
estado de **la cima** de la pila. **No es asi.** El codigo real
(AppManagerStates.cpp:1674-1679, desensamblado a mano) es:

```
2b38c9c: cbz  w24, ...          ; si el cargador NO ha terminado, no hace nada
2b38ca4: bl   AppManager::popState()      ; saca 2 estados
2b38cb0: bl   AppManager::popState()
2b38cb4: ldr  x20, [x19, #0x258] ; INICIO de la pila (no la cima)
2b38cb8: ldr  x21, [x19, #0x260] ; fin de la pila
2b38cc4: ldr  x0, [x20]         ; primer estado
2b38ccc: ldr  x8, [x8, #0x40]   ; getStateType()
2b38cd4: cmp  w0, #0x4          ; InGame?
2b38cd8: b.eq <seguir>
2b38cec: cmp  w0, #0x5 ... 6 ... 8   ; otros tipos que tambien valen
2b38d28: b.ne <cancelar>       ; si no hay NINGUNO -> cancelar la carga
```

O sea: **cuando la carga termina, el motor saca 2 estados de la pila y exige
que sobre algum estado de tipo 4 (`InGame`), 5, 6 u 8**. Si no queda ninguno,
la carga se cancela.

Y el motor **si** construye esa pila cuando el juego se carga desde el GUI:
`InLoadGameDialog::process()` (AppManagerStates.cpp:1442-1521) hace, en este
orden:

1. `AppManager::popStateToMainMenu()` (AppManagerStates.cpp:1461)
2. `operator new(16)` + vtable de `InGame` + `pushState(InGame)`
   (AppManagerStates.cpp:1461-1466) — **el `InGame` del motor es un objeto de
   16 bytes con solo la vtable, sin constructor**: exactamente el mismo
   "fake" que crea el inyector.
3. `pushState(X)` (AppManagerStates.cpp:1492)
4. `operator new(...)` + `InLoadingGameBox::InLoadingGameBox(...)` +
   `pushState(InLoadingGameBox)` (AppManagerStates.cpp:1464-1467)

O sea, la pila buena es **`[InGame, X, InLoadingGameBox]`** (tres estados), y
al sacar los 2 ultimos sobrevive el `InGame` (tipo 4) y la carga sigue.

**Las vias de linea de comandos no construyen esa pila**: solo meten la caja
de carga encima del menu. Medido con `--load-game`:

```
tick=23  [InMainMenu, InLoadingGameBox]   cargador=0x...  estado_cargador=0
   26.179 Checksum for script __level__/control.lua: 3704017257
tick=24  [InMainMenu, InLoadingGameBox]   cargador=0x...  estado_cargador=1
tick=25  [InMainMenu, InGame]             <-- se cancela igualmente
```

### 12.4 No es el entorno de prueba (matriz de drivers)

Se probo el mismo `--load-scenario base/xfv-nivel` con cuatro combinaciones
de SDL/Mesa en el contenedor del banco:

| combinacion                                   | ultima linea del log                   | RCON |
|-----------------------------------------------|----------------------------------------|------|
| x11 + zink + lavapipe (la del banco)          | `Applying migration: Base Mod: 2.0.0.lua` | no |
| x11 + **sin zink** (GLX por software de Mesa)  | `Applying migration: Base Mod: 2.0.0.lua` | no |
| x11 + zink + lavapipe **sin vsync**           | `Applying migration: Base Mod: 2.0.0.lua` | no |
| **offscreen** (sin servidor X) + zink         | `Applying migration: Base Mod: 2.0.0.lua` | no |

**Las cuatro se atascan en el mismo punto**, asi que el driver grafico no es
la causa (ni Zink, ni lavapipe, ni el vsync, ni el servidor X). El tamano
tampoco: a 320x180 el bucle corre a ~80 fps y se atasca igual.

### 12.5 El headless carga el MISMO escenario sin problema

Con el mismo `data/` y el mismo escenario:

```
$ ./bin/arm64/factorio-headless --config /factorio/config/config.ini \
      --start-server-load-scenario base/xfv-nivel --rcon-port 27016 ...
   0.278 Loading level.dat: 1413855 bytes.
   0.315 Applying migration: Base Mod: 2.0.0.lua
   0.584 Info ServerMultiplayerManager.cpp:809: updateTick(157301) changing state from(CreatingGame) to(InGame)
   0.585 Info RemoteCommandProcessor.cpp:119: Starting RCON interface at IP ADDR:({0.0.0.0:27016})
```

El headless pasa por el **mismo** "Applying migration" y sigue hasta
`InGame` + RCON en 0,58 s. Los datos son los mismos; lo que cambia es que el
headless **no tiene GUI** y por tanto no pasa por
`InGenerateMapDialog`/`InCreatingGame` ni por la pila de estados del GUI.

### 12.6 El hilo del cargador desaparece

Con `eu-stack` sobre el proceso atascado **no hay ningun hilo** en
`ParallelScenarioLoader::doLoad` ni en `Scenario::loadFactory`: todos los
hilos del motor estan ociosos (`WorkerThread::loop()`,
`MapGenerationHelper::consumeTasks()`, `PackagePathAdapter::readFile()`) y el
hilo principal esta en `MainLoop::run` -> `swapRenderBuffers()` ->
`kopperSwapBuffersWithDamage()` (dentro de `libgallium`), esperando al
rasterizador por software.

Es decir: el hilo que carga el escenario **termina** (y el motor se queda con
un `InGame` sin mapa), en vez de quedarse esperando.

### 12.7 Ni siquiera el boton "Continue" del GUI funciona (conduciendo el menu con el raton)

Con `xdotool` + una captura (`import -window root`) se ven los botones del
menu: "Continue xfv-nivel" (verde, el de arriba), "Single player",
"Multiplayer", "Map editor", "Settings", "Mods", "About", "Exit".

Con un clic en "Continue" el motor carga el save **por su propia via del GUI**:

```
  45.986 Loading map /factorio/saves/xfv-nivel.zip: 495397 bytes.
  45.986 Loading level.dat: 1035122 bytes.
  46.001 Loading script.dat: 401 bytes.
  46.004 Checksum for script __level__/control.lua: 2743417419
```

y la pila acaba igual: **`[InMainMenu, InGame]`**, sin mapa y sin RCON
(`Starting RCON interface` = 0 veces en el log).

**Conclusion:** en este entorno el cliente grafico **no** consigue cargar una
partida por NINGUNA via (ni `--load-game`, ni `--load-scenario`, ni el boton
"Continue" del propio menu).

### 12.8 Lo que SI funciona: el modo pesado (`--heavy`)

`--load-game ... --heavy` y `--load-scenario ... --heavy` **crean el mapa y lo
hacen correr**:

```
  28.846 Heavy mode - tick 157301 finished
  ...
  63.239 Heavy mode - tick 158216 finished
```

(miles de ticks simulados). O sea: el motor **puede** crear el mapa y
ejecutarlo aqui; lo que no llega a funcionar es el camino de un jugador.

### 12.9 Otros dos bloqueos encontrados de paso

* **`--load-scenario base/campaigns/tutorial/level-01` no es valido** por linea
  de comandos: `Error CommandLineUtils.cpp:80: Invalid scenario level name`.
  El CLI solo acepta escenarios, no niveles de campana. Por eso la prueba de
  la campana tiene que hacerse con una **copia** del nivel puesta como
  escenario (`base/xfv-nivel`).
* **El actualizador mete un `InUpdaterLoginGui`**: al arrancar, el motor
  empuja `InUpdater` y su dialogo de login de la cuenta Factorio, que es
  modal y bloquea la pila de estados. Se quita con `check-updates=false` en el
  `config.ini`. (Con el actualizador apagado, la pila queda
  `[InMainMenu, InUpdater]` y el juego se queda en el menu.)

## 13. Conclusion y siguiente paso

1. **El bloqueo NO es del entorno grafico** (12.4) ni de los datos (12.5): el
   motor se atasa en el mismo punto con zink, sin zink, sin vsync, en
   offscreen y a 320x180.
2. **El bloqueo NO es de nuestro inyector**: las vias oficiales del motor se
   atascan igual (12.2, 12.7).
3. **La causa concreta**: el motor cancela la carga cuando al terminarla no
   queda en la pila ningun estado de tipo 4/5/6/8 (12.3). Las vias de linea de
   comandos y el propio menu dejan la pila en `[InMainMenu, InGame]`, con un
   `InGame` sin vista de control (`GlobalContext+0x2c0 == NULL`), que es un
   estado muerto: no crea el mapa, no abre el RCON y no se queja.
4. **El mapa si se puede crear y correr aqui** (12.8, modo pesado).
5. **El headless va bien** (12.5): carga, crea el mapa y abre el RCON.

**Lo que queda por hacer para el cliente grafico** es una de estas dos:

* **A)** Que el inyector Monte la pila **exactamente** como lo hace
  `InLoadGameDialog::process()`: `InGame` (16 bytes + vtable) -> estado `X` ->
  `InLoadingGameBox`, y **ademas** panne la vista de control
  (`GlobalContext+0x2c0`), que hoy no se crea en ninguna via y de la que
  depende `InGame::process()`. La pista: el constructor del `GameView` es
  `_ZN8GameViewC2ER4GameP6Player9NamedBoolI15IsSImulationTagEP18EngineFramebuffersS4_I17MuteWindSoundsTagE`
  (necesita un `Game*` y un `Player*` de verdad, que solo crea el motor).
* **B)** Usar el **headless** para la parte de servidor (carga + RCON, que ya
  funciona) y el cliente grafico solo como visor.

Mientras tanto, el inyector ya trae dos piezas nuevas que hacen falta para
cualquiera de las dos:

* `AppManager::startRconIfEnabled()` resuelta y llamada desde el hook de
  render (`tick_rcon()`): es la **unica** via que tiene el motor para abrir
  el RCON (la llama el `InLoadingGameBox::process()` cuando la carga
  termina). Con el autostart por `createGame` se llega a llamarla, pero el
  puerto no abre todavia porque el `RemoteCommandProcessor` todavia no esta
  en marcha (hace falta que el mapa corra).
* `injector-linux/src/diagnostico.rs`: el volcado de la pila de estados, que
  es lo que ha permitido llegar hasta aqui.

## 14. Avance final: el mapa SI se crea y el juego corre (2026-10-20)

Con `XFACTORVR_ESTADO_INGAME=1` (el `InGame` de 16 bytes con la vtable
encima de la caja de carga, que es exactamente como lo construye el propio
motor en `InLoadGameDialog::process()`) **la carga se completa, el motor crea
el mapa y el juego se pone a pintar el mapa**:

```
  30.327 Loading map /factorio/saves/client_test.zip: 523809 bytes.
  30.338 Info Scenario.cpp:152: Map version 2.1.22-1
  30.347 Checksum for script __level__/control.lua: 3704017257
```

y la captura de pantalla (`import -window root`) muestra el **mapa renderizado**
con el minimapa, la barra de atajos, el inventario y el dialogo de bienvenida
("Welcome to Factorio Freeplay. Your task is to launch a rocket into
space..."). El juego aguanta mas de 2 minutos sin caerse.

### 14.1 El crash que queda si NO se pone el `InGame`

Sin `XFACTORVR_ESTADO_INGAME=1` el motor crea el mapa tambien, pero el juego
peta a los ~36 ticks:

```
  32.070 Error CrashHandler.cpp:614: Received SIGSEGV
  src/GameView.cpp (1813): render
  src/Game.cpp (435): render
  src/MainLoop.cpp (1539): render
  src/MainLoop.cpp (728): mainLoopStep
  36.300 Error CrashHandler.cpp:163: Map tick at moment of crash: 36
```

Es el `InGame` a medio construir (16 bytes, solo vtable) el que se queda
colgado: `InGame::process()` (AppManagerStates.cpp:82-94) necesita la vista de
control (`GlobalContext+0x2c0`) y, si no la encuentra, hace
`popStateToMainMenu()` y el motor sigue llamando a su `process()` sobre
memoria ya liberada. Con el `InGame` de verdad (el que crea el motor) no hay
crash; con el del inyector, si.

### 14.2 El RCON: como se abre y por que todavia no

`AppManager::startRconIfEnabled()` (AppManager.cpp:662-674) es la unica via
para abrir el RCON. La funcion:

1. lee el **socket** de `GlobalContext+0x240` -> `+0x2348` (una
   `std::string` tipo `"0.0.0.0:27015"`),
2. lee la **password** de `+0x2428` (longitud) y `+0x2430` (puntero),
3. y llama a `RemoteCommandProcessor::addInterface<RconInterface>(...)`.

El inyector ya la resuelve y la llama (`tick_rcon()`, reintentando cada 30
frames porque el `RemoteCommandProcessor` todavia no esta en marcha en el
instante en que el motor empuja el `InGame`; el motor, en su flujo normal,
tambien la llama en cada frame). El diagnostico nuevo (`diag: rcon:`) lee las
dos cadenas para ver que hay puesto.

**Pendiente**: en este entorno la cadena del socket sale **vacia/garbage**
(aunque el parametro `--rcon-port 27015` este en la linea de comandos), y la
clave `local-rcon-socket` del `config.ini` es para el **servidor de
multiplayer del menu** ("Socket to host RCON when launching MP server from the
menu"), no para el single player. Es el punto exacto donde hay que seguir:
encontrar de donde saca el motor el puerto del RCON en single player (el
`GameSettings` / la linea de comandos) y por que no llega a la cadena.

### 14.3 Como queda el estado

| via                                        | mapa | RCON | nota                                    |
|--------------------------------------------|------|------|-----------------------------------------|
| `--load-scenario` (oficial)                 | no   | no   | se cancela la carga (12.2)              |
| `--load-game` (oficial)                     | no   | no   | se cancela la carga (12.3)              |
| boton "Continue" del GUI (con xdotool)      | no   | no   | se cancela la carga (12.7)              |
| `--load-scenario/--load-game --heavy`       | **si** | no | el motor crea el mapa y lo simula (12.8) |
| inyector + `XFACTORVR_ESTADO_INGAME=1`     | **si** | casi | el juego corre; falta que el RCON abra |
| headless (`--start-server-load-scenario`)   | **si** | **si** | funciona perfecto (12.5)               |
