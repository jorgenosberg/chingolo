import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button, Spinner } from "../../components/ui";
import { api } from "../../lib/api";

/** Checks once at start. Stays hidden when offline or up to date. */
export function UpdateBar() {
	const [dismissed, setDismissed] = useState(false);
	const [installing, setInstalling] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const update = useQuery({
		queryKey: ["update"],
		queryFn: api.checkForUpdate,
		staleTime: Number.POSITIVE_INFINITY,
		retry: false,
		refetchOnWindowFocus: false,
	});

	if (dismissed || !update.data) return null;

	const install = async () => {
		setInstalling(true);
		setError(null);
		try {
			await api.installUpdate();
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
			setInstalling(false);
		}
	};

	return (
		<div
			role="status"
			className="fixed bottom-4 left-1/2 z-40 flex -translate-x-1/2 items-center gap-3 rounded-lg border border-border bg-background px-4 py-2.5 text-sm shadow-lg"
		>
			{installing ? (
				<>
					<Spinner />
					<span>
						Installing Chingolo {update.data.version}, then restarting.
					</span>
				</>
			) : (
				<>
					<span>
						{error
							? `Update failed: ${error}`
							: `Chingolo ${update.data.version} is available.`}
					</span>
					<Button variant="primary" onPress={install}>
						{error ? "Try again" : "Update and restart"}
					</Button>
					<Button variant="ghost" onPress={() => setDismissed(true)}>
						Later
					</Button>
				</>
			)}
		</div>
	);
}
