// System Back (the Android button / gesture).
//
// Overlays such as the history drawer register a handler while they are open, and Back closes the
// topmost one. With nothing open, Back leaves Settings for the chat, and on the chat itself it
// exits the app. Overlays deliberately do not add browser-history entries: mixing those with
// SvelteKit's own navigation left the router showing the wrong page after Back.

import { goto } from "$app/navigation";
import { page } from "$app/state";
import { invoke } from "@tauri-apps/api/core";
import { onBackButtonPress } from "@tauri-apps/api/app";

const handlers: Array<() => void> = [];

/** Registers `handler` as the thing Back does while it is on top. Returns the unregister function. */
export function pushBackHandler(handler: () => void): () => void {
  handlers.push(handler);
  return () => {
    const i = handlers.lastIndexOf(handler);
    if (i >= 0) handlers.splice(i, 1);
  };
}

let cameFromApp = false;
/** Called after every navigation, so `navigateBack` knows whether there is a page to go back to. */
export function noteNavigation(fromInApp: boolean) {
  cameFromApp = fromInApp;
}

/** Back to the previous page, without stacking a new entry on top of the history. */
export function navigateBack() {
  if (cameFromApp) history.back();
  else goto("/", { replaceState: true });
}

function onBack() {
  const top = handlers[handlers.length - 1];
  if (top) top();
  else if (page.url.pathname !== "/") navigateBack();
  else void invoke("exit_app");
}

/** Takes over the Android Back button. Does nothing on other platforms. Returns a cleanup function. */
export async function installBackButton(): Promise<() => void> {
  if (!/android/i.test(navigator.userAgent)) return () => {};
  try {
    const listener = await onBackButtonPress(onBack);
    return () => void listener.unregister();
  } catch (e) {
    console.warn("Could not take over the Back button:", e);
    return () => {};
  }
}
