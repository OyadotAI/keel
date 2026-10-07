// Inline SVG icons. Paths adapted from Lucide (https://lucide.dev, ISC licence). Kept by hand
// rather than through `lucide-react`: two dozen paths are a few KB, a dependency is not.

import type { SVGProps } from "react";

const paths = {
  chat: ["M21 11.5a8 8 0 0 1-8 8H6l-4 3V11.5a9.5 9.5 0 0 1 19 0Z", "M7 10h10", "M7 14h6"],
  "chevron-right": ["M9 18l6-6-6-6"],
  "chevron-down": ["M6 9l6 6 6-6"],
  plus: ["M12 5v14", "M5 12h14"],
  branch: ["M6 3v12", "M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z", "M6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6z", "M18 9a9 9 0 0 1-9 9"],
  x: ["M18 6L6 18", "M6 6l12 12"],
  hand: ["M18 11V6a2 2 0 0 0-4 0v5", "M14 10V4a2 2 0 0 0-4 0v6", "M10 10.5V6a2 2 0 0 0-4 0v8", "M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"],
  "x-circle": ["M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z", "M15 9l-6 6", "M9 9l6 6"],
  "check-circle": ["M22 11.08V12a10 10 0 1 1-5.93-9.14", "M22 4L12 14.01l-3-3"],
  "minus-circle": ["M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z", "M8 12h8"],
  "circle-dashed": ["M10.1 2.18a9.93 9.93 0 0 1 3.8 0", "M17.6 3.71a9.95 9.95 0 0 1 2.69 2.7", "M21.82 10.1a9.93 9.93 0 0 1 0 3.8", "M20.29 17.6a9.95 9.95 0 0 1-2.7 2.69", "M13.9 21.82a9.94 9.94 0 0 1-3.8 0", "M6.4 20.29a9.95 9.95 0 0 1-2.69-2.7", "M2.18 13.9a9.93 9.93 0 0 1 0-3.8", "M3.71 6.4a9.95 9.95 0 0 1 2.7-2.69"],
  octagon: ["M7.86 2h8.28L22 7.86v8.28L16.14 22H7.86L2 16.14V7.86L7.86 2z", "M12 8v4", "M12 16h.01"],
  terminal: ["M4 17l6-6-6-6", "M12 19h8"],
  eye: ["M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7z", "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z"],
  panel: ["M3 4h18v16H3z", "M15 4v16"],
  pencil: ["M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z"],
  users: ["M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2", "M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z", "M22 21v-2a4 4 0 0 0-3-3.87", "M16 3.13a4 4 0 0 1 0 7.75"],
  file: ["M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z", "M14 2v6h6"],
  rewind: ["M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8", "M3 3v5h5"],
  shield: ["M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"],
  "wifi-off": ["M2 2l20 20", "M8.5 16.5a5 5 0 0 1 7 0", "M2 8.82a15 15 0 0 1 4.17-2.65", "M10.66 5c4.01-.36 8.14.9 11.34 3.76", "M16.85 11.25a10 10 0 0 1 2.22 1.68", "M5 13a10 10 0 0 1 5.24-2.76", "M12 20h.01"],
  copy: ["M20 9h-9a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-9a2 2 0 0 0-2-2z", "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"],
  "panel-right": ["M19 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2z", "M15 3v18"],
  "panel-left": ["M19 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2z", "M9 3v18"],
  maximize: ["M15 3h6v6", "M9 21H3v-6", "M21 3l-7 7", "M3 21l7-7"],
  search: ["M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16z", "M21 21l-4.35-4.35"],
  gear: ["M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z", "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"],
  puzzle: ["M19.44 10.98a2.5 2.5 0 1 1 0 3.54L17 17v3a1 1 0 0 1-1 1h-3l-2.46-2.44a2.5 2.5 0 1 0-3.54 0L4.54 21H4a1 1 0 0 1-1-1v-3l2.46-2.46a2.5 2.5 0 1 0 0-3.54L3 10.54V7a1 1 0 0 1 1-1h3l2.44-2.44a2.5 2.5 0 1 1 3.54 0L15.44 6H16a1 1 0 0 1 1 1v3z"],
  folder: ["M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"],
} satisfies Record<string, string[]>;

export type IconName = keyof typeof paths;

export function Icon({ name, size = 16, ...rest }: { name: IconName | "more"; size?: number } & SVGProps<SVGSVGElement>) {
  // Stroke-cap dots are one pixel wide at 14px; the overflow menu needs real dots to be findable.
  if (name === "more")
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden {...rest}>
        <circle cx="5" cy="12" r="2" />
        <circle cx="12" cy="12" r="2" />
        <circle cx="19" cy="12" r="2" />
      </svg>
    );
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden {...rest}>
      {paths[name].map((d) => (
        <path key={d} d={d} />
      ))}
    </svg>
  );
}
