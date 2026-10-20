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

## 10. Como se comprueba

```bash
bash scripts/test_injector.sh          # el hook sigue vivo (no se ha roto nada)
bash scripts/test_autostart.sh          # el juego arranca solo y el RCON responde
```

`scripts/test_autostart.sh` **no se da por bueno un caso que no se ha
comprobado**: sale con 0 solo si el log del cliente muestra la carga del
nivel **y** el RCON responde de verdad; con 1 si no arranca o si el RCON no
contesta; con 2 si no se ha podido ni hacer la prueba.
