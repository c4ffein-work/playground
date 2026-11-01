// Illustrated stand-ins for field photos. Each one is a real file whose
// sha256 goes on-chain, so the site can re-hash it and show it's untouched.

const W = 640;
const H = 420;

function rng(seed) {
  let s = seed >>> 0;
  return () => ((s = (s * 1664525 + 1013904223) >>> 0) / 2 ** 32);
}

const tree = (x, y, k) =>
  `<g transform="translate(${x} ${y}) scale(${k})" fill="#3d5a35">` +
  `<rect x="-4" y="-60" width="8" height="60" fill="#5b4330"/>` +
  `<ellipse cx="0" cy="-66" rx="58" ry="14"/><ellipse cx="-18" cy="-78" rx="34" ry="10"/>` +
  `<ellipse cx="22" cy="-80" rx="30" ry="9"/></g>`;

const person = (x, y, color, k = 1) =>
  `<g transform="translate(${x} ${y}) scale(${k})"><circle cx="0" cy="-58" r="8" fill="#3b2a20"/>` +
  `<path d="M-10 -48 h20 l4 32 h-28 z" fill="${color}"/>` +
  `<rect x="-8" y="-16" width="6" height="16" fill="#3b2a20"/><rect x="2" y="-16" width="6" height="16" fill="#3b2a20"/></g>`;

const hut = (x, y) =>
  `<g transform="translate(${x} ${y})"><rect x="-30" y="-36" width="60" height="36" fill="#b98a5e"/>` +
  `<path d="M-38 -34 L0 -64 L38 -34 Z" fill="#8a6a3c"/><rect x="-7" y="-22" width="14" height="22" fill="#4a3424"/></g>`;

const rig = (x, y) =>
  `<g transform="translate(${x} ${y})"><rect x="-70" y="-30" width="120" height="30" fill="#c9502e"/>` +
  `<rect x="50" y="-46" width="44" height="46" fill="#a8401f"/><rect x="60" y="-40" width="24" height="16" fill="#cfe3ea"/>` +
  `<circle cx="-46" cy="4" r="12" fill="#222"/><circle cx="20" cy="4" r="12" fill="#222"/><circle cx="74" cy="4" r="12" fill="#222"/>` +
  `<path d="M-40 -30 L-10 -190 L20 -30" stroke="#e0b030" stroke-width="7" fill="none"/>` +
  `<path d="M-32 -70 L12 -70 M-26 -110 L6 -110 M-20 -150 L0 -150" stroke="#e0b030" stroke-width="4"/>` +
  `<rect x="-14" y="-30" width="8" height="60" fill="#555"/></g>`;

const pump = (x, y, broken) =>
  `<g transform="translate(${x} ${y})"><ellipse cx="0" cy="6" rx="80" ry="16" fill="#a9a59c"/>` +
  `<rect x="-60" y="-4" width="120" height="12" fill="#bdb8ad"/>` +
  `<rect x="-8" y="-86" width="16" height="84" fill="#2f5d8a"/><rect x="-14" y="-96" width="28" height="12" fill="#2f5d8a"/>` +
  `<rect x="8" y="-58" width="26" height="8" fill="#2f5d8a"/>` +
  (broken
    ? `<path d="M-6 -92 L-46 -70" stroke="#6b6b6b" stroke-width="6" stroke-dasharray="6 5"/>`
    : `<path d="M-6 -92 L-70 -120" stroke="#2f5d8a" stroke-width="7"/>` +
      `<path d="M30 -50 q4 18 2 34" stroke="#6fc3ea" stroke-width="6" fill="none"/>`) +
  `<path d="M18 -16 h30 l-4 20 h-22 z" fill="#d8a531"/></g>`;

const plan = () =>
  `<rect x="80" y="40" width="480" height="300" fill="#f6f3ea" stroke="#9aa" />` +
  `<path d="M120 300 C 200 120, 300 260, 520 90" stroke="#2f5d8a" stroke-width="3" fill="none" stroke-dasharray="8 6"/>` +
  `<circle cx="210" cy="190" r="10" fill="#c9502e"/><circle cx="430" cy="140" r="10" fill="#c9502e"/>` +
  `<text x="226" y="196" font-family="monospace" font-size="16" fill="#333">Village A</text>` +
  `<text x="446" y="146" font-family="monospace" font-size="16" fill="#333">Village B</text>` +
  `<text x="100" y="70" font-family="monospace" font-size="18" fill="#333">Borehole siting survey</text>`;

/// kind: drilling | pump | water | broken | survey | lab | invoice
function photo({ kind, caption, seed = 1 }) {
  const r = rng(seed);
  const sky = kind === "broken" ? ["#d9c9a4", "#efe2c2"] : ["#8cc4e0", "#e9f3f2"];
  let scene = "";
  if (kind === "survey" || kind === "invoice" || kind === "lab") {
    scene = plan();
    if (kind === "lab")
      scene =
        `<rect x="0" y="0" width="${W}" height="${H}" fill="#eef2f3"/>` +
        [0, 1, 2, 3, 4]
          .map(
            (i) =>
              `<rect x="${120 + i * 84}" y="120" width="34" height="150" rx="12" fill="#ffffff" stroke="#8aa"/>` +
              `<rect x="${124 + i * 84}" y="${170 + i * 6}" width="26" height="${96 - i * 6}" rx="9" fill="#9ed2ea"/>`
          )
          .join("") +
        `<text x="120" y="80" font-family="monospace" font-size="18" fill="#234">Water quality: E. coli 0 CFU/100mL</text>`;
  } else {
    scene =
      `<defs><linearGradient id="s${seed}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${sky[0]}"/>` +
      `<stop offset="1" stop-color="${sky[1]}"/></linearGradient></defs>` +
      `<rect width="${W}" height="${H}" fill="url(#s${seed})"/>` +
      `<circle cx="${80 + r() * 480}" cy="${50 + r() * 30}" r="26" fill="#fbe7a1"/>` +
      `<path d="M0 270 Q ${160 + r() * 80} ${240 + r() * 20} 320 262 T ${W} 255 V ${H} H 0 Z" fill="#d7b37c"/>` +
      `<rect y="300" width="${W}" height="${H - 300}" fill="#c99d63"/>` +
      tree(60 + r() * 60, 290, 0.9 + r() * 0.3) +
      tree(520 + r() * 60, 280, 0.7 + r() * 0.3) +
      hut(150 + r() * 40, 300) +
      (kind === "drilling" ? rig(380, 330) + person(250, 340, "#2f5d8a") : "") +
      (kind === "pump" || kind === "water" || kind === "broken" ? pump(340, 330, kind === "broken") : "") +
      (kind === "water" ? person(420, 352, "#c9502e") + person(250, 350, "#5a7d3a", 0.8) : "") +
      (kind === "pump" ? person(470, 352, "#7a4a9a") : "") +
      (kind === "broken" ? person(460, 352, "#c9502e") : "");
  }
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}">` +
    scene +
    `<rect y="${H - 34}" width="${W}" height="34" fill="rgba(0,0,0,0.62)"/>` +
    `<text x="12" y="${H - 12}" font-family="monospace" font-size="14" fill="#fff">${caption}</text></svg>`
  );
}

module.exports = { photo };
