// Source Control card sizing separates the saved preference from constrained displayed geometry.
export const CARD_SPLIT_DEFAULT = 0.72;
export const CARD_SPLIT_MIN = 0;
export const CARD_SPLIT_MAX = 1;
export const CARD_HANDLE_HEIGHT = 10;
export const CARD_GAP = 6;
export const CARD_HEADER_MIN = 32;
export const CHANGES_MIN_HEIGHT = 170;
export const GRAPH_MIN_HEIGHT = 100;
export const CARD_KEY_STEP = 0.02;
export const CARD_KEY_FAST_STEP = 0.1;

/** Invalid preferences must not make either card disappear after restarting. */
export function cardPreference(value: unknown): number {
  const ratio = typeof value === "number" ? value : typeof value === "string" && value.trim() ? Number(value) : NaN;
  return Number.isFinite(ratio) && ratio >= CARD_SPLIT_MIN && ratio <= CARD_SPLIT_MAX ? ratio : CARD_SPLIT_DEFAULT;
}

/** Short windows shrink content before headers, then scroll rather than producing negative sizes. */
export function cardLayout(height: number, preferred: number, upperHeader: number, lowerHeader: number, upperOpen: boolean, lowerOpen: boolean) {
  const upperMin = Math.max(CARD_HEADER_MIN, upperHeader), lowerMin = Math.max(CARD_HEADER_MIN, lowerHeader);
  const separator = upperOpen && lowerOpen ? CARD_HANDLE_HEIGHT : CARD_GAP;
  const usable = Math.max(upperMin + lowerMin, Math.max(0, height) - separator);
  if (!upperOpen || !lowerOpen) return { upper: upperOpen ? usable - lowerMin : upperMin, lower: lowerOpen ? usable - upperMin : lowerMin, separator, usable, minimum: 0, maximum: 1 };
  const normal = usable >= Math.max(CHANGES_MIN_HEIGHT, upperMin) + Math.max(GRAPH_MIN_HEIGHT, lowerMin);
  const minUpper = normal ? Math.max(CHANGES_MIN_HEIGHT, upperMin) : upperMin;
  const minLower = normal ? Math.max(GRAPH_MIN_HEIGHT, lowerMin) : lowerMin;
  const upper = Math.min(usable - minLower, Math.max(minUpper, usable * cardPreference(preferred)));
  return { upper, lower: usable - upper, separator, usable, minimum: minUpper / usable, maximum: (usable - minLower) / usable };
}
