# ADR-003 — Inyector LD_PRELOAD para Factorio 2.1.22 ARM64 (Linux)

- **Estado**: aceptado (implementado y **validado** en `injector-linux/`)
- **Fecha**: 2026-10-19
- **Contexto**: port del "mod 3D" de GDani31 (`factorio-3d-mod`) de Windows a
  Linux ARM64, para correr dentro del visor Android con Mesa+Zink.

---

## 1. Por qué `LD_PRELOAD` y no una DLL con PDB

El upstream (Windows) engancha el motor con una DLL que:

1. Lee `factorio.pdb` para obtener **nombres de símbolo** (no direcciones).
2. Resuelve cada función por nombre y, si el símbolo desaparece en una
   actualización, cae a un **RVA de reserva** (`src/offsets.rs`).

En Linux ARM64 no hay PDB, pero el problema es menor: **el motor no va
stripeado**. Su `.symtab` conserva **94.946 símbolos C++ con nombre** (de los
que 94.821 son funciones en el build headless y 94.939 en el cliente).

| | Windows (upstream) | Linux ARM64 (este ADR) |
|---|---|---|
| Símbolos | `factorio.pdb` (nombres) | `.symtab` del propio ELF (nombres mangled) |
| Cómo se llega | `GetModuleHandleA` + `pdb` crate | `dl_iterate_phdr` + parser de ELF propio |
| Formato | PDB (MSVC) | ELF64 / ABI Itanium (mangled) |
| Reserva | RVA hardcodeado | no hace falta: el symtab sigue ahí |
| Carga | `dll_syringe` (inyecta en el proceso) | `LD_PRELOAD` (el cargador lo carga) |

Con `LD_PRELOAD` el orden es trivial: la `.so` entra en el proceso **antes**
que el juego, y su constructor (`.init_array`) puede parchear el `.text` del
ejecutable principal antes de que este ejecute una sola instrucción.

Ventaja frente al `dll_syringe`: no hace falta buscar el PID ni usar
`ptrace`/`/proc/<pid>/mem`; el propio cargador hace el trabajo sucio.

---

## 2. Detour en ARM64 (AArch64)

### 2.1 El problema del salto absoluto

En AArch64 todas las instrucciones son de **4 bytes y alineadas a 4**, así que
un salto nunca cae en mitad de una instrucción (el problema clásico de x86-64
con el `jmp` de 5 bytes). Aun así, un salto **absoluto** a una función de
nuestra libreria no cabe en una instrucción:

- `MOV` (MOVZ) solo codifica **16 bits** de inmediato.
- Un `B` condicional tiene alcance relativo limitado.

La forma canónica de AArch64 para saltar a una dirección de 64 bits es
**cargar el destino en un registro y ramificar a través de él**:

```asm
        ldr  x16, #<literal>      ; x16 = direccion del hook (literal PC-relativo)
        br   x16                 ; salta ahi
```

`LDR (literal, 64 bits)` tiene la codificación `0x58000000 | (imm19 << 5) | Rt`
y `BR Xn` es `0xD61F0000 | (Rn << 5)`.

### 2.2 Los dos parches que usa el inyector

El literal (los 8 bytes con la dirección) tiene que vivir en algún sitio:

**Corto (8 bytes de código)** — el literal está en **nuestra** `.so`:

```asm
destino+0:  ldr  x16, #lit      ; 0x58000050 | (imm19 << 5)
destino+4:  br   x16            ; 0xD61F0200
```

- Solo se **reubican 8 bytes** (2 instrucciones) del original.
- Exige que el literal esté a menos de **1 MB** (alcance de `LDR` PC-relativo:
  `imm19` con signo × 4 → ±1 MB).
- El literal se reserva en un pool de 64 entradas (`static LITERALES:
  [AtomicU64; 64]` en la `.so`): su dirección es estable de por vida y su
  contenido se escribe al instalar el detour.

**Largo (16 bytes)** — el literal va **en el sitio**, en el hueco que deja el
código:

```asm
destino+0:  ldr  x16, #2        ; literal en destino+8
destino+4:  br   x16
destino+8:  .quad <direccion del hook>   ; pool literal
```

- Se reubican **16 bytes** (4 instrucciones) del original.
- Funciona siempre, sin límite de distancia.

El inyector elige **corto** si el literal queda a menos de 1 MB y **largo** en
caso contrario. Se puede forzar con `XFACTORVR_HOOK_PATCH=corto|largo`.

> **Por qué no 8 bytes "a lo bruto"**: el enunciado original pedía un parche
> de "8 bytes con `mov x16, #imm; br x16`". En AArch64 eso es imposible para
> una dirección de 64 bits: `MOVZ` solo da 16 bits y harían falta 4 `MOVZ/MOVK`
> más el `BR` (20 bytes). La forma de 8 bytes que sí funciona es
> `ldr x16, #literal` + `br x16`, que es exactamente la misma idea (saltar a
> través de x16) con la única diferencia de que la constante se lee de un
> literal en vez de codificarse en la instrucción. El literal de 8 bytes es
> inevitable; por eso el parche corto necesita el pool en nuestra `.so` y el
> largo lo lleva dentro.

### 2.3 Reubicar el bloque: el problema de las instrucciones PC-relativas

Al copiar el principio de una función a otro sitio, las instrucciones que
calculan direcciones relativas al PC dejan de apuntar a lo mismo. En el
motor real esto **pasa en el primer frame**: el prólogo de
`GameRenderer::render()` es

```asm
17837e0: stp  x29, x30, [sp, #-0x20]!
17837e4: stp  x20, x19, [sp, #0x10]
17837e8: mov  x29, sp
17837ec: adrp x8, 0x3eba000        <-- PC-relativo
```

El `adrp` de la cuarta instrucción apunta a un global del juego. Copiada tal
cual al trampolín apuntaría a otra página ✗ (y el juego crashea al primer
frame).

Por eso el trampolín **reescribe** las instrucciones PC-relativas para que
apunten al mismo destino desde su nueva dirección:

| Instrucción | Corrección | Alcance |
|---|---|---|
| `adrp Xd, pagina` | se recalcula `imm21` (21 bits) | ±4 GB |
| `adr Xd, etiqueta` | se recalcula `imm21` (21 bits) | ±1 MB |
| `ldr`/`prfm` de literal | si el literal está **dentro** del bloque reubicado se mantiene el desplazamiento; si está fuera, se recalcula `imm19` | ±1 MB |

Si algún destino queda fuera de alcance, el inyector **renuncia** (error
claro) en vez de generar código roto.

> **Trampa (la que costó el crash)**: `imm21` son **21 bits**, no 19. Con la
> máscara equivocada (`0x1_FFFF` = 17 bits, o `0x7_FFFF` = 19) el `adrp`
> reubicado apuntaba a una página equivocada y el juego moría con SIGSEGV en
> el primer frame, con un PC de salto en la pila. La máscara correcta es
> `0x1F_FFFF`, y el `immhi` se limita a 19 bits (`0x7_FFFF`) al re-codificar.

### 2.4 El trampolín

Página mapeada con `PROT_READ|PROT_WRITE|PROT_EXEC` (una sola para todos los
detours, con un contador):

```asm
trampolin+0:   <bloque original reubicado>   ; 8 o 16 bytes
trampolin+N:   ldr  x17, #2                   ; literal en trampolin+N+8
trampolin+N+4: br   x17
trampolin+N+8: .quad <destino + N>            ; vuelta al original
```

El hook llama al trampolín con un `bl` normal, así que `x30` (lr) apunta a la
instrucción siguiente del hook; el original termina con su `ret` y vuelve al
hook, que a su vez retorna al juego. `x16`/`x17` son registros temporales
según la AAPCS64: se pueden machacar sin romper la convención.

**Dónde vive el trampolín importa**: el `adrp` reubicado solo se puede
corregir si el trampolín está a menos de 4 GB del código original. Como el
motor ocupa ~768 MB de dirección virtual contiguos, una pista de `mmap` a
2 MB cae dentro de su mapa; con una pista mayor (16 MB, 64 MB, 256 MB, 1 GB)
el kernel coloca la página en el hueco libre **más cercano** (a unos pocos
cientos de MB) y la corrección cabe ✓. El inyector prueba varias pistas y
solo acepta una que quede a menos de 2 GB.

### 2.5 Renunciar antes de corromper

Antes de escribir nada se comprueba que **todas** las instrucciones a reubicar
sean reubicables. Se rechaza (y se devuelve un error claro, sin parchear) si
aparece:

| Patrón | Por qué no se puede reubicar |
|---|---|
| `B`, `BL` | salto relativo |
| `B.cond` | salto condicional relativo |
| `CBZ`/`CBNZ`, `TBZ`/`TBNZ` | saltos relativos |
| `BR`, `BLR`, `RET` | saltos a registro / retorno |
| `SVC`, `BRK`, `HVC`… | generan excepción |

Además, si la función es **más corta que el parche** (8 o 16 bytes), el
inyector **renuncia**: se necesita el bloque original entero para reubicarlo.
Hay funciones reales del motor en ese caso, por ejemplo
`Entity::getDirection() const` (0x16f3068 en el build headless) son solo
8 bytes (`mov w0, wzr` / `ret`).

### 2.6 Permisos y coherencia de caché

1. Se localizan las páginas que cubren el parche con `/proc/self/maps`.
2. `mprotect(paginas, PROT_READ|PROT_WRITE|PROT_EXEC)` (la página original es
   `r-x`).
3. Se escriben los bytes del parche.
4. Se **invalida la caché de instrucciones**: en ARM64 la I-cache y la D-cache
   **no son coherentes** entre sí. glibc **no exporta** `__clear_cache` en
   aarch64 (es un símbolo de compiler-rt), así que se hace a mano con la
   secuencia estándar (la misma que usan los motores JIT):

   ```asm
   dc cvau, <linea>      ; limpia la D-cache hasta el punto de unicidad
   dsb ish               ; que la limpieza se complete
   ic ivau, <linea>      ; invalida la I-cache
   dsb ish
   isb                   ; vacia la tuberia de instrucciones
   ```

   (Linux deja el bit `SCTLR_EL1.UCI` activo, así que `dc cvau` está permitido
   en modo usuario.)
5. Se restauran los permisos originales (`r-x`).

### 2.7 Pointer authentication (PAC): por qué esto sigue siendo transparente

El motor ARM64 **sí usa PAC**: 2.602 `paciasp` y 3.264 `autiasp` en el binario.
La convención observada en `_fini` es:

```asm
_fini:
        paciasp                     ; firma lr con el SP como contexto
        stp  x29, x30, [sp, #-0x10]!
        mov  x29, sp
        ldp  x29, x30, [sp], #0x10
        autiasp                     ; verifica lr (SP ya restaurado) y lo firma
        ret
```

O sea: **el calibrador firma la dirección de retorno al entrar y la verifica
al salir**, usando el SP como contexto (el de entrada en ambos extremos).

Nuestro trampolín reproduce eso exactamente:

- La `paciasp` original **se reubica tal cual** y se ejecuta con el mismo SP
  que tendría la función original (el hook no modifica el SP antes de llamar
  al trampolín, y el SP se restaura en el epílogo).
- La `autiasp` final **no se toca**: sigue en el cuerpo original, y verifica
  contra ese mismo SP.
- El juego recibe exactamente la misma dirección de retorno que en el flujo
  sin parchear.

Único matiz: durante la ejecución del original, `lr` contiene la dirección de
retorno *dentro del hook* en vez de la del juego. Solo afecta a código que
leyese `lr` dentro de la función original (prácticamente inexistente).

`GameRenderer::render()` (el objetivo del hook de prueba) **no** usa PAC.

---

## 3. Resolución de símbolos en tiempo de ejecución

### 3.1 Localizar el ejecutable principal

`dl_iterate_phdr` recorre los objetos cargados; el **primero** es el
ejecutable principal y llega con `dlpi_name == ""`. De ahí sale:

- `load_bias = dlpi_addr - p_vaddr del primer PT_LOAD` (en el motor el primer
  PT_LOAD tiene `p_vaddr = 0`, luego `load_bias == dlpi_addr`).
- La ruta en disco se saca de **`/proc/self/exe`** (más fiable que
  `dlpi_name`: sobrevive a renombrados y borrados, porque es el symlink al
  inode).

### 3.2 Leer el `.symtab`

`src/elf.rs` parsea el ELF64 little-endian a mano (sin dependencias):

- Cabecera ELF64 (64 bytes) → `e_shoff`, `e_shentsize`, `e_shnum`.
- Tabla de cabeceras de sección → se busca `.symtab` (`SHT_SYMTAB`) y su
  `.strtab` enlazada por `sh_link`.
- Se leen **solo** los rangos de `.symtab` y `.strtab`, no el fichero entero
  (260 MB).
- Se filtran los símbolos `STT_FUNC` definidos (`st_shndx != SHN_UNDEF`).

### 3.3 Des-mangling (ABI Itanium)

Los símbolos del symtab vienen mangled: `_ZN12GameRenderer6renderEv`.
`src/demangle.rs` implementa el subconjunto del ABI de Itanium necesario:

- Nombres anidados `N ... E`, con calificadores `K` (const), `V`, `r`.
- `<source-name>` (`<longitud><nombre>`), nombres de operador, constructores.
- Sustituciones `S_` / `S<n>_` (tabla de sustituciones).
- Argumentos de plantilla `I ... E`, con `I<n>` y `std::allocator` vía
  sustitución.
- Tipos: predefinidos, punteros, referencias, `const`/`volatile`, arrays,
  miembros, y la lista de argumentos completa.

El des-mangler es **total**: nunca entra en panic ni se queda en bucle; si
encuentra algo que no entiende devuelve `None` y el llamante sigue con el
nombre mangled. Lo que importa (el camino de clases `Owner::metodo`) se
construye siempre con nombres literales, así que es exacto.

Resultado: `GameRenderer::render()` — idéntico al formato de `nm -C`.

### 3.4 Buscar sin des-manglear 94.939 símbolos

Des-manglear todo en cada arranque sería lento. Se hace un **pre-filtrado
barato**: el nombre mangled contiene literalmente los trozos del nombre
buscado (`_ZN12GameRenderer6renderEv` contiene "GameRenderer" y "render"), y
solo se des-manglean los candidatos. La coincidencia final es **exacta** sobre
el nombre cualificado (`Owner::metodo`), lo que además evita el error clásico
de `Turret::draw` ↔ `AmmoTurret::draw` (la búsqueda exige que el nombre de la
clase no esté pegado a otro).

Ejemplo real (`cargo run --example parse_elf`):

```
simbolos de funcion: 94821
resolviendo 'GameRenderer::render'...
  0x177d7b0  GameRenderer::render()
```

Las direcciones coinciden **exactamente** con las de `nm -C` ✓.

### 3.5 De dirección virtual a dirección real

`direccion_real = load_bias + st_value`, porque el motor es un PIE
(`ET_DYN`) cuyo primer `PT_LOAD` tiene `p_vaddr = 0`.

---

## 4. Tabla de hooks de 2.1.22

Regenerada con:

```bash
python3 scripts/resolve_hooks.py \
    --elf factorio_client_arm64/factorio/bin/arm64/factorio \
    --json build/hooks_2.1.22.json
```

**Resultado: 20/20 resueltos.** Para los que no son función, el script
**verifica el offset desensamblando** la función que lo usa.

> **Ojo: las direcciones son por binario.** El repo tiene tres copias del
> motor: el **cliente gráfico** (`factorio_client_arm64/…/factorio`), el
> **headless** (`factorio_arm64/…/factorio`) y `build/factorio_stripped.elf`,
> que es una copia *sin debug info* del headless. Headless y cliente son
> **builds distintos** (94.821 vs 94.939 símbolos de función) y sus
> direcciones **no coinciden**. La tabla de arriba es la del **cliente**, que
> es el que se hookea en el visor. El inyector, en cambio, resuelve siempre
> en el binario que esté corriendo en ese momento ✓.

| Hook | Dirección (cliente) | Nombre en 2.1.22 |
|---|---|---|
| `GAME_RENDERER_RENDER` | `0x17837e0` | `GameRenderer::render()` |
| `GAME_RENDERER_PREPARE` | `0x177fde4` | `GameRenderer::prepare(RenderParameters const&)` |
| `GAME_RENDERER_RENDER_INTERNAL` | `0x1785ee0` | `GameRenderer::renderInternal(bool)` |
| `RP_CENTER_ON` | `0x17788d8` | `RenderParameters::centerOn(Map const&, SurfaceView const&)` |
| `GET_MAP_POSITION` | `0x17726e8` | `GameView::getMapPosition(PixelPosition) const` |
| `GAME_VIEW_CALCULATE_SURFACE_VIEW` | `0x1767c30` | `GameView::calculateSurfaceView() const` |
| `ASSEMBLING_MACHINE_DRAW` | `0x2406754` | `AssemblingMachine::draw(DrawQueue&) const` |
| `FURNACE_DRAW` | `0x313fb10` | `Furnace::draw(DrawQueue&) const` |
| `MINING_DRILL_DRAW` | `0x1eda428` | `MiningDrill::draw(DrawQueue&) const` |
| `TURRET_DRAW` | `0x2f6bdec` | `Turret::draw(DrawQueue&) const` |
| `AMMO_TURRET_DRAW` | `0x31ec918` | `AmmoTurret::draw(DrawQueue&) const` |
| `CRAFTING_MACHINE_DRAW` | `0x240737c` | `CraftingMachine::draw(DrawQueue&, Entity const&, UpdatableEntity const&, WorkingVisualisations const&, CraftingMachineAnimationState, bool, EnergySource const*, RecipeTints const&, WorkingVisualisationEnabledIndices)` |
| `WORKING_VISUALISATION_DRAW` | `0x1ef52b0` | `WorkingVisualisationDrawState::draw(DrawQueue&, WorkingVisualisation const&, Surface const&, Color const*, float, Vector const&, DrawingFlags)` |
| `DRAW_ITEM_STACK` | `0x18fb28c` | `ItemStackPainter::drawItemStack(...)` |
| `TB_DRAW_ITEMS` | `0x2f7f898` | `TransportBelt::drawItems(...)` |
| `DRAW_SCALED_TINTED` | `0x184bbc8` | `DrawQueue::drawScaledTinted(...)` |
| `ENTITY_GET_PROTOTYPE` | `Entity+0x48` | **miembro** (`EntityPrototype const*`) |
| `ENTITY_GET_SURFACE` | `Entity+0x70` | **miembro** (`Surface*`) |
| `DAYTIME_SET_POSITION` | `0x26629b0` | `DayTime::setPosition(double)` |
| `PLAYER_INPUT_WALK_DIR` | `0x1c712c4` | `PlayerInputSource::computeWalkingDirectionFromPlayerInputs()` |

### 4.1 Cambios respecto a 2.0.77 (Windows)

| Upstream 2.0.77 | 2.1.22 | Qué pasó |
|---|---|---|
| `WorkingVisualisation::draw` | `WorkingVisualisationDrawState::draw` | la clase se partió; el dibujado de la visualización de trabajo es ahora una función de estado |
| `Entity::getPrototype()` | **miembro** en `Entity+0x48` | el compilador la inlinó: no queda símbolo |
| `Entity::getSurface()` | **miembro** en `Entity+0x70` | ídem |
| `?render@GameRenderer@@` | `GameRenderer::render()` | mismo nombre, otra dirección |

`GameRenderer::renderInternal(bool)` **sigue existiendo** en el cliente
(`0x1785ee0`); en el build headless está inlinada y solo queda la lambda que
recibe el `DrawQueue` (`_M_invoke`), de ahí el segundo nombre de búsqueda en
la tabla.

**Verificación de los miembros** (la hace el propio script, desensamblando):

- `Entity::prototypeCollide(BoundingBox const&) const` hace
  `ldr x22, [x0, #0x48]` → ese puntero se usa luego como `EntityPrototype`
  (`ldrb w8, [x22, #0x568]`, `add x1, x22, #0x278]`).
- `Entity::setupPosition(Surface&)` hace `str x1, [x0, #0x70]`
  → ahí se guarda la `Surface&`.

### 4.2 Mapeo de los hooks del upstream

El upstream engancha muchos más símbolos (`GENERIC_DRAWS` en
`src/offsets.rs`: `Lab`, `Boiler`, `Generator`, `Inserter`, `ElectricPole`,
`ContainerEntity`, `LogisticContainer`, `SolarPanel`, `Accumulator`, `Beacon`,
`Roboport`, `Radar`, `Wall`, `Gate`, `StorageTank`, `Pump`, `OffshorePump`,
`Reactor`, `TrainStop`, `Lamp`, `ProgrammableSpeaker`, `ArithmeticCombinator`,
`DeciderCombinator`, `ConstantCombinator`, `PowerSwitch`, `ResourceEntity`,
`Tree`, `SimpleEntity`, `Pipe`, `PipeToGround`, `HeatPipe`,
`TransportBeltConnectable`, `UndergroundBelt`, `Splitter`, `ItemEntity`,
`SpiderVehicle`, `Character`, `RocketSiloRocket`, …).

Todos se localizan **exactamente igual** que los 20 de la tabla: por nombre
des-mangled en el `.symtab` del motor. El trabajo pendiente es:

1. Volcar la lista completa de `GENERIC_DRAWS` del upstream a una tabla
   equivalente (mismo formato que `build/hooks_2.1.22.json`).
2. Declarar la **firma** de cada hook (parámetros y retorno) para poder
   reenviarlos al original: el patrón `draw(DrawQueue&)` de AArch64 es
   `x0 = this`, `x1 = DrawQueue&`, y el retorno va en `x0` salvo para tipos
   grandes.
3. Enganchar de verdad el pipeline (cámara, entidades, items, cables) — este
   ADR solo deja la **infraestructura** (resolver + parchear + contar).

---

## 5. El `.so`: compilar para glibc ARM64 desde macOS

El crate `injector-linux/` es **solo std** (ni una dependencia externa) y
compila para:

- `aarch64-unknown-linux-gnu` (el motor) ✓
- `aarch64-linux-android` (el visor) ✓
- el host (macOS) ✓ — las partes de Linux se compilan con `cfg(elf_target)` y
  fuera de él hay stubs, para que `cargo build` sirva de comprobación de
  sintaxis.

### 5.1 Enlazado cruzado

No hay toolchain de Linux en macOS, así que se enlaza con el **clang del NDK**
(que el proyecto ya usa para Android) contra el **sysroot de glibc ARM64** que
prepara `scripts/prepare_glibc.sh` (`build/glibc.tar.gz`, el paquete `glibc`
de Termux). `.cargo/config.toml` del crate fija:

```
linker = <NDK>/bin/clang
-Clink-arg=--target=aarch64-linux-gnu
-Clink-arg=--sysroot=<repo>/build/glibc_sysroot
-Clink-arg=-nostartfiles
-Clink-arg=<repo>/build/glibc_sysroot/link/libc.so
-Clink-arg=-lm
-Clink-arg=-L<repo>/build/glibc_sysroot/link
```

Dos detalles que costaron sangre:

1. **`libc.so` y `libm.so` de Termux** son *linker scripts* con rutas
   absolutas `/data/data/com.termux/...` que no existen en el host. Se
   sustituyen por equivalentes con rutas del sysroot (dentro del sysroot,
   `/lib/libm.so.6` se resuelve relativo a él).
2. **`libgcc_s`**: el sysroot no lo trae. Se crea un **archivo de biblioteca
   vacío** para que el `-lgcc_s` que pasa rustc encuentre algo, y los
   símbolos `_Unwind_*` se cubren con **stubs locales** (§5.2).

### 5.2 Los stubs del desenrollador (`src/gnu_unwind.rs`)

La `std` de Rust referencia `_Unwind_Resume`, `_Unwind_Backtrace`, etc. El
motor los lleva **enlazados estáticamente** y **no los exporta**, y tampoco
carga `libgcc_s.so.1` (sus `DT_NEEDED` son solo `libresolv`, `libm`, `libc`).
Si el `.so` los deja sin resolver, el cargador falla:

```
./bin/arm64/factorio: symbol lookup error: /injector.so: undefined symbol: _Unwind_Resume
```

Solución: stubs propios. Como el inyector va con `panic = "abort"`, **nunca
hay desenrollado**; los stubs solo existen para que el `.so` cargue, y si
alguno se llamara terminan el proceso con código 134 (como un `abort`).

Se emiten con `global_asm!` y **`.globl`** (para que el enlazador ligue las
referencias) dejando que el *version script* que genera Rust los deje en
`local` (no se exportan → no interponen el desenrollador del juego):

```asm
    .globl _Unwind_stub_impl
    .type _Unwind_stub_impl, %function
_Unwind_stub_impl:
    mov  x8, 94          // __NR_exit_group (aarch64)
    mov  x0, 134         // 128 + SIGABRT
    svc  #0
    .globl _Unwind_Resume
    .set  _Unwind_Resume, _Unwind_stub_impl
    ...
```

> **Trampa encontrada por el camino**: la primera versión usaba el
> `libunwind.a` del NDK como `libgcc_s`. Funcionaba, pero el libunwind de LLVM
> saca el `FILE*` de stderr del símbolo **interno** de glibc `__sF`, que en
> **aarch64 no está exportado** (en x86-64 sí). El `.so` no cargaba:
> `symbol lookup error: /injector.so: undefined symbol: __sF`. De ahí que el
> logging del inyector escriba con `write(2)` directo y no con
> `std::io::stderr()`.

### 5.3 Estado del `.so` resultante

```
$ llvm-objdump -T libxfactorvr_injector.so
DYNAMIC SYMBOL TABLE:
   xfactorvr_injector_init      (exportado)
   xfactorvr_injector_version    (exportado)
NEEDED  libc.so.6
```

- Sin `__sF` ✓
- Sin `_Unwind_*` sin resolver ✓ (todos locales)
- Requisitos de versión: `GLIBC_2.17 … GLIBC_2.34` (el contenedor tiene
  glibc 2.39 ✓)

---

## 6. El hook de prueba

`src/hooks.rs` engancha `GameRenderer::render` y:

- cuenta los frames en un `AtomicU64`;
- cada N frames escribe por stderr `hook vivo: N frames`;
- llama al original a través del trampolín, pasando `this` intacto.

Variables de entorno:

| Variable | Por defecto | Para qué |
|---|---|---|
| `XFACTORVR_HOOK_DEBUG` | `1` | `0` desactiva el "pull" de depuración |
| `XFACTORVR_HOOK_EVERY` | `60` | cada cuántos frames se imprime |
| `XFACTORVR_HOOK_TARGET` | `GameRenderer::render` | otro símbolo que enganchar |
| `XFACTORVR_HOOK_PATCH` | auto | `corto` / `largo` |
| `XFACTORVR_HOOK_TRACE` | `0` | vuelca parche y trampolín (diagnóstico) |
| `XFACTORVR_HOOK_NOPATCH` | `0` | resuelve pero **no** parchea (diagnóstico) |
| `XFACTORVR_HOOK_NOCALL` | `0` | el hook **no** llama al original (diagnóstico) |

---

## 7. Validación

`scripts/test_injector.sh`:

1. Prepara el sysroot de glibc y compila el `.so` para
   `aarch64-unknown-linux-gnu`.
2. Arranca la máquina de podman si está apagada.
3. Si el contenedor no existe → **aviso claro y código de salida 2**
   (no se da por validado algo que no lo está).
4. Copia el `.so` y lanza el **cliente gráfico real** dentro del contenedor
   con el entorno de Mesa/Zink:
   `SDL_VIDEODRIVER=offscreen`, `EGL_PLATFORM=surfaceless`,
   `LIBGL_ALWAYS_SOFTWARE=1`, `MESA_LOADER_DRIVER_OVERRIDE=zink`,
   `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`,
   `LD_PRELOAD=/injector.so`.
5. Busca `hook vivo: N frames` con **N > 0**.

Códigos de salida: `0` validado, `1` fallo real, `2` no validado (sin
contenedor/máquina).

Salida real de la validación (contenedor `xfactorvr-client`, glibc 2.39, Mesa
25.2.8 con Zink sobre lavapipe):

```
[xfv-injector] info: arrancando: objetivo='GameRenderer::render' debug=si cada=10
[xfv-injector] info: ejecutable: /factorio/bin/arm64/factorio base=0xaaaaabe40000
[xfv-injector] info: simbolos de funcion leidos: 94939
[xfv-injector] info: resuelto: GameRenderer::render() -> 0xaaaaad5c37e0 (mangled _ZN12GameRenderer6renderEv)
[xfv-injector] info: bloque original (16 bytes): fd7bbea9f44f01a9fd030091a83901f0 -> reubicado en 0xaaaab15c3000 (bloque de 32 bytes)
[xfv-injector] info: detour instalado: parche de 16 bytes (Largo), trampolin en 0xaaaab15c3000
[xfv-injector] info: hook activo: cada frame pasa por hook_render()
  55.786 Factorio initialised
[xfactorvr] hook vivo: 1 frames
[xfactorvr] hook vivo: 10 frames
[xfactorvr] hook vivo: 20 frames
...
OK: hook vivo -> hook vivo: 20 frames
```

---

## 8. Limitaciones conocidas y trabajo pendiente

| # | Limitación | Impacto |
|---|---|---|
| 1 | El parche se escribe con el juego ya en marcha: hay una ventana (microsegundos) en la que otro hilo podría ejecutar la función a medio parchear. El upstream hace lo mismo para los hooks "calientes" pero suspende los otros hilos. | bajo con un solo hook; hay que resolverlo para los hooks de entidad |
| 2 | Solo hay **un** hook de prueba (contar frames). El pipeline 3D real (cámara, entidades, items, cables) está pendiente. | — |
| 3 | Los stubs `_Unwind_*` abortan si se llamaran. Con `panic = "abort"` no deberían llamarse nunca. Si el motor dejara de llevar el desenrollador estático y empezara a cargar `libgcc_s.so.1`, habría que quitar los stubs. | bajo |
| 4 | `panic = "abort"`: un panic en el inyector mata el juego. Por eso el código del hook está escrito sin panic posible (nada de `unwrap`). | — |
| 5 | La firma de cada hook hay que declararla a mano (qué va en `x0..x7`, qué vuelve y dónde). No hay des-mangling de tipos suficiente para deducirlo. | medio: hay que hacerlo hook a hook |
| 6 | El des-mangler es un subconjunto del ABI de Itanium: las plantillas más exóticas pueden no des-manglearse bien. El camino `Owner::metodo` sí es exacto. | bajo |
| 7 | El `.so` de glibc no vale para el visor Android (bionic): ahí hay que usar el target `aarch64-linux-android`, que ya compila ✓. | — |
| 8 | El trampolín tiene que quedar a menos de 4 GB del código enganchado (por el `adrp`). Con la pista de `mmap` se consigue en la práctica, pero no está garantizado. | bajo |
| 9 | La tabla de direcciones es **por binario**: cliente y headless no coinciden (§4). | — |

### Herramientas de análisis

Para este trabajo se ha usado `nm`, `llvm-objdump` (el del NDK) y scripts en
Python. **REA no ha podido usarse en esta máquina**: su proveedor Hopper no
arranca (macOS no autoriza los *Apple events* — hay que conceder el permiso de
Automatización en Ajustes del Sistema → Privacidad), Ghidra no está instalado
y `inspect_binary_layout` solo soporta el perfil verificado de Linux x64.
Conceder el permiso de Automización para Hopper habilitaría el análisis con
decompilador, que es lo que faltaría para portar los hooks complejos (firma
de los parámetros, qué hace exactamente cada `draw`).

---

## 9. Ficheros

| Fichero | Qué es |
|---|---|
| `injector-linux/Cargo.toml` | crate, sin dependencias, perfil release |
| `injector-linux/build.rs` | define el cfg `elf_target` (Linux/Android) |
| `injector-linux/.cargo/config.toml` | enlazado cruzado a glibc ARM64 |
| `injector-linux/src/lib.rs` | punto de entrada + constructor `.init_array` |
| `injector-linux/src/demangle.rs` | des-mangler del ABI de Itanium |
| `injector-linux/src/elf.rs` | lector de `.symtab` del ELF |
| `injector-linux/src/resolver.rs` | `dl_iterate_phdr` + `/proc/self/exe` + búsqueda |
| `injector-linux/src/detour.rs` | parche ARM64, reubicación PC-relativa, trampolín, `mprotect` |
| `injector-linux/src/hooks.rs` | hook de prueba y contador de frames |
| `injector-linux/src/maps.rs` | lectura de `/proc/self/maps` |
| `injector-linux/src/log.rs` | logging por `write(2)` |
| `injector-linux/src/gnu_unwind.rs` | stubs locales del desenrollador |
| `injector-linux/examples/parse_elf.rs` | herramienta de diagnóstico |
| `scripts/resolve_hooks.py` | tabla de hooks de 2.1.22 (verificada) |
| `scripts/test_injector.sh` | validación en el contenedor podman |
