// Settings › Notifications: one switch, and the one thing a person cannot see for themselves —
// that the system has Keel's banners turned off. "Don't allow", clicked once at the first prompt,
// was silent forever and read as the feature being broken.
import { useEffect, useState } from "react";
import { notificationsOn, setNotificationsOn, systemAllows } from "../notify";

export function NotificationSetting() {
  const [on, setOn] = useState(notificationsOn);
  const [allowed, setAllowed] = useState<boolean>();
  useEffect(() => {
    let live = true;
    void systemAllows().then((a) => { if (live) setAllowed(a); });
    return () => { live = false; };
  }, []);
  const mac = navigator.platform.toLowerCase().includes("mac");
  return (
    <section className="settings-section">
      <h2>Notifications</h2>
      <p className="muted">When Keel is not in front: a turn that finished, with what the checks said; a turn that stopped; and an approval or question waiting for you, which also bounces the dock. The dock badge counts the lanes waiting.</p>
      <label className="small" style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <input type="checkbox" checked={on} onChange={(e) => { setOn(e.target.checked); setNotificationsOn(e.target.checked); }} />
        Show notifications
      </label>
      {on && allowed === false && (
        <p className="small warn">
          {mac
            ? "Notifications are turned off for Keel in macOS. Turn them on in System Settings › Notifications › Keel."
            : "Notifications are turned off for Keel in Windows. Turn them on in Settings › System › Notifications."}
        </p>
      )}
    </section>
  );
}
