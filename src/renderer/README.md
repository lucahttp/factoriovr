# renderer 3D del visor

Aqui se dibujan las maquinas de Factorio con su **modelo 3D real** (los GLB
del mod `factorio-3d-mod`), instanciadas: una matriz por entidad y una
llamada de dibujo por (prototipo, parte).

| fichero | que hace |
|---|---|
| `gl.rs` | punteros a las funciones de OpenGL ES 3.2 (`libGLESv2.so`, con `dlsym`: el NDK no trae la libreria de stubs) |
| `shader.rs` | compila los programas de GLSL (los lee de `assets/shaders/` y, si no estan, usa las copias embebidas) |
| `model.rs` | mallas en la GPU (VBO + EBO) y dibujo instanciado |
| `sgsr.rs` | upscaling SGSR (Qualcomm) en GLES 3.2 |

## Como se dibuja una entidad con su modelo

1. El puente de Factorio manda el prototipo de cada entidad viva y
   `src/modelos3d.rs` le asigna un `model_id` estable (`0` = sin modelo).
2. `recopilar_instancias()` agrupa las entidades vivas por prototipo y
   calcula una matriz por entidad:
   `T(casilla) * Ry(giro) * S(escala) * T(-offset)`.
3. La malla del GLB (vertices unicos + indices, con el color horneado) se
   sube a la GPU una sola vez y se dibuja con `glDrawElementsInstanced`.
4. Las entidades **sin** modelo 3D las sigue pintando el motor de VR con el
   sprite 2D del atlas: nunca queda un hueco.

## Lo que tiene que hacer el motor de VR (C++)

`android/src/openxr_engine.cpp` es el duenno del contexto GL, asi que el
cableado va por FFI (el motor las busca con `dlsym`, como las demas).

### Maquinas 3D

```c
// 1. Al arrancar, con el contexto GL ya corriente (una sola vez):
typedef bool (*pfn_xfactorvr_3d_iniciar)();
s_3d_iniciar = (pfn_xfactorvr_3d_iniciar)dlsym(handle, "xfactorvr_3d_iniciar");
s_3d_iniciar();                       // compila los shaders y carga libGLESv2

// 2. Por ojo y frame, ANTES de dibujar la escena:
//    vp    = vista-proyeccion (16 floats, column-major)
//    tabla = matriz de la mesa (16 floats, column-major): la MISMA que usan
//            los sprites (coloca la maqueta en la mesa fisica).
typedef void (*pfn_xfactorvr_3d_fijar_camaras)(const float* vp, const float* tabla);
typedef uint32_t (*pfn_xfactorvr_dibujar_modelos_3d)();
s_3d_fijar_camaras(vp_mat, s_table_model_mat);
uint32_t n = s_dibujar_modelos_3d();   // nº de instancias dibujadas

// 3. Para NO pintar el sprite 2D de una entidad que ya tiene modelo:
//    (el motor ya lo hace con `ent.model_id != 0`, asi que esto es solo
//     una comprobacion)
typedef bool (*pfn_xfactorvr_3d_tiene_modelo)(uint32_t model_id);
```

Variante con las matrices en la propia llamada (por si el motor prefiere
no guardar estado en Rust):

```c
uint32_t xfactorvr_dibujar_modelos_3d_ex(const float* vp, const float* tabla);
```

Y, si se quiere el especular bien puesto, la posicion de la camara en el
espacio de la HABITACION (3 floats):

```c
void xfactorvr_3d_fijar_ojos(const float* ojos);   // opcional
```

### SGSR (upscaling a 2160x2160)

```c
// Al arrancar, una vez por ojo (si se quiere SGSR):
typedef bool (*pfn_xfactorvr_sgsr_preparar)(int w_in, int h_in, int w_out, int h_out);
typedef uint32_t (*pfn_xfactorvr_sgsr_fbo)();
typedef bool (*pfn_xfactorvr_sgsr_escalar)();

s_sgsr_preparar(1080, 1080, 2160, 2160);   // entrada -> pantalla

// En el bucle de render del ojo:
glBindFramebuffer(GL_FRAMEBUFFER, s_sgsr_fbo());   // se dibuja a 1080x1080
glViewport(0, 0, 1080, 1080);
... se dibuja la escena ...
glBindFramebuffer(GL_FRAMEBUFFER, swapchain_fbo);   // vuelta a la pantalla
s_sgsr_escalar();                                    // reescala con SGSR
```

Se usa el **SGSR 1** (espacial, un pase): el SGSR 2 es GLSL de Vulkan
(`layout(set=..., binding=...)`) y necesita texturas de velocidad y
profundidad que aqui no hay. El shader se subio de `#version 300 es` a
`#version 320 es` porque usa `textureGather` (GLSL ES 3.1+).

## Nota importante: el camino plano

El motor de VR tiene ADEMAS un camino plano (`xfactorvr_modelo3d_geometria_viva`):
manda la geometria ya transformada y la pinta con el shader del diorama.
Ese camino y el de aqui dibujan LO MISMO: cuando se cablee
`xfactorvr_dibujar_modelos_3d()` hay que **quitar el bloque de
`s_modelo3d_viva`** (o dejar el plano solo como respaldo si el renderer 3D
no se pudo inicializar, que es lo que hace `xfactorvr_3d_iniciar()` al
devolver `false`).
