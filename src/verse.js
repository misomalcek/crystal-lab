/**
 * Crystal verse — vanilla three.js port of the hive HarmonikaVerse basics:
 * type-shaped gems, colored edges, additive membranes between neighbors,
 * orbit / 6DOF flight, and the mpcmcp arcade. No eumorphia, no bloom stack.
 */
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

export const KIND_COLOR = {
  markdown: '#06b6d4',
  code: '#7c3aed',
  pdf: '#f59e0b',
  html: '#10b981',
  text: '#64748b',
  image: '#ef4444',
  office: '#475569',
  chunk: '#22d3ee',
  person: '#f472b6',
  concept: '#a78bfa',
  technology: '#34d399',
  organization: '#fb923c',
  work: '#38bdf8',
  place: '#facc15',
};

const ENTITY_KINDS = new Set(['person', 'concept', 'technology', 'organization', 'work', 'place']);

function geomFor(kind) {
  switch (kind) {
    case 'code':
      return new THREE.OctahedronGeometry(1, 0);
    case 'pdf':
      return new THREE.TetrahedronGeometry(1, 0);
    case 'html':
      return new THREE.DodecahedronGeometry(1, 0);
    case 'image':
      return new THREE.BoxGeometry(1.1, 1.1, 1.1);
    case 'office':
      return new THREE.ConeGeometry(0.7, 1.4, 5);
    case 'text':
      return new THREE.SphereGeometry(0.85, 12, 10);
    case 'chunk':
      return new THREE.IcosahedronGeometry(1, 1);
    case 'person':
      return new THREE.OctahedronGeometry(1, 0);
    case 'concept':
      return new THREE.IcosahedronGeometry(1, 1);
    case 'technology':
      return new THREE.BoxGeometry(1.1, 1.1, 1.1);
    case 'organization':
      return new THREE.DodecahedronGeometry(1, 0);
    case 'work':
      return new THREE.TetrahedronGeometry(1, 0);
    case 'place':
      return new THREE.ConeGeometry(0.7, 1.4, 5);
    default:
      return new THREE.IcosahedronGeometry(1, 1);
  }
}

function isEntityNode(n) {
  return ENTITY_KINDS.has(n.kind);
}

function isFileNode(n) {
  return n.kind !== 'chunk' && !isEntityNode(n);
}

function edgeHex(e, a) {
  if (e.rel === 'sibling') return '#d4a84b';
  if (e.rel === 'contains') return '#22d3ee';
  if (e.rel === 'next') return '#67e8f9';
  if (e.rel === 'relates_to') return '#a78bfa';
  if (e.rel === 'mentions') return '#f472b6';
  if (e.rel === 'same_as') return '#f8fafc';
  if (e.rel === 'conflicts_with') return '#fbbf24';
  return KIND_COLOR[a.kind] || '#8a938c';
}

/**
 * Files on an outer shell; chunks orbit their parent.
 * Entities relax with degree-scaled repulsion (LinLog): `hubSpread` is the
 * same knob as Factorium's "hub spread (LinLog)". Floor is 1 — at 0 the
 * degree term vanishes and hubs fall back into one ball.
 */
function layout3d(nodes, edges, hubSpread) {
  const files = nodes.filter(isFileNode);
  const chunks = nodes.filter((n) => n.kind === 'chunk');
  const entities = nodes.filter(isEntityNode);
  const golden = Math.PI * (1 + Math.sqrt(5));
  const place = (list, R) => {
    const n = Math.max(list.length, 1);
    list.forEach((node, i) => {
      const phi = Math.acos(1 - (2 * (i + 0.5)) / n);
      const theta = golden * i;
      node.x = R * Math.sin(phi) * Math.cos(theta);
      node.y = R * Math.cos(phi);
      node.z = R * Math.sin(phi) * Math.sin(theta);
    });
  };
  const nf = Math.max(files.length, 1);
  place(files, 90 + Math.min(nf, 40) * 4);
  const k = Math.min(15, Math.max(1, Number(hubSpread) || 1));
  const degree = new Map();
  const entIds = new Set(entities.map((n) => n.id));
  for (const e of edges || []) {
    if (entIds.has(e.from)) degree.set(e.from, (degree.get(e.from) || 0) + 1);
    if (entIds.has(e.to)) degree.set(e.to, (degree.get(e.to) || 0) + 1);
  }
  const ne = Math.max(entities.length, 1);
  entities.forEach((node, i) => {
    const deg = degree.get(node.id) || 0;
    const R = Math.min(420, 32 + Math.log2(deg + 1) * k * 11);
    const phi = Math.acos(1 - (2 * (i + 0.5)) / ne);
    const theta = golden * i;
    node.x = R * Math.sin(phi) * Math.cos(theta);
    node.y = R * Math.cos(phi);
    node.z = R * Math.sin(phi) * Math.sin(theta);
  });
  if (entities.length > 1) {
    const index = new Map(entities.map((node, i) => [node.id, i]));
    const springs = [];
    for (const e of edges || []) {
      const a = index.get(e.from);
      const b = index.get(e.to);
      if (a === undefined || b === undefined) continue;
      springs.push([a, b]);
    }
    const st = entities.map((node) => ({
      x: node.x,
      y: node.y,
      z: node.z,
      vx: 0,
      vy: 0,
      vz: 0,
      deg: degree.get(node.id) || 0,
    }));
    const iters = Math.min(48, 16 + Math.ceil(entities.length / 8));
    const ideal = 26 + k * 7;
    for (let iter = 0; iter < iters; iter++) {
      const temp = 1 - iter / iters;
      for (let i = 0; i < st.length; i++) {
        for (let j = i + 1; j < st.length; j++) {
          const dx = st[i].x - st[j].x;
          const dy = st[i].y - st[j].y;
          const dz = st[i].z - st[j].z;
          const distSq = dx * dx + dy * dy + dz * dz + 1;
          const dist = Math.sqrt(distSq);
          const degBoost = 1 + (Math.log2(st[i].deg + 1) + Math.log2(st[j].deg + 1)) * k;
          const force = ((420 * temp) / distSq) * degBoost;
          const fx = (dx / dist) * force;
          const fy = (dy / dist) * force;
          const fz = (dz / dist) * force;
          st[i].vx += fx;
          st[i].vy += fy;
          st[i].vz += fz;
          st[j].vx -= fx;
          st[j].vy -= fy;
          st[j].vz -= fz;
        }
      }
      for (const [i, j] of springs) {
        const dx = st[j].x - st[i].x;
        const dy = st[j].y - st[i].y;
        const dz = st[j].z - st[i].z;
        const dist = Math.hypot(dx, dy, dz) + 0.1;
        const force = 0.035 * (dist - ideal) * temp;
        const fx = (dx / dist) * force;
        const fy = (dy / dist) * force;
        const fz = (dz / dist) * force;
        st[i].vx += fx;
        st[i].vy += fy;
        st[i].vz += fz;
        st[j].vx -= fx;
        st[j].vy -= fy;
        st[j].vz -= fz;
      }
      for (const s of st) {
        s.vx *= 0.82;
        s.vy *= 0.82;
        s.vz *= 0.82;
        const sp = Math.hypot(s.vx, s.vy, s.vz);
        if (sp > 22) {
          const sc = 22 / sp;
          s.vx *= sc;
          s.vy *= sc;
          s.vz *= sc;
        }
        s.x += s.vx;
        s.y += s.vy;
        s.z += s.vz;
        const r = Math.hypot(s.x, s.y, s.z);
        if (r > 480) {
          const sc = 480 / r;
          s.x *= sc;
          s.y *= sc;
          s.z *= sc;
        }
      }
    }
    entities.forEach((node, i) => {
      node.x = st[i].x;
      node.y = st[i].y;
      node.z = st[i].z;
    });
  }
  const parent = new Map(files.map((f) => [f.id, f]));
  const byFile = new Map();
  for (const c of chunks) {
    const rel = c.id.replace(/#\d+$/, '');
    if (!byFile.has(rel)) byFile.set(rel, []);
    byFile.get(rel).push(c);
  }
  for (const [rel, list] of byFile) {
    const hub = parent.get(rel);
    if (!hub) continue;
    const n = list.length;
    const base = 16 + Math.min(n, 40) * 0.5;
    const r = Math.max(13, base * (k / 4));
    list.forEach((node, i) => {
      const phi = Math.acos(1 - (2 * (i + 0.5)) / Math.max(n, 1));
      const theta = golden * i;
      node.x = hub.x + r * Math.sin(phi) * Math.cos(theta);
      node.y = hub.y + r * Math.cos(phi);
      node.z = hub.z + r * Math.sin(phi) * Math.sin(theta);
    });
  }
}

function makeLabel(text) {
  const c = document.createElement('canvas');
  c.width = 512;
  c.height = 64;
  const ctx = c.getContext('2d');
  ctx.clearRect(0, 0, 512, 64);
  ctx.fillStyle = 'rgba(12,15,13,0.78)';
  ctx.fillRect(0, 10, 512, 44);
  ctx.fillStyle = '#e8eee6';
  ctx.font = '600 28px ui-sans-serif, system-ui, sans-serif';
  ctx.fillText(text.replace(/\.[^.]+$/, '').slice(0, 36), 12, 42);
  const tex = new THREE.CanvasTexture(c);
  const spr = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, transparent: true, depthWrite: false }));
  spr.scale.set(22, 2.8, 1);
  spr.position.y = 10;
  return spr;
}

const ANGELS = [
  { label: 'Primacy of Truth', weight: 60 },
  { label: 'stop-and-ask', weight: 50 },
  { label: 'Non ducor duco', weight: 50 },
  { label: 'verify-before-done', weight: 45 },
];
const DAEMONS = [
  { label: 'anger', weight: 50 },
  { label: 'greed', weight: 50 },
  { label: 'MCP bloat', weight: 30 },
  { label: 'skip verification', weight: 35 },
  { label: 'flattery', weight: 35 },
];

export function createVerse(canvas, { onSelect }) {
  const scene = new THREE.Scene();
  scene.background = new THREE.Color(0x0c0f0d);
  const camera = new THREE.PerspectiveCamera(55, 1, 0.1, 4000);
  camera.position.set(0, 28, 110);
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2));

  const orbit = new OrbitControls(camera, canvas);
  orbit.enableDamping = true;
  orbit.dampingFactor = 0.06;

  scene.add(new THREE.AmbientLight(0xe8eee6, 0.35));
  const key = new THREE.PointLight(0xd4a84b, 1.4, 0, 0);
  key.position.set(40, 60, 50);
  scene.add(key);

  const starGeo = new THREE.BufferGeometry();
  const starPos = new Float32Array(800 * 3);
  for (let i = 0; i < starPos.length; i++) starPos[i] = (Math.random() - 0.5) * 900;
  starGeo.setAttribute('position', new THREE.BufferAttribute(starPos, 3));
  scene.add(new THREE.Points(starGeo, new THREE.PointsMaterial({ color: 0x8a938c, size: 0.7 })));

  const root = new THREE.Group();
  scene.add(root);

  const raycaster = new THREE.Raycaster();
  const pointer = new THREE.Vector2();
  let nodeMeshes = [];
  let graph = { nodes: [], edges: [] };
  let hubSpread = 4;
  let glow = 0.45;
  let tuneTimer = 0;
  let highlight = new Set();
  let focus = null;
  let flight = false;
  let mpc = false;
  let score = 0;
  const velocity = new THREE.Vector3();
  const keys = {};
  let yaw = 0;
  let pitch = 0;
  let dragging = false;
  let lastPtr = { x: 0, y: 0 };
  let buf = '';
  const crystals = [];
  const bolts = [];
  let lastSpawn = 0;
  let clock = 0;

  function resize() {
    const r = canvas.parentElement.getBoundingClientRect();
    const w = Math.max(1, r.width);
    const h = Math.max(1, r.height);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
    renderer.setSize(w, h, false);
  }

  function meshForNode(node) {
    const file = isFileNode(node);
    const geo = geomFor(node.kind);
    const color = new THREE.Color(KIND_COLOR[node.kind] || '#8a938c');
    const group = new THREE.Group();
    group.position.set(node.x, node.y, node.z);
    const mat = new THREE.MeshPhysicalMaterial({
      color,
      emissive: color,
      emissiveIntensity: file ? 0.7 : 0.5,
      roughness: 0.22,
      metalness: 0.18,
      transparent: true,
      opacity: 0.86,
    });
    const mesh = new THREE.Mesh(geo, mat);
    const scale = isEntityNode(node) ? 5.2 : file ? 8 : 2.4;
    mesh.scale.setScalar(scale);
    mesh.userData.id = node.id;
    group.userData.id = node.id;
    group.userData.file = file;
    group.userData.baseScale = scale;
    group.add(mesh);
    if (file || isEntityNode(node)) {
      const wire = new THREE.Mesh(
        geo.clone(),
        new THREE.MeshBasicMaterial({ color: 0xfbbf24, wireframe: true, transparent: true, opacity: 0.45 }),
      );
      wire.scale.setScalar(scale * 1.06);
      wire.raycast = () => {};
      group.add(wire);
      const spr = makeLabel(node.label || node.id);
      spr.raycast = () => {};
      group.add(spr);
    }
    const ring = new THREE.Mesh(
      new THREE.RingGeometry(1.2, 1.38, 40),
      new THREE.MeshBasicMaterial({
        color: 0xffffff,
        transparent: true,
        opacity: 0.7,
        side: THREE.DoubleSide,
        depthWrite: false,
      }),
    );
    ring.rotation.x = Math.PI / 2;
    ring.scale.setScalar(scale);
    ring.visible = false;
    ring.raycast = () => {};
    group.userData.ring = ring;
    group.add(ring);
    return group;
  }

  function fitCamera() {
    if (!root.children.length) return;
    const box = new THREE.Box3().setFromObject(root);
    const size = Math.max(box.getSize(new THREE.Vector3()).length(), 40);
    const center = box.getCenter(new THREE.Vector3());
    orbit.target.copy(center);
    camera.position.copy(center).add(new THREE.Vector3(0, size * 0.28, size * 0.72));
    camera.near = Math.max(0.1, size / 400);
    camera.far = size * 20;
    camera.updateProjectionMatrix();
    orbit.update();
  }

  function applyHighlight() {
    const dim = highlight.size > 0;
    for (const g of nodeMeshes) {
      const hot = highlight.has(g.userData.id) || g.userData.id === focus;
      const s = g.userData.baseScale * (hot ? 1.45 : 1);
      const mesh = g.children[0];
      if (mesh) {
        mesh.scale.setScalar(s);
        if (mesh.material) {
          mesh.material.emissiveIntensity = hot ? 1.15 : dim ? 0.18 : g.userData.file ? 0.7 : 0.5;
          mesh.material.opacity = hot ? 0.95 : dim ? 0.42 : 0.86;
        }
      }
      if (g.userData.ring) {
        g.userData.ring.visible = g.userData.id === focus;
        g.userData.ring.scale.setScalar(s);
      }
    }
  }

  function rebuild(refit = true) {
    while (root.children.length) {
      const ch = root.children[0];
      root.remove(ch);
      ch.traverse((o) => {
        if (o.geometry) o.geometry.dispose();
        if (o.material) {
          if (o.material.map) o.material.map.dispose();
          o.material.dispose();
        }
      });
    }
    nodeMeshes = [];
    const nodes = graph.nodes || [];
    const edges = graph.edges || [];
    if (!nodes.length) return;
    layout3d(nodes, edges, hubSpread);
    const byId = new Map(nodes.map((n) => [n.id, n]));

    const cores = [];
    const halos = [];
    const col = new THREE.Color();
    const paint = (geo, hex, alpha) => {
      col.set(hex);
      const n = geo.attributes.position.count;
      const colors = new Float32Array(n * 3);
      const alphas = new Float32Array(n);
      for (let v = 0; v < n; v++) {
        colors[v * 3] = col.r;
        colors[v * 3 + 1] = col.g;
        colors[v * 3 + 2] = col.b;
        alphas[v] = alpha;
      }
      geo.setAttribute('color', new THREE.BufferAttribute(colors, 3));
      geo.setAttribute('aAlpha', new THREE.BufferAttribute(alphas, 1));
      return geo;
    };

    for (const e of edges) {
      const a = byId.get(e.from);
      const b = byId.get(e.to);
      if (!a || !b) continue;
      const curve = new THREE.LineCurve3(
        new THREE.Vector3(a.x, a.y, a.z),
        new THREE.Vector3(b.x, b.y, b.z),
      );
      const fileEdge = isFileNode(a) && isFileNode(b);
      const w = Math.max(1, e.weight || 1);
      const radius = fileEdge ? 0.55 : e.rel === 'contains' ? 0.28 : 0.12 + Math.log(w + 1) * 0.16;
      const hex = edgeHex(e, a);
      const coreAlpha = fileEdge ? 0.5 : 0.2 + 0.18 * glow;
      cores.push(paint(new THREE.TubeGeometry(curve, 12, radius, 5, false), hex, coreAlpha));
      const haloScale = 0.14 * glow;
      for (let li = 1; li <= 3; li++) {
        const t = li / 3;
        halos.push(
          paint(
            new THREE.TubeGeometry(curve, 8, radius * (1 + t * 1.4), 4, false),
            hex,
            haloScale * Math.exp(-1.6 * t * t),
          ),
        );
      }
    }
    const rayMat = new THREE.ShaderMaterial({
      vertexShader: `
        attribute float aAlpha; varying vec3 vColor; varying float vAlpha;
        void main() { vColor = color; vAlpha = aAlpha;
          gl_Position = projectionMatrix * modelViewMatrix * vec4(position,1.0); }`,
      fragmentShader: `
        varying vec3 vColor; varying float vAlpha;
        void main() { gl_FragColor = vec4(vColor * vAlpha, vAlpha); }`,
      vertexColors: true,
      transparent: true,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    });
    if (cores.length) {
      const core = mergeGeometries(cores, false);
      const halo = mergeGeometries(halos, false);
      cores.forEach((g) => g.dispose());
      halos.forEach((g) => g.dispose());
      if (core) root.add(new THREE.Mesh(core, rayMat));
      if (halo) root.add(new THREE.Mesh(halo, rayMat.clone()));
    }

    const adj = new Map();
    for (const e of edges) {
      if (!adj.has(e.from)) adj.set(e.from, []);
      if (!adj.has(e.to)) adj.set(e.to, []);
      adj.get(e.from).push(e.to);
      adj.get(e.to).push(e.from);
    }
    const tpos = [];
    const tcol = [];
    for (const [id, nbrs] of adj) {
      const node = byId.get(id);
      if (!node || nbrs.length < 2) continue;
      const limited = nbrs.slice(0, 6);
      for (let i = 0; i < limited.length; i++) {
        const a = byId.get(limited[i]);
        const b = byId.get(limited[(i + 1) % limited.length]);
        if (!a || !b) continue;
        tpos.push(node.x, node.y, node.z, a.x, a.y, a.z, b.x, b.y, b.z);
        const c = new THREE.Color(KIND_COLOR[node.kind] || '#8a938c');
        for (let v = 0; v < 3; v++) tcol.push(c.r * 0.07, c.g * 0.07, c.b * 0.07);
      }
    }
    if (tpos.length) {
      const geo = new THREE.BufferGeometry();
      geo.setAttribute('position', new THREE.Float32BufferAttribute(tpos, 3));
      geo.setAttribute('color', new THREE.Float32BufferAttribute(tcol, 3));
      root.add(
        new THREE.Mesh(
          geo,
          new THREE.MeshBasicMaterial({
            vertexColors: true,
            transparent: true,
            opacity: 0.18,
            blending: THREE.AdditiveBlending,
            depthWrite: false,
            side: THREE.DoubleSide,
          }),
        ),
      );
    }

    for (const n of nodes) {
      const g = meshForNode(n);
      root.add(g);
      nodeMeshes.push(g);
    }
    applyHighlight();
    if (refit) fitCamera();
  }

  function pick(ev) {
    const r = canvas.getBoundingClientRect();
    pointer.x = ((ev.clientX - r.left) / r.width) * 2 - 1;
    pointer.y = -((ev.clientY - r.top) / r.height) * 2 + 1;
    raycaster.setFromCamera(pointer, camera);
    const hit = raycaster.intersectObjects(nodeMeshes, true)[0];
    if (!hit) return;
    let obj = hit.object;
    while (obj && !obj.userData.id) obj = obj.parent;
    if (obj?.userData.id) onSelect(obj.userData.id);
  }

  function isTyping() {
    const el = document.activeElement;
    return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
  }

  function spawnCrystal() {
    const isDaemon = Math.random() < 0.65;
    const data = isDaemon
      ? DAEMONS[Math.floor(Math.random() * DAEMONS.length)]
      : ANGELS[Math.floor(Math.random() * ANGELS.length)];
    const dir = new THREE.Vector3(Math.random() - 0.5, Math.random() - 0.5, Math.random() - 0.5)
      .normalize();
    const pos = camera.position.clone().addScaledVector(dir, 18 + Math.random() * 22);
    const geo = isDaemon ? new THREE.IcosahedronGeometry(0.9, 0) : new THREE.OctahedronGeometry(0.9, 0);
    const color = isDaemon ? 0x9b3b2f : 0xfbbf24;
    const mesh = new THREE.Mesh(
      geo,
      new THREE.MeshPhysicalMaterial({
        color,
        emissive: color,
        emissiveIntensity: 0.7,
        roughness: 0.3,
      }),
    );
    mesh.position.copy(pos);
    scene.add(mesh);
    crystals.push({
      mesh,
      kind: isDaemon ? 'daemon' : 'angel',
      data,
      vel: new THREE.Vector3((Math.random() - 0.5) * 6, (Math.random() - 0.5) * 6, (Math.random() - 0.5) * 6),
    });
  }

  function fire() {
    const dir = new THREE.Vector3(0, 0, -1).applyQuaternion(camera.quaternion);
    const mesh = new THREE.Mesh(
      new THREE.SphereGeometry(0.25, 8, 8),
      new THREE.MeshBasicMaterial({ color: 0x22d3ee }),
    );
    mesh.position.copy(camera.position).addScaledVector(dir, 4);
    scene.add(mesh);
    bolts.push({ mesh, vel: dir.multiplyScalar(90) });
  }

  function onKeyDown(e) {
    if (isTyping()) return;
    keys[e.code] = true;
    if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Space', 'KeyB'].includes(e.code)) {
      e.preventDefault();
    }
    if (flight && !mpc && e.key.length === 1) {
      buf = (buf + e.key.toLowerCase()).slice(-6);
      if (buf === 'mpcmcp') {
        mpc = true;
        score = 0;
        buf = '';
        onHud();
      }
    }
    if (mpc && e.code === 'Enter') {
      e.preventDefault();
      fire();
    }
    if (mpc && e.key === 'Escape') {
      mpc = false;
      crystals.splice(0).forEach((c) => scene.remove(c.mesh));
      bolts.splice(0).forEach((b) => scene.remove(b.mesh));
      onHud();
    }
  }
  function onKeyUp(e) {
    keys[e.code] = false;
  }

  let hud = null;
  function onHud() {
    if (!hud) return;
    hud.style.display = mpc ? 'block' : 'none';
    hud.querySelector('[data-score]').textContent = String(score);
  }

  function tick(dt) {
    clock += dt;
    for (const g of nodeMeshes) {
      const spin = dt * (g.userData.id === focus ? 0.55 : 0.12);
      for (const ch of g.children) {
        if (ch.isSprite || ch === g.userData.ring) continue;
        ch.rotation.y += spin;
      }
    }
    orbit.enabled = !flight;
    if (flight) {
      const euler = new THREE.Euler(pitch, yaw, 0, 'YXZ');
      camera.quaternion.setFromEuler(euler);
      const accel = new THREE.Vector3();
      if (keys.ArrowUp) accel.z -= 1;
      if (keys.ArrowDown) accel.z += 1;
      if (keys.ArrowLeft) accel.x -= 1;
      if (keys.ArrowRight) accel.x += 1;
      if (accel.lengthSq() > 0) accel.normalize().multiplyScalar(900);
      if (keys.Space) accel.z -= 1700;
      if (accel.lengthSq() > 0) {
        accel.applyQuaternion(camera.quaternion).multiplyScalar(dt);
        velocity.add(accel);
      }
      if (keys.KeyB) velocity.multiplyScalar(0.86);
      velocity.multiplyScalar(1 - Math.min(1, 0.5 * dt));
      if (velocity.length() > 1500) velocity.setLength(1500);
      camera.position.addScaledVector(velocity, dt);
    } else {
      orbit.update();
    }

    if (mpc) {
      if (crystals.length < 8 && clock - lastSpawn > 0.45) {
        spawnCrystal();
        lastSpawn = clock;
      }
      for (const c of crystals) {
        c.mesh.position.addScaledVector(c.vel, dt);
        c.mesh.rotation.y += dt * 0.8;
      }
      for (const b of bolts) {
        b.mesh.position.addScaledVector(b.vel, dt);
      }
      for (let i = bolts.length - 1; i >= 0; i--) {
        const b = bolts[i];
        for (let j = crystals.length - 1; j >= 0; j--) {
          const c = crystals[j];
          if (b.mesh.position.distanceTo(c.mesh.position) < 2.2) {
            score += c.kind === 'daemon' ? c.data.weight : -c.data.weight;
            scene.remove(c.mesh);
            scene.remove(b.mesh);
            crystals.splice(j, 1);
            bolts.splice(i, 1);
            onHud();
            break;
          }
        }
      }
    }

    renderer.render(scene, camera);
  }

  let raf;
  function loop() {
    raf = requestAnimationFrame(loop);
    tick(1 / 60);
  }

  canvas.addEventListener('click', pick);
  canvas.addEventListener('pointerdown', (e) => {
    if (!flight) return;
    dragging = true;
    lastPtr = { x: e.clientX, y: e.clientY };
    if (mpc && e.button === 0) canvas.dataset.down = `${e.clientX},${e.clientY}`;
  });
  window.addEventListener('pointerup', (e) => {
    dragging = false;
    if (mpc && canvas.dataset.down) {
      const [x, y] = canvas.dataset.down.split(',').map(Number);
      if (Math.hypot(e.clientX - x, e.clientY - y) < 6) fire();
      delete canvas.dataset.down;
    }
  });
  window.addEventListener('pointermove', (e) => {
    if (!flight || !dragging) return;
    yaw -= (e.clientX - lastPtr.x) * 0.005;
    pitch -= (e.clientY - lastPtr.y) * 0.005;
    const lim = Math.PI / 2 - 0.05;
    pitch = Math.max(-lim, Math.min(lim, pitch));
    lastPtr = { x: e.clientX, y: e.clientY };
  });
  window.addEventListener('keydown', onKeyDown);
  window.addEventListener('keyup', onKeyUp);

  resize();
  loop();

  return {
    setGraph(g) {
      graph = g;
      rebuild(true);
    },
    setTune(next) {
      if (next.hubSpread != null) hubSpread = Math.min(15, Math.max(1, Number(next.hubSpread) || 1));
      if (next.glow != null) glow = Math.min(1.5, Math.max(0, Number(next.glow) || 0));
      clearTimeout(tuneTimer);
      tuneTimer = setTimeout(() => rebuild(false), 40);
    },
    setHighlight(h) {
      highlight = h;
      applyHighlight();
    },
    setFocus(id) {
      focus = id;
      applyHighlight();
    },
    setFlight(on) {
      flight = on;
      if (on) {
        const euler = new THREE.Euler().setFromQuaternion(camera.quaternion, 'YXZ');
        yaw = euler.y;
        pitch = euler.x;
        velocity.set(0, 0, 0);
      } else {
        mpc = false;
        crystals.splice(0).forEach((c) => scene.remove(c.mesh));
        bolts.splice(0).forEach((b) => scene.remove(b.mesh));
        onHud();
      }
    },
    attachHud(el) {
      hud = el;
    },
    isFlight: () => flight,
    isMpc: () => mpc,
    resize,
  };
}
