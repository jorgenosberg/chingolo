import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Checkbox selection over an ordered list: shift-click ranges, select all, Escape clears.
 * `resetKey` changes when the list's scope or filters change, since a selection belongs to the
 * list it was made in.
 */
export function useSelection(ids: string[], resetKey: string) {
	const [selected, setSelected] = useState<Set<string>>(new Set());
	const anchor = useRef<string | null>(null);

	const clear = useCallback(() => {
		setSelected((prev) => (prev.size === 0 ? prev : new Set()));
		anchor.current = null;
	}, []);

	// biome-ignore lint/correctness/useExhaustiveDependencies: clears when the list changes
	useEffect(clear, [resetKey]);

	const count = selected.size;
	useEffect(() => {
		if (count === 0) return;
		const onKey = (e: KeyboardEvent) => {
			const typing =
				e.target instanceof HTMLInputElement && e.target.type !== "checkbox";
			if (e.key === "Escape" && !typing) clear();
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [count, clear]);

	/** `range` is set when shift was held. */
	const select = (id: string, on: boolean, range: boolean) => {
		setSelected((prev) => {
			const next = new Set(prev);
			const from = anchor.current ? ids.indexOf(anchor.current) : -1;
			const to = ids.indexOf(id);
			const picked =
				range && from >= 0 && to >= 0
					? ids.slice(Math.min(from, to), Math.max(from, to) + 1)
					: [id];
			for (const i of picked) {
				if (on) next.add(i);
				else next.delete(i);
			}
			return next;
		});
		anchor.current = id;
	};

	const selectAll = (on: boolean) => {
		setSelected(on ? new Set(ids) : new Set());
		anchor.current = null;
	};

	return { selected, select, selectAll, clear };
}
