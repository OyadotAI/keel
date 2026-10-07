/** Keel's two offset planes: a compact mark that stays clear at toolbar size. */
export function BrandMark({ size = 22 }: { size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
    <path d="M5 4h5v7l8-7v6l-8 7v3H5V4Z" fill="currentColor" />
    <path d="m13 15 5-4v9h-5v-5Z" fill="currentColor" opacity=".5" />
  </svg>;
}
