#version 320 es
// sky.vert — Quad de fondo para cielo VR o máscara de Passthrough
precision highp float;

out vec2 v_uv;

void main() {
    // Genera un quad a pantalla completa
    vec2 uv = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    v_uv = uv;
    gl_Position = vec4(uv * 2.0 - 1.0, 0.9999, 1.0);
}
