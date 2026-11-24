// Settings › Display: the window's zoom. Oya Factory is dark only, so there is no theme to pick.
import { WindowZoom } from "./WindowZoom";

export function AppearanceSetting() {
  return <section className="settings-section">
    <h2>Display</h2>
    <p className="muted">Zoom the whole window. ⌘+ and ⌘− do the same from anywhere.</p>
    <div className="appearance-zoom"><WindowZoom /></div>
  </section>;
}
