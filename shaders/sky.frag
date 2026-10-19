#version 320 es
// sky.frag — Soporte para cielo estético diurno/nocturno o canal alfa 0.0 para Passthrough MR
precision highp float;

in vec2 v_uv;

layout(std140) uniform CameraCB {
    mat4 u_view_proj;
    float u_aspect;
    float u_plane_scale;
    float u_tex_w;
    float u_tex_h;
    vec4 u_sky; // x: night factor
};

// 1 = Modo Mixed Reality Passthrough (fondo transparente); 0 = Modo VR Cielo sintético
uniform int u_passthrough_enabled;

out vec4 frag_color;

void main() {
    if (u_passthrough_enabled == 1) {
        // En Realidad Mixta (Passthrough activo en Pico 4 Ultra), el fondo
        // debe tener canal Alpha = 0 para ver la habitación real a través de las cámaras.
        frag_color = vec4(0.0, 0.0, 0.0, 0.0);
        return;
    }

    // Gradiente de cielo para modo VR inmersivo puro:
    float night = clamp(u_sky.x, 0.0, 1.0);
    vec3 day_zenith = vec3(0.25, 0.50, 0.85);
    vec3 day_horizon = vec3(0.65, 0.75, 0.90);
    vec3 night_zenith = vec3(0.02, 0.03, 0.08);
    vec3 night_horizon = vec3(0.06, 0.08, 0.15);

    vec3 zenith = mix(day_zenith, night_zenith, night);
    vec3 horizon = mix(day_horizon, night_horizon, night);

    float t = clamp(v_uv.y, 0.0, 1.0);
    vec3 sky_color = mix(horizon, zenith, t);

    frag_color = vec4(sky_color, 1.0);
}
