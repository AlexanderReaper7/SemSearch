// The aurora shown while a search runs, from icon/working.html (the user,
// 2026-10-08): three curtains hanging from the top, each with a wavy, sharp
// lower edge, rays that bend with the folds, and light that fades upward from
// the edge. It draws opaque, black where there is no light, and the canvas
// screens over the view, so black leaves the background as it is.

// oklab, so hues blend without going muddy between them.
const toLin = (c) => ((c /= 255) <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const toSrgb = (c) => 255 * (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);
export function hexToOklab(hex) {
  const [r, g, b] = [1, 3, 5].map((k) => toLin(parseInt(hex.slice(k, k + 2), 16)));
  const l = Math.cbrt(0.4122214708 * r + 0.5363485062 * g + 0.051430043 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}
function oklabToRgb([L, A, B]) {
  const l = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s = (L - 0.0894841775 * A - 1.291485548 * B) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ].map((c) => Math.max(0, Math.min(255, toSrgb(Math.max(0, c)))));
}

// The palette as a position p, one turn per 1.0, to a color: the whole hue
// circle, as Gemini does. The spinner's comet and lit cell take theirs here.
export const rainbow = (p) => {
  const h = 2 * Math.PI * p;
  return oklabToRgb([0.66, 0.15 * Math.cos(h), 0.15 * Math.sin(h)]);
};

// working.html's defaults: one turn of the hues in `cycle` seconds, `spread`
// of a turn across the width.
export const SETTINGS = { cycle: 14, spread: 0.22, strength: 0.75 };

const FRAG = `#version 300 es
precision highp float;
uniform vec2 res;
uniform float t, cycle, spread, strength;
out vec4 o;
const float TAU = 6.2831853;

vec3 oklabToLin(vec3 c) {
  float l = c.x + .3963377774 * c.y + .2158037573 * c.z;
  float m = c.x - .1055613458 * c.y - .0638541728 * c.z;
  float s = c.x - .0894841775 * c.y - 1.2914855480 * c.z;
  l = l * l * l; m = m * m * m; s = s * s * s;
  return max(vec3(0), vec3(4.0767416621 * l - 3.3077115913 * m + .2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - .3413193965 * s,
    -.0041960863 * l - .7034186147 * m + 1.7076147010 * s));
}
vec3 toSrgb(vec3 c) { return mix(12.92 * c, 1.055 * pow(c, vec3(1. / 2.4)) - .055, step(.0031308, c)); }
vec3 palette(float p) { float h = TAU * p; return oklabToLin(vec3(.66, .15 * cos(h), .15 * sin(h))); }
float hash(vec2 p) { p = fract(p * vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x * p.y); }
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p); f = f * f * (3. - 2. * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
float fbm(vec2 p) { float a = .5, s = 0.; for (int k = 0; k < 4; k++) { s += a * noise(p); p *= 2.03; a *= .5; } return s / .9375; }

vec3 aurora(float u, float v) {
  float X = u * res.x / res.y;   // across, in units of the glow's height
  vec3 acc = vec3(0);
  for (int k = 0; k < 3; k++) {
    float fk = float(k);
    float base = .28 + .12 * fk, bright = 1. - .25 * fk;
    float sx = 1. + .35 * fk;    // nearer curtains have wider folds
    // Small folds over a gentle sway: big smooth humps read as hills.
    float fold = .045 * sin(X * 1.6 * sx + t * (.17 + .05 * fk) + fk * 2.)
               + .03 * sin(X * 3.9 * sx - t * (.11 + .03 * fk) + fk * 5.)
               + .09 * (fbm(vec2(X * 1.4 * sx + fk * 9., t * .08)) - .5);
    float edge = base + fold;
    float dy = edge - v;         // above the edge is positive
    // Rays follow the folds, so they slant where the curtain bends.
    float rx = X * sx + fold * 6.;
    float r = fbm(vec2(rx * 9., t * .12 + fk * 4.));
    r = smoothstep(.25, .85, r);
    float reach = .05 + .3 * r;  // how far a ray climbs above the edge
    // The lower edge is sharp on the front curtain and softer behind it.
    float I = dy < 0. ? exp(-pow(dy / (.012 * (1. + 1.5 * fk)), 2.)) : exp(-dy / reach) * (.5 + .5 * exp(-dy / .03));
    I *= .2 + .8 * r;
    // The hue turns with time, runs across the width, and shifts going up.
    float p = t / cycle + u * spread + fk * .09 + max(dy, 0.) * .5;
    acc += palette(p) * I * bright;
  }
  // A faint wash of the same colors at the very top.
  acc += palette(t / cycle + u * spread + .25) * .04 * exp(-v * 4.);
  acc *= strength * .9;
  return toSrgb(1. - exp(-acc));  // soft shoulder instead of clipping
}

void main() {
  float u = gl_FragCoord.x / res.x, v = 1. - gl_FragCoord.y / res.y;
  vec3 c = aurora(u, v);
  // Triangular dither of ±1 code, fresh every frame, so the 8-bit output has
  // no bands.
  vec2 q = gl_FragCoord.xy + fract(t * 7.31) * 113.;
  c += (hash(q) + hash(q.yx * 1.7 + 3.1) - 1.) / 255.;
  o = vec4(c, 1);
}`;
const VERT = `#version 300 es
void main() { gl_Position = vec4(vec2(gl_VertexID & 1, gl_VertexID >> 1) * 4. - 1., 0, 1); }`;

// A WebGL2 aurora on `canvas`, or an Error saying why there is none.
export function aurora(canvas, settings = SETTINGS) {
  const gl = canvas.getContext('webgl2', { alpha: false, antialias: false, powerPreference: 'low-power' });
  if (!gl) return new Error('no WebGL2 context');
  const shader = (type, src) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    return s;
  };
  const prog = gl.createProgram();
  gl.attachShader(prog, shader(gl.VERTEX_SHADER, VERT));
  gl.attachShader(prog, shader(gl.FRAGMENT_SHADER, FRAG));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return new Error(gl.getProgramInfoLog(prog));
  gl.useProgram(prog);
  const loc = Object.fromEntries(['res', 't', 'cycle', 'spread', 'strength'].map((n) => [n, gl.getUniformLocation(prog, n)]));

  return function draw(t) {
    const w = Math.round(canvas.clientWidth * devicePixelRatio);
    const h = Math.round(canvas.clientHeight * devicePixelRatio);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    gl.viewport(0, 0, w, h);
    gl.uniform2f(loc.res, w, h);
    gl.uniform1f(loc.t, t);
    gl.uniform1f(loc.cycle, settings.cycle);
    gl.uniform1f(loc.spread, settings.spread);
    gl.uniform1f(loc.strength, settings.strength);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  };
}
