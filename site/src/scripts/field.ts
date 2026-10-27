/*
  The field of light: tens of thousands of points, one per name on a drive, drawn in one WebGL2 call.
  Each point carries its place in all six scenes (scenes.ts); the GPU blends between them with weights
  that follow the scroll, so the drive becomes a list, the list a map, the map a ring, and so on. A point
  now and then catches the light and flashes into a four-pointed glint; the pointer stirs the field;
  the names that match the search fall into the ring and turn pink.

  Lazy-loaded after the page goes idle (main.ts); skipped without WebGL2 or motion.
*/
import { colors, layout, logoColors, sampleLogo, SCENES, seeds, type LogoSample } from "./scenes";

export interface Field {
  tick(time: number, scrollY: number, velocity: number): void;
  gather(): void;
  setPointer(x: number, y: number, on: boolean): void;
  setQuery(hash: number, frac: number, active: boolean): void;
  measure(): void;
}

const VERT = `#version 300 es
precision highp float;
in vec4 a_seed;
in vec3 a_color;
in vec3 a_logo;
in vec2 a_p0;
in vec2 a_p1;
in vec2 a_p2;
in vec2 a_p3;
in vec2 a_p4;
in vec2 a_p5;
uniform float u_w[6];
uniform float u_glow[6];
uniform float u_time;
uniform vec2 u_res;
uniform float u_dpr;
uniform vec2 u_pointer;
uniform float u_pointerOn;
uniform float u_gather;
uniform float u_query;
uniform float u_qfrac;
uniform float u_qon;
uniform vec2 u_center;
uniform float u_radius;
uniform vec2 u_ring;
uniform float u_speed;
out vec3 v_color;
out float v_alpha;
out float v_spark;

float hash(float n) { return fract(sin(n) * 43758.5453123); }

void main() {
  // The drive turns: inner names faster than outer ones, on its tilted disc.
  vec2 q = (a_p0 - u_center) / vec2(1.0, 0.58);
  float d = length(q) / u_radius;
  float ang = u_time * 0.035 / (0.35 + d);
  float c = cos(ang), s = sin(ang);
  vec2 p0 = u_center + vec2(c * q.x - s * q.y, s * q.x + c * q.y) * vec2(1.0, 0.58);

  vec2 p = p0 * u_w[0] + a_p1 * u_w[1] + a_p2 * u_w[2] + a_p3 * u_w[3] + a_p4 * u_w[4] + a_p5 * u_w[5];

  // Every point breathes a little.
  float ph = a_seed.z * 6.2831;
  p += vec2(sin(u_time * 0.35 + ph), cos(u_time * 0.29 + ph * 1.3)) * (1.2 + a_seed.y * 2.4);

  // The names that match the search fall into the ring, towards its centre.
  float sel = step(hash(a_seed.x * 917.0 + u_query * 131.0), u_qfrac) * u_qon * u_w[3];
  p = mix(p, u_ring + (a_p3 - u_ring) * 0.22, sel);

  // The pointer parts the field, and lights what it passes.
  vec2 dp = p - u_pointer;
  float dist = length(dp);
  float near = u_pointerOn * exp(-(dist * dist) / (2.0 * 110.0 * 110.0));
  p += (dist > 0.001 ? dp / dist : vec2(0.0)) * near * 22.0;

  // At first everything waits in the flash at the centre, then pours out.
  float t = smoothstep(a_seed.x * 0.55, a_seed.x * 0.55 + 0.45, u_gather);
  p = mix(u_res * vec2(0.5, 0.5), p, t);

  // Now and then a point catches the light.
  float tw = pow(max(0.0, sin(u_time * (0.5 + a_seed.y * 1.6) * u_speed + ph * 3.0)), 90.0);
  v_spark = tw * (0.5 + a_seed.w) + near * 1.4 + sel * 0.9;

  v_color = mix(a_color, a_logo, u_w[5]);
  v_color = mix(v_color, vec3(1.0, 0.56, 0.82), sel);
  // Scenes that sit behind a lot of reading are dimmer (GLOW in field.ts).
  float glow = u_w[0] * u_glow[0] + u_w[1] * u_glow[1] + u_w[2] * u_glow[2] + u_w[3] * u_glow[3] + u_w[4] * u_glow[4] + u_w[5] * u_glow[5];
  v_alpha = (0.18 + 0.82 * a_seed.w) * t * glow * (1.0 + sel * 0.8);

  gl_PointSize = (1.1 + a_seed.y * 2.6) * (1.0 + v_spark * 3.0) * u_dpr;
  vec2 clip = (p / u_res) * 2.0 - 1.0;
  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
}`;

const FRAG = `#version 300 es
precision mediump float;
in vec3 v_color;
in float v_alpha;
in float v_spark;
out vec4 o;
void main() {
  vec2 uv = gl_PointCoord * 2.0 - 1.0;
  float r = length(uv);
  float core = smoothstep(1.0, 0.0, r);
  core *= core;
  // A four-pointed glint on the points that flash.
  float rays = max(0.0, 1.0 - abs(uv.x) * 9.0) * max(0.0, 1.0 - abs(uv.y)) + max(0.0, 1.0 - abs(uv.y) * 9.0) * max(0.0, 1.0 - abs(uv.x));
  float a = (core + rays * clamp(v_spark, 0.0, 1.0)) * v_alpha;
  o = vec4(v_color * a, a);
}`;

function compile(gl: WebGL2RenderingContext, type: number, src: string) {
  const s = gl.createShader(type);
  if (!s) return null;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
    console.warn(gl.getShaderInfoLog(s));
    return null;
  }
  return s;
}

/** Each scene's brightness: the list, the map and the horizon sit behind cards and text. */
const GLOW = new Float32Array([1, 0.5, 0.55, 0.95, 0.42, 1]);
/** On a phone, copy covers more of every scene. */
const GLOW_NARROW = new Float32Array([0.72, 0.42, 0.45, 0.8, 0.38, 0.9]);

/** As many points as the screen deserves: fewer on small or modest devices. */
function pointCount(w: number, h: number) {
  const cores = navigator.hardwareConcurrency || 4;
  const n = Math.round((w * h) / 15) * (cores <= 4 ? 0.6 : 1);
  return Math.max(16000, Math.min(90000, n));
}

export function createField(canvas: HTMLCanvasElement): Field | null {
  const gl = canvas.getContext("webgl2", { alpha: true, antialias: false, premultipliedAlpha: true, powerPreference: "high-performance" });
  if (!gl) return null;
  const vs = compile(gl, gl.VERTEX_SHADER, VERT);
  const fs = compile(gl, gl.FRAGMENT_SHADER, FRAG);
  const prog = gl.createProgram();
  if (!vs || !fs || !prog) return null;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return null;
  gl.useProgram(prog);

  const dpr = Math.min(window.devicePixelRatio || 1, 1.75);
  let vw = canvas.clientWidth || window.innerWidth;
  let vh = canvas.clientHeight || window.innerHeight;
  const n = pointCount(vw, vh);
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);

  const attr = (name: string, size: number, data: Float32Array) => {
    const loc = gl.getAttribLocation(prog, name);
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
    if (loc >= 0) {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
    }
    return buf;
  };
  const own = colors(n);
  attr("a_seed", 4, seeds(n));
  attr("a_color", 3, own);
  const logoBuf = attr("a_logo", 3, own);
  let logo: LogoSample | null = null;
  let built = layout(n, vw, vh, logo);
  const posBufs = built.pos.map((p, i) => attr(`a_p${i}`, 2, p));

  const u = (name: string) => gl.getUniformLocation(prog, name);
  const U = {
    w: u("u_w"),
    glow: u("u_glow"),
    time: u("u_time"),
    res: u("u_res"),
    dpr: u("u_dpr"),
    pointer: u("u_pointer"),
    pointerOn: u("u_pointerOn"),
    gather: u("u_gather"),
    query: u("u_query"),
    qfrac: u("u_qfrac"),
    qon: u("u_qon"),
    center: u("u_center"),
    radius: u("u_radius"),
    ring: u("u_ring"),
    speed: u("u_speed"),
  };

  gl.enable(gl.BLEND);
  gl.blendFunc(gl.ONE, gl.ONE);
  gl.disable(gl.DEPTH_TEST);

  const upload = () => {
    built.pos.forEach((p, i) => {
      gl.bindBuffer(gl.ARRAY_BUFFER, posBufs[i]);
      gl.bufferData(gl.ARRAY_BUFFER, p, gl.STATIC_DRAW);
    });
  };

  const size = () => {
    vw = canvas.clientWidth || window.innerWidth;
    vh = canvas.clientHeight || window.innerHeight;
    canvas.width = Math.round(vw * dpr);
    canvas.height = Math.round(vh * dpr);
    gl.viewport(0, 0, canvas.width, canvas.height);
  };
  size();

  // The mark arrives a moment later: its pixels become scene 5, in its colours.
  void sampleLogo("/favicon.svg").then((sample) => {
    if (!sample) return;
    logo = sample;
    built = layout(n, vw, vh, logo);
    upload();
    gl.bindBuffer(gl.ARRAY_BUFFER, logoBuf);
    gl.bufferData(gl.ARRAY_BUFFER, logoColors(n, own, logo), gl.STATIC_DRAW);
  });

  // A resize lays the same points out again; a phone's toolbar doesn't (the canvas is 100lvh).
  let resizeTimer = 0;
  new ResizeObserver(() => {
    clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(() => {
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (Math.abs(w - vw) < 2 && Math.abs(h - vh) < 2) return;
      size();
      built = layout(n, vw, vh, logo);
      upload();
      measure();
    }, 160);
  }).observe(canvas);

  // Where each chapter is, in page coordinates, and which scene it shows.
  let chapters: { top: number; height: number; scene: number }[] = [];
  const measure = () => {
    chapters = [...document.querySelectorAll<HTMLElement>("[data-scene]")].map((el) => {
      const box = el.parentElement?.classList.contains("pin-spacer") ? el.parentElement : el;
      const r = box.getBoundingClientRect();
      return { top: r.top + window.scrollY, height: r.height, scene: Number(el.dataset.scene) };
    });
  };
  measure();

  const weights = new Float32Array(SCENES);
  weights[0] = 1;
  const target = new Float32Array(SCENES);
  const pointer = { x: -9999, y: -9999, on: 0, aim: 0 };
  const query = { hash: 0, frac: 0, on: 0, aim: 0 };
  let gather = 0;
  let gathering = false;
  let last = 0;
  let speed = 1;

  const smooth = (k: number, dt: number) => 1 - Math.exp(-dt * k);
  const step = (e0: number, e1: number, x: number) => {
    const t = Math.min(1, Math.max(0, (x - e0) / (e1 - e0)));
    return t * t * (3 - 2 * t);
  };

  return {
    tick(time, scrollY, velocity) {
      const dt = last ? Math.min(0.1, time - last) : 0.016;
      last = time;

      // The chapter at the middle of the screen sets the scene; its last stretch hands over to the next.
      target.fill(0);
      const mid = scrollY + window.innerHeight * 0.5;
      let i = chapters.findIndex((c) => mid >= c.top && mid < c.top + c.height);
      if (i < 0) i = mid < (chapters[0]?.top ?? 0) ? 0 : chapters.length - 1;
      const here = chapters[i];
      if (here) {
        const next = chapters[i + 1];
        const blend = next ? step(0.72, 1, (mid - here.top) / here.height) : 0;
        target[here.scene] += 1 - blend;
        if (next) target[next.scene] += blend;
      } else target[0] = 1;
      const k = smooth(4.2, dt);
      for (let s = 0; s < SCENES; s++) weights[s] += (target[s] - weights[s]) * k;

      if (gathering) gather = Math.min(1, gather + dt / 2.4);
      pointer.on += (pointer.aim - pointer.on) * smooth(5, dt);
      query.on += (query.aim - query.on) * smooth(4, dt);
      speed += (1 + Math.min(2.5, Math.abs(velocity) / 1600) - speed) * smooth(3, dt);

      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.uniform1fv(U.w, weights);
      gl.uniform1fv(U.glow, vw < 900 ? GLOW_NARROW : GLOW);
      gl.uniform1f(U.time, time);
      gl.uniform2f(U.res, vw, vh);
      gl.uniform1f(U.dpr, dpr);
      gl.uniform2f(U.pointer, pointer.x, pointer.y);
      gl.uniform1f(U.pointerOn, pointer.on);
      gl.uniform1f(U.gather, gather);
      gl.uniform1f(U.query, query.hash);
      gl.uniform1f(U.qfrac, query.frac);
      gl.uniform1f(U.qon, query.on);
      gl.uniform2f(U.center, built.center[0], built.center[1]);
      gl.uniform1f(U.radius, built.radius);
      gl.uniform2f(U.ring, built.ring[0], built.ring[1]);
      gl.uniform1f(U.speed, speed);
      gl.drawArrays(gl.POINTS, 0, n);

      if (!canvas.classList.contains("is-on")) {
        canvas.classList.add("is-on");
        document.documentElement.classList.add("field-live");
      }
    },
    gather() {
      gathering = true;
    },
    setPointer(x, y, on) {
      pointer.x = x;
      pointer.y = y;
      pointer.aim = on ? 1 : 0;
    },
    setQuery(hash, frac, active) {
      query.hash = hash;
      query.frac = frac;
      query.aim = active ? 1 : 0;
    },
    measure,
  };
}
