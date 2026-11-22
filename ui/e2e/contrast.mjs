// WCAG contrast of a text colour on the backgrounds under it, from the CSS
// colours getComputedStyle gives ("rgb(r, g, b)" / "rgba(r, g, b, a)").

const parse = (c) => {
  const [r, g, b, a = 1] = c.match(/[\d.]+/g).map(Number);
  return [r, g, b, a];
};

/** `top` composited over the opaque colour `under`. */
const over = (top, under) => [...top.slice(0, 3).map((v, i) => v * top[3] + under[i] * (1 - top[3])), 1];

const luminance = (rgb) => {
  const [r, g, b] = rgb.slice(0, 3).map((v) => (v / 255 <= 0.03928 ? v / 255 / 12.92 : ((v / 255 + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

/** Contrast ratio (1 to 21) of text in `fg` on `layers`, the backgrounds
 *  from the text's own element down, the last one opaque. */
export function contrast(fg, layers) {
  const bg = layers.map(parse).reduceRight((under, top) => over(top, under));
  const [a, b] = [luminance(over(parse(fg), bg)), luminance(bg)];
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}
