// Settings destinations keep navigation and labelled content in one stable order.
export const SETTINGS_DESTINATIONS = [
  { id: "trust", label: "Project trust", icon: "shield" },
  { id: "preferences", label: "Preferences", icon: "gear" },
  { id: "claude", label: "Claude Code", icon: "terminal" },
  { id: "license", label: "License & updates", icon: "lock" },
] as const;
