#version 320 es
// modelo_visor.vert — Vertex shader de las maquinas con su MODELO 3D real.
//
// Adaptado de shaders/model.vert (que venia de Generals-Zero-Hour-XR y usaba
// un Uniform Block std140 + esqueleto). Aqui:
//   * NO hay Uniform Block: los uniformes se ponen por nombre, que es lo
//     que sabe hacer el visor (OpenGL ES 3.2 del motor de VR).
//   * NO hay esqueleto (los GLB del mod factorio-3d-mod no vienen skinned).
//   * SI hay instancing: una matriz por entidad, en 4 atributos (4..7).
//
// Los vertices ya traen el color horneado (COLOR_0 del GLB optimizado), asi
// que no hace falta ninguna textura.
precision highp float;

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;
layout(location = 2) in vec2 in_uv;
layout(location = 3) in vec4 in_color;

// Matriz de la instancia (4 atributos = 4 columnas de la mat4).
layout(location = 4) in vec4 in_m0;
layout(location = 5) in vec4 in_m1;
layout(location = 6) in vec4 in_m2;
layout(location = 7) in vec4 in_m3;

// vista * tabla (la tabla ya coloca la maqueta en la mesa fisica).
uniform mat4 u_mvp;

out vec3 v_normal;
out vec2 v_uv;
out vec4 v_color;
out vec3 v_mundo;

void main() {
    mat4 inst = mat4(in_m0, in_m1, in_m2, in_m3);
    vec4 mundo = inst * vec4(in_position, 1.0);
    v_mundo = mundo.xyz;
    v_normal = normalize(mat3(inst) * in_normal);
    v_uv = in_uv;
    v_color = in_color;
    gl_Position = u_mvp * mundo;
}
