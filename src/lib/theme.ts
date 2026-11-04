import { useSyncExternalStore } from "react";

export type ThemePref = "system" | "light" | "dark";

const KEY = "chingui.theme";
const media = window.matchMedia("(prefers-color-scheme: dark)");
const listeners = new Set<() => void>();

function read(): ThemePref {
	try {
		const v = localStorage.getItem(KEY);
		if (v === "light" || v === "dark" || v === "system") return v;
	} catch {
		// storage can be unavailable; fall back to system
	}
	return "system";
}

let pref = read();

function apply() {
	const dark = pref === "dark" || (pref === "system" && media.matches);
	document.documentElement.dataset.theme = dark ? "dark" : "light";
}

export function initTheme() {
	apply();
	media.addEventListener("change", apply);
}

export function setTheme(next: ThemePref) {
	pref = next;
	try {
		localStorage.setItem(KEY, next);
	} catch {
		// ignore
	}
	apply();
	for (const l of listeners) l();
}

export function useTheme(): ThemePref {
	return useSyncExternalStore(
		(cb) => {
			listeners.add(cb);
			return () => listeners.delete(cb);
		},
		() => pref,
	);
}
