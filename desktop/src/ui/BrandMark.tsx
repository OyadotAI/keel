/** Oya's overlapping rings keep Factory recognizable beside the rest of the product family. */
/** Keep the shared mark's geometry stable while its foreground follows the active theme. */
export function BrandMark({ size = 22 }: { size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 512 512" fill="none" aria-hidden="true">
    <circle cx="276" cy="276" r="160" stroke="var(--lime)" strokeWidth="80" />
    <circle cx="236" cy="236" r="160" stroke="var(--text)" strokeWidth="80" />
  </svg>;
}
