#version 330 core
uniform sampler2D u_tex;
uniform vec2 u_resolution;
out vec4 o_color;

void main() {
    vec2 uv = gl_FragCoord.xy / max(u_resolution, vec2(1.0));
    o_color = texture(u_tex, uv);
}
