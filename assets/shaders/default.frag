#version 330 core
uniform float u_time;
uniform vec2  u_resolution;

uniform float u_gain;
uniform float u_zoom;
uniform float u_spin;

out vec4 o_color;

mat2 rot(float a) {
    float s = sin(a), c = cos(a);
    return mat2(c, -s, s, c);
}

void main() {
    vec2 res = max(u_resolution, vec2(1.0));
    vec2 uv = (gl_FragCoord.xy / res) * 2.0 - 1.0;

    uv *= (1.0 / max(u_zoom, 0.0001));
    uv = rot(u_spin) * uv;

    float t = u_time;
    float r = length(uv);

    float wave = 0.5 + 0.5 * sin(10.0 * r - t * 2.0);
    wave = mix(wave, pow(wave, 0.35), clamp(u_gain, 0.0, 1.0));

    vec3 col = vec3(
        0.5 + 0.5 * sin(t + uv.x * 2.0),
        wave,
        0.5 + 0.5 * sin(t + uv.y * 2.0)
    );

    o_color = vec4(col, 1.0);
}
