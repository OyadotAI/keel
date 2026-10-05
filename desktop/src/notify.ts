// A banner when the window is not in front and something needs the person or has finished.
// An approval nobody sees is a turn stuck for four minutes and then refused; a turn that ended
// while you were in another app is the reason you switched away.
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";

let allowed: boolean | undefined;

async function permitted(): Promise<boolean> {
  if (allowed !== undefined) return allowed;
  try {
    allowed = (await isPermissionGranted()) || (await requestPermission()) === "granted";
  } catch {
    allowed = false;
  }
  return allowed;
}

/// Only when the window is not the one in front: a banner about what you are looking at is noise.
export async function notify(title: string, body: string) {
  if (document.hasFocus() || !(await permitted())) return;
  try {
    sendNotification({ title, body });
  } catch {
    // A missed banner is not worth a failure on screen.
  }
}
