#!/usr/bin/env node
/**
 * Generates README visual assets for Tile (Light + Dark pairs):
 *   - docs/assets/size-cycle-{light,dark}.svg
 *   - docs/assets/corner-composition-{light,dark}.svg
 *   - docs/assets/display-throws-{light,dark}.svg
 *   - docs/assets/hero-{light,dark}.webp (requires Chrome/Chromium/Edge + img2webp)
 *
 * Usage:
 *   node docs/assets/generate.mjs                # generate all 8 assets
 *   node docs/assets/generate.mjs --svg-only     # generate only the 6 static SVGs (zero external deps)
 *   node docs/assets/generate.mjs --hero-only    # generate only the 2 animated WebP heroes
 *   node docs/assets/generate.mjs --preview-dir <dir>  # also render PNG inspection previews
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync, spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const OUT_DIR = __dirname;

const FONT_STACK =
  '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif';

/**
 * Exact tokens from DESIGN.md and apps/tile/ui/src/styles.css.
 * color-mix() values are precomputed in sRGB so SVGs render identically everywhere.
 */
const THEMES = {
  light: {
    bg: "#f7f8fa",
    surface: "#ffffff",
    surface2: "#f1f3f6",
    border: "#dfe3e8",
    text: "#1f2937",
    textDim: "#667085",
    accent: "#2563eb",
    accentText: "#ffffff",
    accentTextOnTint: "#2563eb",
    accentSoftBg: "#e8effd",
    accentSoftBorder: "#8ba9e9",
    paneFill: "#d3e0fb",
    paneBorder: "#8ba9e9",
    paneBar: "#a3bdf7",
    paneBarBorder: "#9eb6e9",
    paneLabelColor: "#1d4ed8",
    ghostStroke: "#667085",
    ghostOpacity: 0.55,
    keyBottom: "#cfd5de",
    keyActiveBottom: "#1d4ed8",
    shadowFloating: "0 7px 14px -6px rgb(0 0 0 / 55%)",
    shadowSnapped: "0 2px 4px -2px rgb(0 0 0 / 40%)",
    svgFloatShadowOpacity: 0.28,
    svgSnapShadowOpacity: 0.14,
  },
  dark: {
    bg: "#17191c",
    surface: "#22262b",
    surface2: "#2b3036",
    border: "#3a414a",
    text: "#f3f4f6",
    textDim: "#a8b0bb",
    accent: "#3b82f6",
    accentText: "#ffffff",
    accentTextOnTint: "#60a5fa",
    accentSoftBg: "#25344d",
    accentSoftBorder: "#3b82f6",
    paneFill: "#273854",
    paneBorder: "#3a5e97",
    paneBar: "#2d4d80",
    paneBarBorder: "#3a5886",
    paneLabelColor: "#93c5fd",
    ghostStroke: "#a8b0bb",
    ghostOpacity: 0.55,
    keyBottom: "#181b1f",
    keyActiveBottom: "#2563eb",
    shadowFloating: "0 7px 14px -6px rgb(0 0 0 / 65%)",
    shadowSnapped: "0 2px 4px -2px rgb(0 0 0 / 45%)",
    svgFloatShadowOpacity: 0.42,
    svgSnapShadowOpacity: 0.22,
  },
};

function esc(str) {
  return String(str)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function fmt(n) {
  return Number.isInteger(n) ? String(n) : Number(n.toFixed(2)).toString();
}

// ============================================================================
// Shared SVG primitives
// ============================================================================

function svgDoc(width, height, title, desc, defs, body) {
  return [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" role="img" aria-label="${esc(title)}">`,
    `  <title>${esc(title)}</title>`,
    `  <desc>${esc(desc)}</desc>`,
    defs ? `  <defs>\n${defs}\n  </defs>` : "",
    body,
    `</svg>`,
    "",
  ]
    .filter(Boolean)
    .join("\n");
}

function shadowDefs(t, prefix) {
  return [
    `    <filter id="${prefix}-float" x="-18%" y="-18%" width="136%" height="146%">`,
    `      <feDropShadow dx="0" dy="4" stdDeviation="3.5" flood-color="#000000" flood-opacity="${t.svgFloatShadowOpacity}"/>`,
    `    </filter>`,
    `    <filter id="${prefix}-snap" x="-10%" y="-10%" width="120%" height="126%">`,
    `      <feDropShadow dx="0" dy="1.5" stdDeviation="1.4" flood-color="#000000" flood-opacity="${t.svgSnapShadowOpacity}"/>`,
    `    </filter>`,
  ].join("\n");
}

function outerCard(w, h, t) {
  return `  <rect x="0.5" y="0.5" width="${w - 1}" height="${h - 1}" rx="10" fill="${t.surface}" stroke="${t.border}" stroke-width="1"/>`;
}

function keycap(x, y, w, h, label, t, opts = {}) {
  const {
    active = false,
    tinted = false,
    fontSize = 12,
    fontWeight = 600,
    radius = 6,
  } = opts;
  const fill = active ? t.accent : tinted ? t.accentSoftBg : t.surface2;
  const stroke = active
    ? t.accent
    : tinted
      ? t.accentSoftBorder
      : t.border;
  const bottomColor = active
    ? t.keyActiveBottom
    : tinted
      ? t.accentSoftBorder
      : t.keyBottom;
  const fg = active ? t.accentText : tinted ? t.accentTextOnTint : t.text;
  const cx = x + w / 2;
  const cy = y + h / 2 + fontSize * 0.34;
  return [
    `  <g>`,
    `    <rect x="${fmt(x)}" y="${fmt(y + 1)}" width="${fmt(w)}" height="${fmt(h)}" rx="${radius}" fill="${bottomColor}"/>`,
    `    <rect x="${fmt(x + 0.5)}" y="${fmt(y + 0.5)}" width="${fmt(w - 1)}" height="${fmt(h - 1)}" rx="${radius}" fill="${fill}" stroke="${stroke}" stroke-width="1"/>`,
    `    <text x="${fmt(cx)}" y="${fmt(cy)}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="${fontSize}" font-weight="${fontWeight}" fill="${fg}">${esc(label)}</text>`,
    `  </g>`,
  ].join("\n");
}

function modifierPillRight(rightX, centerY, t, opts = {}) {
  const {
    macKeys = "⌃⌥",
    winKeys = "Win",
    macW = 46,
    winW = 46,
    prefixHold = false,
    suffixKeys = null,
    suffixW = 74,
  } = opts;
  const keyH = 24;
  const keyY = centerY - keyH / 2;
  const parts = [];
  let cursor = rightX;

  if (suffixKeys) {
    cursor -= suffixW;
    parts.push(
      keycap(cursor, keyY, suffixW, keyH, suffixKeys, t, {
        fontSize: 11.5,
        radius: 5,
      })
    );
    cursor -= 16;
    parts.push(
      `  <text x="${fmt(cursor + 8)}" y="${fmt(centerY + 4)}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">+</text>`
    );
  }

  cursor -= winW;
  parts.push(
    keycap(cursor, keyY, winW, keyH, winKeys, t, {
      tinted: prefixHold,
      fontSize: 11.5,
      radius: 5,
    })
  );
  cursor -= 6;
  parts.push(
    `  <text x="${fmt(cursor)}" y="${fmt(centerY + 4)}" text-anchor="end" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">Windows</text>`
  );
  cursor -= 58;
  parts.push(
    `  <text x="${fmt(cursor)}" y="${fmt(centerY + 4)}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12" fill="${t.textDim}">·</text>`
  );
  cursor -= 9 + macW;
  parts.push(
    keycap(cursor, keyY, macW, keyH, macKeys, t, {
      tinted: prefixHold,
      fontSize: 11.5,
      radius: 5,
    })
  );
  cursor -= 6;
  parts.push(
    `  <text x="${fmt(cursor)}" y="${fmt(centerY + 4)}" text-anchor="end" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">macOS</text>`
  );

  if (prefixHold) {
    cursor -= 48;
    const pillW = 54;
    const pillH = 22;
    const pillX = cursor - pillW;
    const pillY = centerY - pillH / 2;
    parts.push(
      `  <g>`,
      `    <rect x="${fmt(pillX + 0.5)}" y="${fmt(pillY + 0.5)}" width="${fmt(pillW - 1)}" height="${fmt(pillH - 1)}" rx="11" fill="${t.accentSoftBg}" stroke="${t.accentSoftBorder}" stroke-width="1"/>`,
      `    <circle cx="${fmt(pillX + 13)}" cy="${fmt(centerY)}" r="3" fill="${t.accentTextOnTint}"/>`,
      `    <text x="${fmt(pillX + 21)}" y="${fmt(centerY + 3.8)}" font-family="${esc(FONT_STACK)}" font-size="11" font-weight="600" fill="${t.accentTextOnTint}">Hold</text>`,
      `  </g>`
    );
  }

  return parts.join("\n");
}

/**
 * Renders an OS-neutral mini-display with optional active window pane and/or dashed ghost outline.
 * Coordinates inside pane/ghost are work-area fractions: { x, y, w, h } in [0..1].
 * Ghost is drawn ABOVE the pane so shrinking steps remain clearly visible (matches styles.css).
 */
function miniDisplay(x, y, w, h, t, opts = {}) {
  const {
    id,
    pane = null,
    ghost = null,
    floating = false,
    paneLabel = null,
    ghostLabel = null,
    filterPrefix = "sh",
    radius = 7,
  } = opts;
  const inset = 3;
  const innerX = x + inset;
  const innerY = y + inset;
  const innerW = w - inset * 2;
  const innerH = h - inset * 2;
  const parts = [];
  const defs = [];

  parts.push(
    `  <rect x="${fmt(x + 0.5)}" y="${fmt(y + 0.5)}" width="${fmt(w - 1)}" height="${fmt(h - 1)}" rx="${radius}" fill="${t.surface2}" stroke="${t.border}" stroke-width="1"/>`
  );

  if (pane) {
    const px = innerX + pane.x * innerW;
    const py = innerY + pane.y * innerH;
    const pw = pane.w * innerW;
    const ph = pane.h * innerH;
    const pr = 4.5;
    const barH = Math.max(6, Math.min(8, Math.round(h * 0.065)));
    const clipId = `${id}-pane-clip`;
    defs.push(
      `    <clipPath id="${clipId}"><rect x="${fmt(px)}" y="${fmt(py)}" width="${fmt(pw)}" height="${fmt(ph)}" rx="${pr}"/></clipPath>`
    );
    const filterRef = floating
      ? `filter="url(#${filterPrefix}-float)"`
      : `filter="url(#${filterPrefix}-snap)"`;
    parts.push(
      `  <g ${filterRef}>`,
      `    <rect x="${fmt(px)}" y="${fmt(py)}" width="${fmt(pw)}" height="${fmt(ph)}" rx="${pr}" fill="${t.paneFill}"/>`,
      `    <g clip-path="url(#${clipId})">`,
      `      <rect x="${fmt(px)}" y="${fmt(py)}" width="${fmt(pw)}" height="${fmt(barH)}" fill="${t.paneBar}"/>`,
      `      <line x1="${fmt(px)}" y1="${fmt(py + barH)}" x2="${fmt(px + pw)}" y2="${fmt(py + barH)}" stroke="${t.paneBarBorder}" stroke-width="1"/>`,
      `    </g>`,
      `    <rect x="${fmt(px + 0.5)}" y="${fmt(py + 0.5)}" width="${fmt(pw - 1)}" height="${fmt(ph - 1)}" rx="${pr}" fill="none" stroke="${t.paneBorder}" stroke-width="1"/>`,
      `  </g>`
    );
    if (paneLabel) {
      parts.push(
        `  <text x="${fmt(px + pw / 2)}" y="${fmt(py + barH + (ph - barH) / 2 + 4.5)}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="13" font-weight="600" fill="${t.paneLabelColor}">${esc(paneLabel)}</text>`
      );
    }
  }

  if (ghost) {
    const gx = innerX + ghost.x * innerW;
    const gy = innerY + ghost.y * innerH;
    const gw = ghost.w * innerW;
    const gh = ghost.h * innerH;
    parts.push(
      `  <rect x="${fmt(gx + 0.75)}" y="${fmt(gy + 0.75)}" width="${fmt(gw - 1.5)}" height="${fmt(gh - 1.5)}" rx="4.5" fill="none" stroke="${t.ghostStroke}" stroke-opacity="${t.ghostOpacity}" stroke-width="1.25" stroke-dasharray="4 3"/>`
    );
    if (ghostLabel) {
      parts.push(
        `  <text x="${fmt(gx + gw / 2)}" y="${fmt(gy + gh / 2 + 4.5)}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.textDim}">${esc(ghostLabel)}</text>`
      );
    }
  }

  return { defs: defs.join("\n"), svg: parts.join("\n") };
}

function hArrow(x1, x2, y, t, opts = {}) {
  const { reverse = false, stroke = t.textDim } = opts;
  const dir = reverse ? -1 : 1;
  const tipX = reverse ? x1 : x2;
  const head = `M ${fmt(tipX - dir * 5.5)} ${fmt(y - 3.8)} L ${fmt(tipX)} ${fmt(y)} L ${fmt(tipX - dir * 5.5)} ${fmt(y + 3.8)}`;
  return [
    `  <g fill="none" stroke="${stroke}" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">`,
    `    <line x1="${fmt(x1)}" y1="${fmt(y)}" x2="${fmt(x2)}" y2="${fmt(y)}"/>`,
    `    <path d="${head}"/>`,
    `  </g>`,
  ].join("\n");
}

function vArrow(x, y1, y2, t, opts = {}) {
  const { up = false, stroke = t.textDim } = opts;
  const dir = up ? -1 : 1;
  const tipY = up ? y1 : y2;
  const head = `M ${fmt(x - 3.8)} ${fmt(tipY - dir * 5.5)} L ${fmt(x)} ${fmt(tipY)} L ${fmt(x + 3.8)} ${fmt(tipY - dir * 5.5)}`;
  return [
    `  <g fill="none" stroke="${stroke}" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">`,
    `    <line x1="${fmt(x)}" y1="${fmt(y1)}" x2="${fmt(x)}" y2="${fmt(y2)}"/>`,
    `    <path d="${head}"/>`,
    `  </g>`,
  ].join("\n");
}

// ============================================================================
// Diagram 1: Size Cycle (size-cycle-{light,dark}.svg)
// ============================================================================

function renderSizeCycleSvg(themeName) {
  const t = THEMES[themeName];
  const W = 840;
  const H = 408;
  const defs = [shadowDefs(t, `sc-${themeName}`)];
  const body = [];

  body.push(outerCard(W, H, t));

  // Header
  body.push(
    `  <text x="24" y="34" font-family="${esc(FONT_STACK)}" font-size="13.5" font-weight="600" fill="${t.text}">Press again to cycle width</text>`
  );
  body.push(modifierPillRight(816, 29, t));
  body.push(
    `  <line x1="24" y1="54" x2="816" y2="54" stroke="${t.border}" stroke-width="1"/>`
  );

  const dispW = 176;
  const dispH = 110;
  const xs = [184, 412, 640];

  // Row 1: Side arrows (Left)
  const r1Y = 72;
  const r1Mid = r1Y + dispH / 2;
  body.push(keycap(24, r1Y + 14, 42, 32, "←", t, { active: true, fontSize: 15 }));
  body.push(
    `  <text x="24" y="${r1Y + 67}" font-family="${esc(FONT_STACK)}" font-size="13" font-weight="600" fill="${t.text}">Side arrows</text>`,
    `  <text x="24" y="${r1Y + 84}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">← left · → right</text>`
  );

  const sideSteps = [
    {
      pane: { x: 0, y: 0, w: 0.5, h: 1 },
      ghost: { x: 0, y: 0, w: 2 / 3, h: 1 },
      frac: "½",
      label: "Half",
    },
    {
      pane: { x: 0, y: 0, w: 2 / 3, h: 1 },
      ghost: { x: 0, y: 0, w: 1 / 3, h: 1 },
      frac: "⅔",
      label: "Two thirds",
    },
    {
      pane: { x: 0, y: 0, w: 1 / 3, h: 1 },
      ghost: { x: 0, y: 0, w: 0.5, h: 1 },
      frac: "⅓",
      label: "Third  ·  ↻ ½",
    },
  ];

  sideSteps.forEach((step, i) => {
    const d = miniDisplay(xs[i], r1Y, dispW, dispH, t, {
      id: `sc-${themeName}-side-${i}`,
      pane: step.pane,
      ghost: step.ghost,
      filterPrefix: `sc-${themeName}`,
    });
    if (d.defs) defs.push(d.defs);
    body.push(d.svg);

    const cx = xs[i] + dispW / 2;
    body.push(
      `  <text x="${fmt(cx)}" y="${r1Y + dispH + 20}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12">` +
        `<tspan font-weight="600" fill="${t.text}">${esc(step.frac)}</tspan>` +
        `<tspan fill="${t.textDim}">  ${esc(step.label)}</tspan>` +
        `</text>`
    );

    if (i < 2) {
      const gx1 = xs[i] + dispW + 10;
      const gx2 = xs[i + 1] - 10;
      const gcx = (gx1 + gx2) / 2;
      body.push(
        keycap(gcx - 13, r1Mid - 28, 26, 20, "←", t, {
          fontSize: 11.5,
          radius: 4.5,
        })
      );
      body.push(hArrow(gx1, gx2, r1Mid + 4, t));
    }
  });

  // Divider
  body.push(
    `  <line x1="24" y1="216" x2="816" y2="216" stroke="${t.border}" stroke-width="1"/>`
  );

  // Row 2: Center column (Enter)
  const r2Y = 234;
  const r2Mid = r2Y + dispH / 2;
  body.push(
    keycap(24, r2Y + 14, 72, 32, "Enter", t, { active: true, fontSize: 13 })
  );
  body.push(
    `  <text x="24" y="${r2Y + 67}" font-family="${esc(FONT_STACK)}" font-size="13" font-weight="600" fill="${t.text}">Center column</text>`,
    `  <text x="24" y="${r2Y + 84}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">full height, centred</text>`
  );

  const centerSteps = [
    {
      pane: { x: 0.25, y: 0, w: 0.5, h: 1 },
      ghost: { x: 1 / 6, y: 0, w: 2 / 3, h: 1 },
      frac: "½",
      label: "Center half",
    },
    {
      pane: { x: 1 / 6, y: 0, w: 2 / 3, h: 1 },
      ghost: { x: 1 / 3, y: 0, w: 1 / 3, h: 1 },
      frac: "⅔",
      label: "Center two thirds",
    },
    {
      pane: { x: 1 / 3, y: 0, w: 1 / 3, h: 1 },
      ghost: { x: 0.25, y: 0, w: 0.5, h: 1 },
      frac: "⅓",
      label: "Center third  ·  ↻ ½",
    },
  ];

  centerSteps.forEach((step, i) => {
    const d = miniDisplay(xs[i], r2Y, dispW, dispH, t, {
      id: `sc-${themeName}-center-${i}`,
      pane: step.pane,
      ghost: step.ghost,
      filterPrefix: `sc-${themeName}`,
    });
    if (d.defs) defs.push(d.defs);
    body.push(d.svg);

    const cx = xs[i] + dispW / 2;
    body.push(
      `  <text x="${fmt(cx)}" y="${r2Y + dispH + 20}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12">` +
        `<tspan font-weight="600" fill="${t.text}">${esc(step.frac)}</tspan>` +
        `<tspan fill="${t.textDim}">  ${esc(step.label)}</tspan>` +
        `</text>`
    );

    if (i < 2) {
      const gx1 = xs[i] + dispW + 10;
      const gx2 = xs[i + 1] - 10;
      const gcx = (gx1 + gx2) / 2;
      body.push(
        keycap(gcx - 13, r2Mid - 28, 26, 20, "↵", t, {
          fontSize: 11.5,
          radius: 4.5,
        })
      );
      body.push(hArrow(gx1, gx2, r2Mid + 4, t));
    }
  });

  // Footer legend for dashed outline
  body.push(
    `  <g>`,
    `    <rect x="24.75" y="381.75" width="14.5" height="10.5" rx="2.5" fill="none" stroke="${t.ghostStroke}" stroke-opacity="${t.ghostOpacity}" stroke-width="1.2" stroke-dasharray="3 2"/>`,
    `    <text x="46" y="391" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">Dashed outline shows the next press in the cycle (configurable in Settings → Behaviour)</text>`,
    `  </g>`
  );

  return svgDoc(
    W,
    H,
    "Tile size cycling diagram",
    "Repeating left or right arrow cycles a side pane through half, two thirds, and third, then repeats. Repeating Enter cycles a full-height centered column through the same widths.",
    defs.join("\n"),
    body.join("\n")
  );
}

// ============================================================================
// Diagram 2: Corner Composition (corner-composition-{light,dark}.svg)
// ============================================================================

function renderCornerCompositionSvg(themeName) {
  const t = THEMES[themeName];
  const W = 840;
  const H = 532;
  const defs = [shadowDefs(t, `cc-${themeName}`)];
  const body = [];

  body.push(outerCard(W, H, t));

  // Header
  body.push(
    `  <text x="24" y="34" font-family="${esc(FONT_STACK)}" font-size="13.5" font-weight="600" fill="${t.text}">Corner composition — keep the modifier held</text>`
  );
  body.push(modifierPillRight(816, 29, t, { prefixHold: true }));
  body.push(
    `  <line x1="24" y1="54" x2="816" y2="54" stroke="${t.border}" stroke-width="1"/>`
  );

  // Geometry constants
  const dispW = 160;
  const dispH = 100;
  const centerX = 328; // center spine x: 328..488 (mid 408)
  const rightX = 648; // right push-past x: 648..808 (mid 728)
  const topY = 90; // 90..190 (mid 140)
  const midY = 242; // 242..342 (mid 292)
  const botY = 394; // 394..494 (mid 444)

  // Center Spine: Top corner, Full height (start), Bottom corner
  // Top corner
  body.push(
    `  <text x="${centerX + dispW / 2}" y="${topY - 11}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.text}">Top corner</text>`
  );
  {
    const d = miniDisplay(centerX, topY, dispW, dispH, t, {
      id: `cc-${themeName}-top`,
      pane: { x: 0, y: 0, w: 1 / 3, h: 0.5 },
      ghost: { x: 0, y: 0, w: 1 / 3, h: 1 },
      filterPrefix: `cc-${themeName}`,
    });
    if (d.defs) defs.push(d.defs);
    body.push(d.svg);
  }

  // Vertical connector: Full <-> Top
  {
    const y1 = topY + dispH + 7;
    const y2 = midY - 7;
    const ym = (y1 + y2) / 2;
    // Left arrow going UP (Full -> Top)
    body.push(vArrow(centerX + 54, y1, y2, t, { up: true }));
    body.push(
      keycap(centerX + 20, ym - 11, 26, 22, "↑", t, {
        active: true,
        fontSize: 12,
        radius: 5,
      })
    );
    // Right arrow going DOWN (Top -> Full)
    body.push(vArrow(centerX + 106, y1, y2, t, { up: false }));
    body.push(
      keycap(centerX + 114, ym - 11, 26, 22, "↓", t, {
        fontSize: 12,
        radius: 5,
      })
    );
  }

  // Middle: Full height (start)
  {
    const d = miniDisplay(centerX, midY, dispW, dispH, t, {
      id: `cc-${themeName}-full`,
      pane: { x: 0, y: 0, w: 1 / 3, h: 1 },
      paneLabel: "⅓",
      filterPrefix: `cc-${themeName}`,
    });
    if (d.defs) defs.push(d.defs);
    body.push(d.svg);
  }
  // Subtle "keeps ⅓ width" callout to right of Full
  body.push(
    `  <text x="${centerX + dispW + 16}" y="${midY + dispH / 2 - 4}" font-family="${esc(FONT_STACK)}" font-size="12" font-weight="600" fill="${t.text}">Full height</text>`,
    `  <text x="${centerX + dispW + 16}" y="${midY + dispH / 2 + 13}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">↑ / ↓ step vertically</text>`,
    `  <text x="${centerX + dispW + 16}" y="${midY + dispH / 2 + 28}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">while keeping ⅓ width</text>`
  );

  // Vertical connector: Full <-> Bottom
  {
    const y1 = midY + dispH + 7;
    const y2 = botY - 7;
    const ym = (y1 + y2) / 2;
    // Left arrow going DOWN (Full -> Bottom)
    body.push(vArrow(centerX + 54, y1, y2, t, { up: false }));
    body.push(
      keycap(centerX + 20, ym - 11, 26, 22, "↓", t, {
        active: true,
        fontSize: 12,
        radius: 5,
      })
    );
    // Right arrow going UP (Bottom -> Full)
    body.push(vArrow(centerX + 106, y1, y2, t, { up: true }));
    body.push(
      keycap(centerX + 114, ym - 11, 26, 22, "↑", t, {
        fontSize: 12,
        radius: 5,
      })
    );
  }

  // Bottom corner
  {
    const d = miniDisplay(centerX, botY, dispW, dispH, t, {
      id: `cc-${themeName}-bot`,
      pane: { x: 0, y: 0.5, w: 1 / 3, h: 0.5 },
      ghost: { x: 0, y: 0, w: 1 / 3, h: 1 },
      filterPrefix: `cc-${themeName}`,
    });
    if (d.defs) defs.push(d.defs);
    body.push(d.svg);
  }
  body.push(
    `  <text x="${centerX + dispW / 2}" y="${botY + dispH + 20}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.text}">Bottom corner</text>`
  );

  // Left Column: Start entry (aligned with Full)
  {
    const cy = midY + dispH / 2;
    body.push(keycap(24, cy - 34, 34, 26, "←", t, { active: true, fontSize: 13 }));
    body.push(
      `  <text x="69" y="${cy - 17}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">or</text>`
    );
    body.push(keycap(80, cy - 34, 34, 26, "→", t, { fontSize: 13 }));
    body.push(
      `  <text x="24" y="${cy + 12}" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.text}">Pick a side &amp; width</text>`,
      `  <text x="24" y="${cy + 29}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">Repeat to size (shown at ⅓)</text>`
    );
    body.push(hArrow(244, centerX - 10, cy, t));
  }

  // Left Column Top: Horizontal moves while in a corner
  {
    const y0 = topY + 2;
    body.push(
      `  <text x="24" y="${y0 + 10}" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.text}">While in a corner</text>`
    );
    body.push(keycap(24, y0 + 22, 26, 22, "←", t, { active: true, fontSize: 11.5, radius: 5 }));
    body.push(
      `  <text x="58" y="${y0 + 37}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.text}">Same side</text>`,
      `  <text x="58" y="${y0 + 52}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">cycles corner width (½ · ⅔ · ⅓)</text>`
    );
    body.push(keycap(24, y0 + 62, 26, 22, "→", t, { fontSize: 11.5, radius: 5 }));
    body.push(
      `  <text x="58" y="${y0 + 77}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.text}">Opposite side</text>`,
      `  <text x="58" y="${y0 + 92}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">resets to full right half</text>`
    );
    // Subtle dotted link toward Top corner
    body.push(
      `  <line x1="258" y1="${topY + 50}" x2="${centerX - 12}" y2="${topY + 50}" stroke="${t.border}" stroke-width="1.2" stroke-dasharray="3 3"/>`
    );
  }

  // Left Column Bottom: Release modifier note
  {
    const y0 = botY + 18;
    body.push(
      `  <text x="24" y="${y0 + 12}" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.text}">Release the modifier</text>`,
      `  <text x="24" y="${y0 + 31}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">Ends composition immediately.</text>`,
      `  <text x="24" y="${y0 + 48}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">↑ maximizes and ↓ restores again.</text>`
    );
  }

  // Right Column Top: Push past Top -> Maximize
  {
    const cy = topY + dispH / 2;
    const ax1 = centerX + dispW + 12;
    const ax2 = rightX - 12;
    const amx = (ax1 + ax2) / 2;
    body.push(
      keycap(amx - 40, cy - 30, 26, 22, "↑", t, {
        active: true,
        fontSize: 12,
        radius: 5,
      }),
      `  <text x="${fmt(amx - 7)}" y="${cy - 15}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">past top</text>`
    );
    body.push(hArrow(ax1, ax2, cy + 4, t));

    body.push(
      `  <text x="${rightX + dispW / 2}" y="${topY - 11}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.text}">Maximize</text>`
    );
    const d = miniDisplay(rightX, topY, dispW, dispH, t, {
      id: `cc-${themeName}-max`,
      pane: { x: 0, y: 0, w: 1, h: 1 },
      filterPrefix: `cc-${themeName}`,
    });
    if (d.defs) defs.push(d.defs);
    body.push(d.svg);
    body.push(
      `  <text x="${rightX + dispW / 2}" y="${topY + dispH + 18}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">Ends composition</text>`
    );
  }

  // Right Column Bottom: Push past Bottom -> Restore
  {
    const cy = botY + dispH / 2;
    const ax1 = centerX + dispW + 12;
    const ax2 = rightX - 12;
    const amx = (ax1 + ax2) / 2;
    body.push(
      keycap(amx - 48, cy - 30, 26, 22, "↓", t, {
        active: true,
        fontSize: 12,
        radius: 5,
      }),
      `  <text x="${fmt(amx - 15)}" y="${cy - 15}" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">past bottom</text>`
    );
    body.push(hArrow(ax1, ax2, cy + 4, t));

    body.push(
      `  <text x="${rightX + dispW / 2}" y="${botY - 11}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="12.5" font-weight="600" fill="${t.text}">Restore</text>`
    );
    const d = miniDisplay(rightX, botY, dispW, dispH, t, {
      id: `cc-${themeName}-restore`,
      pane: { x: 0.22, y: 0.16, w: 0.56, h: 0.64 },
      ghost: { x: 0, y: 0.5, w: 1 / 3, h: 0.5 },
      floating: true,
      filterPrefix: `cc-${themeName}`,
    });
    if (d.defs) defs.push(d.defs);
    body.push(d.svg);
    body.push(
      `  <text x="${rightX + dispW / 2}" y="${botY + dispH + 20}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">Ends composition</text>`
    );
  }

  return svgDoc(
    W,
    H,
    "Tile corner composition state diagram",
    "Keep the base modifier held after pressing left or right arrow. Up and down step between Top corner, Full height, and Bottom corner while preserving width. Same side arrow cycles corner width, opposite arrow resets to the opposite full half, and pushing past top maximizes or past bottom restores and ends composition.",
    defs.join("\n"),
    body.join("\n")
  );
}

// ============================================================================
// Diagram 3: Display Throws (display-throws-{light,dark}.svg)
// ============================================================================

function renderDisplayThrowsSvg(themeName) {
  const t = THEMES[themeName];
  const W = 840;
  const H = 418;
  const defs = [shadowDefs(t, `dt-${themeName}`)];
  const body = [];

  body.push(outerCard(W, H, t));

  // Header
  body.push(
    `  <text x="24" y="34" font-family="${esc(FONT_STACK)}" font-size="13.5" font-weight="600" fill="${t.text}">Moving between displays</text>`
  );
  body.push(
    modifierPillRight(816, 29, t, {
      macKeys: "⌃⌘⌥",
      winKeys: "Win+Alt",
      macW: 58,
      winW: 72,
      suffixKeys: "← → ↑ ↓",
      suffixW: 76,
    })
  );
  body.push(
    `  <line x1="24" y1="54" x2="816" y2="54" stroke="${t.border}" stroke-width="1"/>`
  );

  // Shared two-monitor geometry (bottom-aligned 4K + smaller 1080p)
  const leftX = 324;
  const leftW = 232;
  const leftH = 130;
  const rightX = 640;
  const rightW = 176;
  const rightH = 110;
  const rightOffsetY = leftH - rightH; // 20px lower top edge, flush bottom

  // Row 1: Slot preservation
  const r1Y = 74;
  body.push(
    `  <text x="24" y="${r1Y + 40}" font-family="${esc(FONT_STACK)}" font-size="13" font-weight="600" fill="${t.text}">Recognised slot</text>`,
    `  <text x="24" y="${r1Y + 61}" font-family="${esc(FONT_STACK)}" font-size="12" fill="${t.textDim}">Keeps the exact tile slot on arrival —</text>`,
    `  <text x="24" y="${r1Y + 79}" font-family="${esc(FONT_STACK)}" font-size="12" fill="${t.textDim}">a left third stays a left third,</text>`,
    `  <text x="24" y="${r1Y + 97}" font-family="${esc(FONT_STACK)}" font-size="12" fill="${t.textDim}">resized to fit the target display.</text>`
  );

  {
    const dLeft = miniDisplay(leftX, r1Y, leftW, leftH, t, {
      id: `dt-${themeName}-slot-l`,
      ghost: { x: 0, y: 0, w: 1 / 3, h: 1 },
      ghostLabel: "⅓",
      filterPrefix: `dt-${themeName}`,
    });
    if (dLeft.defs) defs.push(dLeft.defs);
    body.push(dLeft.svg);

    const dRight = miniDisplay(rightX, r1Y + rightOffsetY, rightW, rightH, t, {
      id: `dt-${themeName}-slot-r`,
      pane: { x: 0, y: 0, w: 1 / 3, h: 1 },
      paneLabel: "⅓",
      filterPrefix: `dt-${themeName}`,
    });
    if (dRight.defs) defs.push(dRight.defs);
    body.push(dRight.svg);

    const arrowY = r1Y + rightOffsetY + rightH / 2 + 4;
    const amx = (leftX + leftW + rightX) / 2;
    body.push(
      keycap(amx - 14, arrowY - 32, 28, 22, "→", t, {
        active: true,
        fontSize: 12,
        radius: 5,
      })
    );
    body.push(hArrow(leftX + leftW + 12, rightX - 12, arrowY, t));

    body.push(
      `  <text x="${leftX + leftW / 2}" y="${r1Y + leftH + 18}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">4K display · Left ⅓</text>`,
      `  <text x="${rightX + rightW / 2}" y="${r1Y + leftH + 18}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">1080p display · Left ⅓</text>`
    );
  }

  // Divider
  body.push(
    `  <line x1="24" y1="234" x2="816" y2="234" stroke="${t.border}" stroke-width="1"/>`
  );

  // Row 2: Unslotted floating window (proportional mapping)
  const r2Y = 252;
  body.push(
    `  <text x="24" y="${r2Y + 34}" font-family="${esc(FONT_STACK)}" font-size="13" font-weight="600" fill="${t.text}">Floating window</text>`,
    `  <text x="24" y="${r2Y + 55}" font-family="${esc(FONT_STACK)}" font-size="12" fill="${t.textDim}">Unslotted windows map proportionally</text>`,
    `  <text x="24" y="${r2Y + 73}" font-family="${esc(FONT_STACK)}" font-size="12" fill="${t.textDim}">by work-area fractions — never</text>`,
    `  <text x="24" y="${r2Y + 91}" font-family="${esc(FONT_STACK)}" font-size="12" fill="${t.textDim}">overflowing across mixed DPI.</text>`
  );

  {
    const floatRect = { x: 0.44, y: 0.14, w: 0.48, h: 0.72 };
    const dLeft = miniDisplay(leftX, r2Y, leftW, leftH, t, {
      id: `dt-${themeName}-flt-l`,
      ghost: floatRect,
      filterPrefix: `dt-${themeName}`,
    });
    if (dLeft.defs) defs.push(dLeft.defs);
    body.push(dLeft.svg);

    const dRight = miniDisplay(rightX, r2Y + rightOffsetY, rightW, rightH, t, {
      id: `dt-${themeName}-flt-r`,
      pane: floatRect,
      floating: true,
      filterPrefix: `dt-${themeName}`,
    });
    if (dRight.defs) defs.push(dRight.defs);
    body.push(dRight.svg);

    const arrowY = r2Y + rightOffsetY + rightH / 2 + 4;
    const amx = (leftX + leftW + rightX) / 2;
    body.push(
      keycap(amx - 14, arrowY - 32, 28, 22, "→", t, {
        active: true,
        fontSize: 12,
        radius: 5,
      })
    );
    body.push(hArrow(leftX + leftW + 12, rightX - 12, arrowY, t));

    body.push(
      `  <text x="${leftX + leftW / 2}" y="${r2Y + leftH + 18}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">4K display · 48% × 72%</text>`,
      `  <text x="${rightX + rightW / 2}" y="${r2Y + leftH + 18}" text-anchor="middle" font-family="${esc(FONT_STACK)}" font-size="11.5" fill="${t.textDim}">1080p display · 48% × 72%</text>`
    );
  }

  return svgDoc(
    W,
    H,
    "Tile moving windows between displays diagram",
    "Moving a tiled window to an adjacent display preserves its exact tile slot, such as keeping a left third on a smaller screen. Moving an unslotted floating window maps its relative position and size proportionally by work-area fractions so it stays inside bounds across mixed resolutions and DPI.",
    defs.join("\n"),
    body.join("\n")
  );
}

// ============================================================================
// Hero Animated WebP Timeline & HTML Renderer
// ============================================================================

const HERO_W = 840;
const HERO_H = 316;
const HERO_DPR = 2;

const RECTS = {
  floating: { x: 0.14, y: 0.14, w: 0.56, h: 0.64 },
  leftHalf: { x: 0, y: 0, w: 0.5, h: 1 },
  leftTwoThirds: { x: 0, y: 0, w: 2 / 3, h: 1 },
  leftThird: { x: 0, y: 0, w: 1 / 3, h: 1 },
  topLeftThird: { x: 0, y: 0, w: 1 / 3, h: 0.5 },
  maximize: { x: 0, y: 0, w: 1, h: 1 },
};

// Exact cubic-bezier(0.22, 1, 0.36, 1) solver (Newton-Raphson)
function cubicBezier(x1, y1, x2, y2, t) {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  const cx = 3 * x1;
  const bx = 3 * (x2 - x1) - cx;
  const ax = 1 - cx - bx;
  const cy = 3 * y1;
  const by = 3 * (y2 - y1) - cy;
  const ay = 1 - cy - by;
  const sampleX = (u) => ((ax * u + bx) * u + cx) * u;
  const sampleDX = (u) => (3 * ax * u + 2 * bx) * u + cx;
  const sampleY = (u) => ((ay * u + by) * u + cy) * u;
  let u = t;
  for (let i = 0; i < 6; i++) {
    const err = sampleX(u) - t;
    const dx = sampleDX(u);
    if (Math.abs(err) < 1e-6 || Math.abs(dx) < 1e-6) break;
    u -= err / dx;
  }
  return sampleY(Math.min(1, Math.max(0, u)));
}

function easeOut(t) {
  return cubicBezier(0.22, 1, 0.36, 1, t);
}

function lerpRect(a, b, p) {
  return {
    x: a.x + (b.x - a.x) * p,
    y: a.y + (b.y - a.y) * p,
    w: a.w + (b.w - a.w) * p,
    h: a.h + (b.h - a.h) * p,
  };
}

/**
 * Builds deterministic frame schedule with variable hold durations for img2webp.
 * Glides run at ~30fps (12 frames * 32ms = 384ms) for silky motion; static pauses
 * encode as single frames with exact ms durations for tiny, sharp WebP files.
 */
function buildHeroFrames() {
  const frames = [];
  const GLIDE_FRAMES = 12;
  const FRAME_MS = 32; // 31.25 fps -> 384ms glide

  function pushGlide({
    from,
    to,
    ghost,
    fromFloat,
    toFloat,
    activeKey,
    modHeld,
    stepIdx,
    title,
    subtitle,
    holdMs,
    releaseModAfterMs = null,
  }) {
    for (let i = 1; i <= GLIDE_FRAMES; i++) {
      const t = i / GLIDE_FRAMES;
      const p = easeOut(t);
      const elapsed = i * FRAME_MS;
      frames.push({
        duration: FRAME_MS,
        pane: lerpRect(from, to, p),
        ghost,
        ghostOpacity: Math.min(1, Math.max(0, (t - 0.15) / 0.5)),
        floatMix: fromFloat + (toFloat - fromFloat) * p,
        activeKey: elapsed <= 320 ? activeKey : null,
        modHeld,
        stepIdx,
        title,
        subtitle,
      });
    }
    if (releaseModAfterMs !== null && releaseModAfterMs < holdMs) {
      frames.push({
        duration: releaseModAfterMs,
        pane: to,
        ghost,
        ghostOpacity: 1,
        floatMix: toFloat,
        activeKey: null,
        modHeld,
        stepIdx,
        title,
        subtitle,
      });
      frames.push({
        duration: holdMs - releaseModAfterMs,
        pane: to,
        ghost,
        ghostOpacity: 1,
        floatMix: toFloat,
        activeKey: null,
        modHeld: false,
        stepIdx,
        title,
        subtitle,
      });
    } else {
      frames.push({
        duration: holdMs,
        pane: to,
        ghost,
        ghostOpacity: 1,
        floatMix: toFloat,
        activeKey: null,
        modHeld,
        stepIdx,
        title,
        subtitle,
      });
    }
  }

  // Beat 0: Floating rest (620ms)
  frames.push({
    duration: 620,
    pane: RECTS.floating,
    ghost: RECTS.leftHalf,
    ghostOpacity: 1,
    floatMix: 1,
    activeKey: null,
    modHeld: false,
    stepIdx: -1,
    title: "One modifier, four arrows",
    subtitle: "Hold the modifier and press an arrow to tile",
  });

  // Beat 1: Left 1/2 (modifier pressed & held)
  pushGlide({
    from: RECTS.floating,
    to: RECTS.leftHalf,
    ghost: RECTS.leftTwoThirds,
    fromFloat: 1,
    toFloat: 0,
    activeKey: "left",
    modHeld: true,
    stepIdx: 0,
    title: "Left half",
    subtitle: "Press ← to snap left",
    holdMs: 620,
  });

  // Beat 2: Left 2/3 (modifier still held)
  pushGlide({
    from: RECTS.leftHalf,
    to: RECTS.leftTwoThirds,
    ghost: RECTS.leftThird,
    fromFloat: 0,
    toFloat: 0,
    activeKey: "left",
    modHeld: true,
    stepIdx: 1,
    title: "Left two thirds",
    subtitle: "Press ← again to cycle width",
    holdMs: 580,
  });

  // Beat 3: Left 1/3 (modifier still held)
  pushGlide({
    from: RECTS.leftTwoThirds,
    to: RECTS.leftThird,
    ghost: RECTS.topLeftThird,
    fromFloat: 0,
    toFloat: 0,
    activeKey: "left",
    modHeld: true,
    stepIdx: 2,
    title: "Left third",
    subtitle: "Press ← again for a third",
    holdMs: 620,
  });

  // Beat 4: Compose Top-Left 1/3 corner (modifier STILL held!)
  pushGlide({
    from: RECTS.leftThird,
    to: RECTS.topLeftThird,
    ghost: RECTS.maximize,
    fromFloat: 0,
    toFloat: 0,
    activeKey: "up",
    modHeld: true,
    stepIdx: 3,
    title: "Top-left corner",
    subtitle: "Keep holding, press ↑ — keeps ⅓ width",
    holdMs: 740,
  });

  // Beat 5: Push past Top -> Maximize (ends composition, modifier releases)
  pushGlide({
    from: RECTS.topLeftThird,
    to: RECTS.maximize,
    ghost: RECTS.floating,
    fromFloat: 0,
    toFloat: 0,
    activeKey: "up",
    modHeld: true,
    stepIdx: 4,
    title: "Maximize",
    subtitle: "Press ↑ again to push past and fill screen",
    holdMs: 740,
    releaseModAfterMs: 260,
  });

  // Beat 6: Restore back to original floating window
  pushGlide({
    from: RECTS.maximize,
    to: RECTS.floating,
    ghost: RECTS.leftHalf,
    fromFloat: 0,
    toFloat: 1,
    activeKey: "down",
    modHeld: true,
    stepIdx: 5,
    title: "Restore",
    subtitle: "Press ↓ to return where the window started",
    holdMs: 960,
    releaseModAfterMs: 220,
  });

  return frames;
}

function buildHeroHtml(themeName) {
  const t = THEMES[themeName];
  return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  html, body {
    width: ${HERO_W}px;
    height: ${HERO_H}px;
    background: transparent;
    overflow: hidden;
    font-family: ${FONT_STACK};
    -webkit-font-smoothing: antialiased;
    color: ${t.text};
  }
  .card {
    width: ${HERO_W}px;
    height: ${HERO_H}px;
    border-radius: 10px;
    background: ${t.surface};
    border: 1px solid ${t.border};
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .header {
    height: 44px;
    padding: 0 22px;
    border-bottom: 1px solid ${t.border};
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-shrink: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .brand-name {
    font-size: 13.5px;
    font-weight: 600;
    color: ${t.text};
    letter-spacing: -0.01em;
  }
  .brand-sep {
    color: ${t.textDim};
    font-size: 12px;
  }
  .brand-sub {
    font-size: 12px;
    color: ${t.textDim};
  }
  .dots {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: ${t.border};
  }
  .dot.active {
    width: 18px;
    background: ${t.accent};
  }
  .body {
    flex: 1;
    padding: 16px 22px 18px;
    display: flex;
    align-items: center;
    gap: 28px;
  }
  .stage-wrap {
    width: 384px;
    height: 240px;
    flex-shrink: 0;
    border-radius: 8px;
    background: ${t.surface2};
    border: 1px solid ${t.border};
    position: relative;
    overflow: hidden;
  }
  .stage-inner {
    position: absolute;
    inset: 4px;
  }
  .pane {
    position: absolute;
    border-radius: 5px;
    background: ${t.paneFill};
    border: 1px solid ${t.paneBorder};
    overflow: hidden;
    z-index: 0;
  }
  .pane-bar {
    height: 8px;
    background: ${t.paneBar};
    border-bottom: 1px solid ${t.paneBarBorder};
  }
  .ghost {
    position: absolute;
    border-radius: 5px;
    border: 1.5px dashed ${t.ghostStroke};
    z-index: 1;
    pointer-events: none;
  }
  .panel {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
    height: 240px;
  }
  .step-title {
    font-size: 17px;
    font-weight: 600;
    letter-spacing: -0.015em;
    color: ${t.text};
    line-height: 1.25;
  }
  .step-sub {
    margin-top: 4px;
    font-size: 12.5px;
    color: ${t.textDim};
    line-height: 1.35;
    min-height: 18px;
  }
  .deck {
    margin-top: 20px;
    display: flex;
    align-items: flex-end;
    gap: 18px;
  }
  .mod-col {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .mod-status {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: ${t.textDim};
  }
  .mod-status.held {
    color: ${t.accentTextOnTint};
  }
  .mod-dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: ${t.border};
  }
  .mod-status.held .mod-dot {
    background: ${t.accentTextOnTint};
  }
  .mod-pill {
    width: 158px;
    height: 64px;
    padding: 0 13px;
    border-radius: 7px;
    background: ${t.surface2};
    border: 1px solid ${t.border};
    border-bottom: 2px solid ${t.keyBottom};
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 4px;
  }
  .mod-pill.held {
    background: ${t.accentSoftBg};
    border-color: ${t.accentSoftBorder};
    border-bottom-color: ${t.accent};
    transform: translateY(1px);
  }
  .mod-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .mod-os {
    font-size: 11px;
    color: ${t.textDim};
  }
  .mod-keys {
    font-size: 12.5px;
    font-weight: 600;
    color: ${t.text};
  }
  .mod-pill.held .mod-keys {
    color: ${t.accentTextOnTint};
  }
  .plus {
    font-size: 14px;
    color: ${t.textDim};
    padding-bottom: 22px;
    user-select: none;
  }
  .arrows {
    display: grid;
    grid-template-columns: repeat(3, 36px);
    grid-template-rows: repeat(2, 29px);
    gap: 6px;
  }
  .key {
    border-radius: 6px;
    background: ${t.surface2};
    border: 1px solid ${t.border};
    border-bottom: 2px solid ${t.keyBottom};
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 14px;
    font-weight: 600;
    color: ${t.text};
  }
  .key.active {
    background: ${t.accent};
    border-color: ${t.accent};
    border-bottom-color: ${t.keyActiveBottom};
    color: ${t.accentText};
    transform: translateY(1px);
  }
  .key-up { grid-column: 2; grid-row: 1; }
  .key-left { grid-column: 1; grid-row: 2; }
  .key-down { grid-column: 2; grid-row: 2; }
  .key-right { grid-column: 3; grid-row: 2; }
  .rail {
    margin-top: 22px;
    padding-top: 12px;
    border-top: 1px solid ${t.border};
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: ${t.textDim};
  }
  .rail-item.active {
    color: ${t.accentTextOnTint};
    font-weight: 600;
  }
  .rail-item.done {
    color: ${t.text};
  }
  .rail-dot {
    color: ${t.textDim};
    opacity: 0.45;
  }
</style>
</head>
<body>
  <div class="card">
    <div class="header">
      <div class="brand">
        <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
          <rect x="0" y="0" width="24" height="24" rx="5.5" fill="${t.accent}"/>
          <rect x="4.8" y="4.8" width="6.6" height="14.4" fill="#ffffff"/>
          <rect x="12.6" y="4.8" width="6.6" height="6.6" fill="#ffffff"/>
          <rect x="12.6" y="12.6" width="6.6" height="6.6" fill="#ffffff"/>
        </svg>
        <span class="brand-name">Tile</span>
        <span class="brand-sep">·</span>
        <span class="brand-sub">Default keyboard loop</span>
      </div>
      <div class="dots" id="dots">
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="dot"></span>
      </div>
    </div>
    <div class="body">
      <div class="stage-wrap">
        <div class="stage-inner">
          <div class="pane" id="pane"><div class="pane-bar"></div></div>
          <div class="ghost" id="ghost"></div>
        </div>
      </div>
      <div class="panel">
        <div class="step-title" id="title"></div>
        <div class="step-sub" id="subtitle"></div>
        <div class="deck">
          <div class="mod-col">
            <div class="mod-status" id="modStatus">
              <span class="mod-dot"></span>
              <span id="modStatusText">Modifier</span>
            </div>
            <div class="mod-pill" id="modPill">
              <div class="mod-row"><span class="mod-os">macOS</span><span class="mod-keys">⌃⌥</span></div>
              <div class="mod-row"><span class="mod-os">Windows</span><span class="mod-keys">Win</span></div>
            </div>
          </div>
          <div class="plus">+</div>
          <div class="arrows">
            <div class="key key-up" id="k-up">↑</div>
            <div class="key key-left" id="k-left">←</div>
            <div class="key key-down" id="k-down">↓</div>
            <div class="key key-right" id="k-right">→</div>
          </div>
        </div>
        <div class="rail" id="rail">
          <span class="rail-item" data-idx="0">½</span>
          <span class="rail-dot">·</span>
          <span class="rail-item" data-idx="1">⅔</span>
          <span class="rail-dot">·</span>
          <span class="rail-item" data-idx="2">⅓</span>
          <span class="rail-dot">·</span>
          <span class="rail-item" data-idx="3">Corner</span>
          <span class="rail-dot">·</span>
          <span class="rail-item" data-idx="4">Max</span>
          <span class="rail-dot">·</span>
          <span class="rail-item" data-idx="5">Restore</span>
        </div>
      </div>
    </div>
  </div>
  <script>
    const IS_DARK = ${themeName === "dark" ? "true" : "false"};
    window.renderFrame = function(f) {
      const pane = document.getElementById("pane");
      pane.style.left = (f.pane.x * 100) + "%";
      pane.style.top = (f.pane.y * 100) + "%";
      pane.style.width = (f.pane.w * 100) + "%";
      pane.style.height = (f.pane.h * 100) + "%";

      const m = f.floatMix;
      const yOff = 2 + 5 * m;
      const blur = 4 + 10 * m;
      const spread = -2 - 4 * m;
      const alpha = (IS_DARK ? 0.45 : 0.40) + (IS_DARK ? 0.20 : 0.15) * m;
      pane.style.boxShadow = "0 " + yOff.toFixed(1) + "px " + blur.toFixed(1) + "px " + spread.toFixed(1) + "px rgba(0,0,0," + alpha.toFixed(2) + ")";

      const ghost = document.getElementById("ghost");
      ghost.style.left = (f.ghost.x * 100) + "%";
      ghost.style.top = (f.ghost.y * 100) + "%";
      ghost.style.width = (f.ghost.w * 100) + "%";
      ghost.style.height = (f.ghost.h * 100) + "%";
      ghost.style.opacity = (f.ghostOpacity * 0.55).toFixed(3);

      document.getElementById("title").textContent = f.title;
      document.getElementById("subtitle").textContent = f.subtitle;

      const modPill = document.getElementById("modPill");
      const modStatus = document.getElementById("modStatus");
      const modStatusText = document.getElementById("modStatusText");
      modPill.classList.toggle("held", f.modHeld);
      modStatus.classList.toggle("held", f.modHeld);
      modStatusText.textContent = f.modHeld ? "Held" : "Modifier";

      for (const k of ["up", "left", "down", "right"]) {
        document.getElementById("k-" + k).classList.toggle("active", f.activeKey === k);
      }

      const dots = document.querySelectorAll("#dots .dot");
      dots.forEach((d, idx) => d.classList.toggle("active", idx === f.stepIdx));

      const items = document.querySelectorAll("#rail .rail-item");
      items.forEach((el) => {
        const idx = Number(el.dataset.idx);
        el.classList.toggle("active", idx === f.stepIdx);
        el.classList.toggle("done", f.stepIdx > idx);
      });
    };
  </script>
</body>
</html>`;
}

// ============================================================================
// Cross-platform Chrome/Edge + img2webp Discovery & CDP Driver
// ============================================================================

function findBinary(envVar, candidates) {
  if (process.env[envVar] && fs.existsSync(process.env[envVar])) {
    return process.env[envVar];
  }
  for (const c of candidates) {
    if (path.isAbsolute(c)) {
      if (fs.existsSync(c)) return c;
    } else {
      try {
        const cmd = process.platform === "win32" ? "where" : "which";
        const out = execFileSync(cmd, [c], {
          encoding: "utf8",
          stdio: ["ignore", "pipe", "ignore"],
        }).trim();
        if (out) return out.split(/\r?\n/)[0];
      } catch {
        // continue
      }
    }
  }
  return null;
}

function findChrome() {
  return findBinary("TILE_CHROME", [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    "google-chrome",
    "chromium",
    "chromium-browser",
    "msedge",
  ]);
}

function findImg2Webp() {
  return findBinary("TILE_IMG2WEBP", [
    "img2webp",
    "/opt/homebrew/bin/img2webp",
    "/usr/local/bin/img2webp",
  ]);
}

class CdpSession {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(ev.data.toString());
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message));
        else resolve(msg.result);
      }
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    try {
      this.ws.close();
    } catch {
      // ignore
    }
  }
}

async function launchChromeCdp(chromePath) {
  const userDataDir = fs.mkdtempSync(path.join(os.tmpdir(), "tile-chrome-"));
  const proc = spawn(
    chromePath,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-first-run",
      "--no-default-browser-check",
      "--hide-scrollbars",
      "--remote-debugging-port=0",
      `--user-data-dir=${userDataDir}`,
      "about:blank",
    ],
    { stdio: ["ignore", "pipe", "pipe"] }
  );

  const wsUrl = await new Promise((resolve, reject) => {
    let stderr = "";
    const timer = setTimeout(() => {
      reject(new Error(`Timed out waiting for Chrome DevTools:\n${stderr}`));
    }, 15000);

    proc.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
      const m = stderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);
      if (m) {
        clearTimeout(timer);
        resolve(m[1]);
      }
    });
    proc.on("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`Chrome exited early (${code}):\n${stderr}`));
    });
  });

  // Discover page target
  const port = new URL(wsUrl).port;
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const pageTarget = targets.find((t) => t.type === "page") ?? targets[0];
  if (!pageTarget?.webSocketDebuggerUrl) {
    throw new Error("Could not find Chrome page target");
  }

  const ws = new WebSocket(pageTarget.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });

  const cdp = new CdpSession(ws);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");

  return {
    cdp,
    async dispose() {
      cdp.close();
      proc.kill("SIGTERM");
      await new Promise((r) => setTimeout(r, 150));
      try {
        proc.kill("SIGKILL");
      } catch {
        // already exited
      }
      fs.rmSync(userDataDir, { recursive: true, force: true });
    },
  };
}

async function setViewport(cdp, width, height, deviceScaleFactor = 2) {
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width,
    height,
    deviceScaleFactor,
    mobile: false,
  });
  await cdp.send("Emulation.setDefaultBackgroundColorOverride", {
    color: { r: 0, g: 0, b: 0, a: 0 },
  });
}

async function loadHtml(cdp, html) {
  const { frameTree } = await cdp.send("Page.getFrameTree");
  await cdp.send("Page.setDocumentContent", {
    frameId: frameTree.frame.id,
    html,
  });
  // Wait for two animation frames so fonts and layout settle
  await cdp.send("Runtime.evaluate", {
    expression:
      "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))",
    awaitPromise: true,
  });
}

async function renderHeroWebp(cdp, img2webpPath, themeName, previewDir) {
  const frames = buildHeroFrames();
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), `tile-hero-${themeName}-`));
  try {
    await setViewport(cdp, HERO_W, HERO_H, HERO_DPR);
    await loadHtml(cdp, buildHeroHtml(themeName));

    const img2webpArgs = ["-loop", "0"];
    const keyPreviewIndices = new Set([0, 4, 43, 51, 64, 78]);

    for (let i = 0; i < frames.length; i++) {
      const f = frames[i];
      await cdp.send("Runtime.evaluate", {
        expression: `window.renderFrame(${JSON.stringify(f)}); new Promise(r => requestAnimationFrame(r));`,
        awaitPromise: true,
      });
      const { data } = await cdp.send("Page.captureScreenshot", {
        format: "png",
        fromSurface: true,
      });
      const pngPath = path.join(tmpDir, `frame-${String(i).padStart(3, "0")}.png`);
      const buf = Buffer.from(data, "base64");
      fs.writeFileSync(pngPath, buf);

      if (previewDir && keyPreviewIndices.has(i)) {
        fs.writeFileSync(
          path.join(previewDir, `hero-${themeName}-f${String(i).padStart(3, "0")}.png`),
          buf
        );
      }

      img2webpArgs.push("-d", String(f.duration), "-lossless", "-m", "4", pngPath);
    }

    const outPath = path.join(OUT_DIR, `hero-${themeName}.webp`);
    img2webpArgs.push("-o", outPath);
    execFileSync(img2webpPath, img2webpArgs, { stdio: "pipe" });

    const kb = (fs.statSync(outPath).size / 1024).toFixed(1);
    const totalMs = frames.reduce((s, f) => s + f.duration, 0);
    console.log(
      `  wrote ${path.relative(process.cwd(), outPath)} (${frames.length} frames, ${(totalMs / 1000).toFixed(2)}s, ${kb} KB)`
    );
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

async function renderSvgPreview(cdp, svgPath, previewDir, bg) {
  const svg = fs.readFileSync(svgPath, "utf8");
  const wMatch = svg.match(/width="(\d+)"/);
  const hMatch = svg.match(/height="(\d+)"/);
  const w = Number(wMatch?.[1] ?? 840);
  const h = Number(hMatch?.[1] ?? 420);
  const pad = 16;
  await setViewport(cdp, w + pad * 2, h + pad * 2, 2);
  await loadHtml(
    cdp,
    `<!doctype html><html><body style="margin:0;padding:${pad}px;background:${bg};display:flex;align-items:center;justify-content:center;">${svg}</body></html>`
  );
  const { data } = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
  });
  const base = path.basename(svgPath, ".svg");
  fs.writeFileSync(path.join(previewDir, `${base}.png`), Buffer.from(data, "base64"));
}

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const svgOnly = args.includes("--svg-only");
  const heroOnly = args.includes("--hero-only");
  const previewIdx = args.indexOf("--preview-dir");
  const previewDir = previewIdx >= 0 ? args[previewIdx + 1] : null;
  if (previewDir) fs.mkdirSync(previewDir, { recursive: true });

  const svgFiles = [];

  if (!heroOnly) {
    console.log("Generating static SVGs...");
    const specs = [
      ["size-cycle", renderSizeCycleSvg],
      ["corner-composition", renderCornerCompositionSvg],
      ["display-throws", renderDisplayThrowsSvg],
    ];
    for (const [name, fn] of specs) {
      for (const theme of ["light", "dark"]) {
        const outPath = path.join(OUT_DIR, `${name}-${theme}.svg`);
        fs.writeFileSync(outPath, fn(theme), "utf8");
        svgFiles.push({ path: outPath, theme });
        const kb = (fs.statSync(outPath).size / 1024).toFixed(1);
        console.log(`  wrote ${path.relative(process.cwd(), outPath)} (${kb} KB)`);
      }
    }
  }

  if (!svgOnly || previewDir) {
    const chromePath = findChrome();
    if (!chromePath) {
      throw new Error(
        "Chrome/Chromium/Edge not found. Set TILE_CHROME or run with --svg-only."
      );
    }

    let img2webpPath = null;
    if (!svgOnly) {
      img2webpPath = findImg2Webp();
      if (!img2webpPath) {
        throw new Error(
          "img2webp not found (install WebP tools, e.g. `brew install webp` or `winget install Google.Libwebp`), or set TILE_IMG2WEBP / use --svg-only."
        );
      }
    }

    const browser = await launchChromeCdp(chromePath);
    try {
      if (!svgOnly) {
        console.log("Generating animated WebP heroes...");
        await renderHeroWebp(browser.cdp, img2webpPath, "light", previewDir);
        await renderHeroWebp(browser.cdp, img2webpPath, "dark", previewDir);
      }

      if (previewDir && svgFiles.length > 0) {
        console.log(`Rendering SVG inspection previews into ${previewDir}...`);
        for (const item of svgFiles) {
          const githubBg = item.theme === "dark" ? "#0d1117" : "#ffffff";
          await renderSvgPreview(browser.cdp, item.path, previewDir, githubBg);
        }
      }
    } finally {
      await browser.dispose();
    }
  }
}

main().catch((err) => {
  console.error(err.message || err);
  process.exit(1);
});
