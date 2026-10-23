#version 320 es
// fullscreen.vert — Triangulo de pantalla completa (para el upscaling SGSR).
//
// El nombre del out es `in_TEXCOORD0` porque es lo que espera el shader
// sgsr1.frag de Qualcomm.
precision highp float;

layout(location = 0) out highp vec4 in_TEXCOORD0;

void main() {
    vec2 uv = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    in_TEXCOORD0 = vec4(uv, 0.0, 1.0);
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}
