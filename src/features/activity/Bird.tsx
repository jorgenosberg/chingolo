import bird from "../../assets/mascot/bird.svg";
import reading from "../../assets/mascot/reading.svg";
import { cx } from "../../components/ui";

export type BirdMood = "busy" | "waiting" | "still";

/**
 * The mascot, perched on the edge of the list. It hops while work runs and sits still otherwise.
 * It is an image, so it moves as a whole (see `.bird-bob` in index.css, which stops for reduced
 * motion). A red dot says something needs attention.
 */
export function Bird({ mood, problem }: { mood: BirdMood; problem?: boolean }) {
	return (
		<span
			className="relative -mb-1.5 mr-1 inline-block shrink-0"
			aria-hidden="true"
		>
			<img
				src={bird}
				alt=""
				draggable={false}
				className={cx(
					"size-16 select-none",
					mood === "busy" && "bird-bob",
					mood === "waiting" && "bird-bob bird-bob-slow",
				)}
			/>
			{problem && (
				<span className="absolute right-2 top-2 size-2.5 rounded-full bg-problem ring-2 ring-background" />
			)}
		</span>
	);
}

/** For when nothing is running: the bird with its books, standing still. */
export function ReadingBird() {
	return (
		<img
			src={reading}
			alt=""
			aria-hidden="true"
			draggable={false}
			className="h-28 w-auto select-none"
		/>
	);
}
