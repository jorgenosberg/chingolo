import {
	CaretDownIcon,
	CaretUpIcon,
	MagnifyingGlassIcon,
	XIcon,
} from "@phosphor-icons/react";
import {
	type RefObject,
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react";
import { IconButton } from "../../components/ui";

const canHighlight = typeof CSS !== "undefined" && "highlights" in CSS;

/** Every case-insensitive hit of `q` inside a single text node under `root`. */
function collect(root: HTMLElement, q: string): Range[] {
	const needle = q.toLowerCase();
	const found: Range[] = [];
	const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
	for (let n = walker.nextNode(); n; n = walker.nextNode()) {
		// Button labels and the like aren't message text.
		if (n.parentElement?.closest("button")) continue;
		const hay = (n.nodeValue ?? "").toLowerCase();
		for (
			let i = hay.indexOf(needle);
			i !== -1;
			i = hay.indexOf(needle, i + needle.length)
		) {
			const r = new Range();
			r.setStart(n, i);
			r.setEnd(n, i + needle.length);
			found.push(r);
		}
	}
	return found;
}

// One Highlight object per name, emptied with clear() before every change. Some WebKit builds
// (bug 306396, fixed upstream in January 2026) don't repaint the old marks when a highlight is
// deleted from or replaced in `CSS.highlights`, so stale marks stay on screen. Clearing the
// Highlight's own ranges repaints them, whatever the system WebKit does.
const matches = canHighlight ? new Highlight() : null;
const currentMatch = canHighlight ? new Highlight() : null;
if (currentMatch) currentMatch.priority = 1; // drawn over the plain matches

function clearHighlights() {
	if (!matches || !currentMatch) {
		// The fallback marks the current match as the selection.
		window.getSelection()?.removeAllRanges();
		return;
	}
	matches.clear();
	currentMatch.clear();
	CSS.highlights.delete("find");
	CSS.highlights.delete("find-current");
}

function paint(ranges: Range[], current: number) {
	if (!ranges.length) return clearHighlights();
	if (matches && currentMatch) {
		matches.clear();
		currentMatch.clear();
		for (const r of ranges) matches.add(r);
		currentMatch.add(ranges[current]);
		if (!CSS.highlights.has("find")) {
			CSS.highlights.set("find", matches);
			CSS.highlights.set("find-current", currentMatch);
		}
	} else {
		// Older WebKit: show the current match as the selection.
		const sel = window.getSelection();
		sel?.removeAllRanges();
		sel?.addRange(ranges[current]);
	}
}

/**
 * Find in the open chat (⌘F). Searches the text on screen under `root`, which is the message
 * scroller, so it sees what Streamdown rendered, not the markdown source.
 */
export function FindBar({
	root,
	focusTick,
	onClose,
	expandEarlier,
}: {
	root: RefObject<HTMLElement | null>;
	/** Changes when ⌘F is pressed again, to put the cursor back in the field. */
	focusTick: number;
	onClose: () => void;
	/** Shows a fork's collapsed messages so they are searched too. */
	expandEarlier: () => void;
}) {
	const input = useRef<HTMLInputElement>(null);
	const query = useRef("");
	const ranges = useRef<Range[]>([]);
	const index = useRef(0);
	const [text, setText] = useState("");
	const [stat, setStat] = useState({ total: 0, current: 0 });

	const show = useCallback((scroll: boolean) => {
		const list = ranges.current;
		paint(list, index.current);
		setStat({
			total: list.length,
			current: list.length ? index.current + 1 : 0,
		});
		if (scroll) {
			list[index.current]?.startContainer.parentElement?.scrollIntoView({
				block: "center",
			});
		}
	}, []);

	// `keep` is for re-runs caused by new text: stay on the current match, don't scroll.
	const search = useCallback(
		(q: string, keep: boolean) => {
			const el = root.current;
			ranges.current = el && q ? collect(el, q) : [];
			const last = Math.max(ranges.current.length - 1, 0);
			index.current = keep ? Math.min(index.current, last) : 0;
			show(!keep);
		},
		[root, show],
	);

	const step = (d: 1 | -1) => {
		const n = ranges.current.length;
		if (!n) return;
		index.current = (index.current + d + n) % n;
		show(true);
	};

	// biome-ignore lint/correctness/useExhaustiveDependencies: refocus when the tick changes
	useEffect(() => {
		input.current?.focus();
		input.current?.select();
	}, [focusTick]);

	// New or changed messages, including a reply that is streaming. Throttled, not debounced, so
	// the count keeps moving while text arrives.
	useEffect(() => {
		const el = root.current;
		if (!el) return;
		let timer: number | undefined;
		const obs = new MutationObserver(() => {
			if (timer !== undefined) return;
			timer = window.setTimeout(() => {
				timer = undefined;
				if (query.current) search(query.current, true);
			}, 200);
		});
		obs.observe(el, { childList: true, subtree: true, characterData: true });
		return () => {
			obs.disconnect();
			window.clearTimeout(timer);
		};
	}, [root, search]);

	useEffect(() => clearHighlights, []);

	const noMatches = text && stat.total === 0;

	return (
		<form
			role="search"
			className="flex shrink-0 items-center gap-1.5 border-b border-border px-5 py-2"
			onSubmit={(e) => e.preventDefault()}
			// On the form, so Escape closes the bar from the buttons as well as the field.
			onKeyDown={(e) => {
				if (e.key !== "Escape" || e.nativeEvent.isComposing) return;
				e.preventDefault();
				onClose();
			}}
		>
			<div className="relative min-w-0 flex-1">
				<MagnifyingGlassIcon className="pointer-events-none absolute left-2 top-1.5 size-4 text-muted-foreground" />
				<input
					ref={input}
					type="search"
					aria-label="Find in conversation"
					placeholder="Find in conversation"
					value={text}
					onChange={(e) => {
						const q = e.target.value;
						setText(q);
						query.current = q;
						if (q) expandEarlier();
						search(q, false);
					}}
					onKeyDown={(e) => {
						if (e.nativeEvent.isComposing) return;
						if (e.key === "Enter") {
							e.preventDefault();
							step(e.shiftKey ? -1 : 1);
						}
					}}
					className="h-7 w-full rounded-md border border-transparent bg-muted pr-2 pl-7 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50"
				/>
			</div>
			{text && (
				<span
					aria-live="polite"
					className={`shrink-0 text-xs ${noMatches ? "text-problem" : "text-muted-foreground"}`}
				>
					{noMatches ? "No matches" : `${stat.current} of ${stat.total}`}
				</span>
			)}
			<IconButton
				label="Previous match"
				isDisabled={!stat.total}
				onPress={() => step(-1)}
			>
				<CaretUpIcon />
			</IconButton>
			<IconButton
				label="Next match"
				isDisabled={!stat.total}
				onPress={() => step(1)}
			>
				<CaretDownIcon />
			</IconButton>
			<IconButton label="Close find" onPress={onClose}>
				<XIcon />
			</IconButton>
		</form>
	);
}
