import { setAppearance, useAppearance } from "../appearance";
import { Icon } from "./icons";

export function AppearanceSetting() {
  const { mode } = useAppearance();
  return <section className="settings-section">
    <h2>Appearance</h2>
    <p className="muted">Choose a theme, or follow your system’s appearance.</p>
    <div className="appearance-options" role="radiogroup" aria-label="Appearance">
      {([ ["system", "System", "monitor"], ["light", "Light", "sun"], ["dark", "Dark", "moon"] ] as const).map(([value, title, icon]) => <label key={value} className={mode === value ? "selected" : ""}>
        <input type="radio" name="appearance" value={value} checked={mode === value} onChange={() => setAppearance(value)} /><Icon name={icon} size={17} /><span>{title}</span>
      </label>)}
    </div>
  </section>;
}
