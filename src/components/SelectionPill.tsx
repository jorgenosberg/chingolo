import { BookmarkSimpleIcon, CheckIcon } from "@phosphor-icons/react";
import { type RefObject, useEffect, useRef, useState } from "react";
import { cx, POPOVER } from "./ui";

type Pos = { top: number; left: number };

/**
 * A small "Save" button above text the user selected inside `containerRef`. It shows when the
 * mouse is released over a selection, and goes away when the selection is cleared, on scroll,
 * on Escape or after saving. `onSave` gets a copy of the selected range.
 */
export function SelectionPill({
	containerRef,
	onSave,
	label = "Save",
}: {
	containerRef: RefObject<HTMLElement | null>;
	onSave: (range: Range) => Promise<unknown>;
	label?: string;
}) {
	const [pos, setPos] = useState<Pos | null>(null);
	const [state, setState] = useState<"idle" | "saving" | "done" | "error">(
		"idle",
	);
	const range = useRef<Range | null>(null);

	useEffect(() => {
		const show = () =>
			requestAnimationFrame(() => {
				const el = containerRef.current;
				const sel = window.getSelection();
				if (!el || !sel || sel.isCollapsed || sel.rangeCount === 0) return;
				if (!sel.toString().trim()) return;
				const r = sel.getRangeAt(0);
				if (!el.contains(r.commonAncestorContainer)) return;
				const box = r.getBoundingClientRect();
				range.current = r.cloneRange();
				setState("idle");
				setPos({
					top: Math.max(8, box.top - 40),
					left: Math.min(
						Math.max(box.left + box.width / 2, 60),
						window.innerWidth - 60,
					),
				});
			});
		const hide = () => setPos(null);
		const onSelectionChange = () => {
			if (window.getSelection()?.isCollapsed) setPos(null);
		};
		const onKey = (e: KeyboardEvent) => e.key === "Escape" && hide();
		document.addEventListener("mouseup", show);
		document.addEventListener("selectionchange", onSelectionChange);
		document.addEventListener("keydown", onKey);
		window.addEventListener("scroll", hide, true);
		window.addEventListener("resize", hide);
		return () => {
			document.removeEventListener("mouseup", show);
			document.removeEventListener("selectionchange", onSelectionChange);
			document.removeEventListener("keydown", onKey);
			window.removeEventListener("scroll", hide, true);
			window.removeEventListener("resize", hide);
		};
	}, [containerRef]);

	if (!pos) return null;
	return (
		<button
			type="button"
			// Keep the selection: pressing a button would otherwise clear it first.
			onMouseDown={(e) => e.preventDefault()}
			disabled={state === "saving"}
			onClick={async () => {
				const r = range.current;
				if (!r) return;
				setState("saving");
				try {
					await onSave(r);
					setState("done");
					setTimeout(() => {
						setPos(null);
						window.getSelection()?.removeAllRanges();
					}, 900);
				} catch {
					setState("error");
				}
			}}
			style={{ top: pos.top, left: pos.left }}
			className={cx(
				"fixed z-40 flex h-8 -translate-x-1/2 items-center gap-1.5 px-2.5 text-[13px] font-medium",
				POPOVER,
				state === "error" && "text-problem",
			)}
		>
			{state === "done" ? (
				<CheckIcon className="size-4" />
			) : (
				<BookmarkSimpleIcon className="size-4" />
			)}
			{state === "done" ? "Saved" : state === "error" ? "Couldn't save" : label}
		</button>
	);
}
