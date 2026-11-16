import { useSyncExternalStore } from "react";

/** Switches for the Developer tab in Settings. Kept on this Mac, per browser profile. */
const KEY = "chingui.dev.layoutOverlay";
const listeners = new Set<() => void>();

function read(): boolean {
	try {
		return localStorage.getItem(KEY) === "1";
	} catch {
		return false;
	}
}

let layoutOverlay = read();

export function setLayoutOverlay(on: boolean) {
	layoutOverlay = on;
	try {
		localStorage.setItem(KEY, on ? "1" : "0");
	} catch {
		// a convenience only
	}
	for (const l of listeners) l();
}

/** Draw the layout model's regions over pages in the reader. */
export function useLayoutOverlay(): boolean {
	return useSyncExternalStore(
		(cb) => {
			listeners.add(cb);
			return () => listeners.delete(cb);
		},
		() => layoutOverlay,
	);
}
