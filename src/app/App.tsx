import { DialogProvider } from "../components/dialogs";
import { Button, Spinner } from "../components/ui";
import { Onboarding } from "../features/settings/Onboarding";
import { useSettings } from "../lib/queries";
import { AppShell } from "./AppShell";
import { NavProvider } from "./nav";

function Gate() {
	const settings = useSettings();

	if (settings.isPending) {
		return (
			<div className="flex h-full items-center justify-center">
				<Spinner />
			</div>
		);
	}
	if (settings.isError) {
		return (
			<div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
				<p role="alert" className="max-w-md text-sm text-problem">
					Chingui couldn't load its settings: {settings.error.message}
				</p>
				<Button onPress={() => settings.refetch()}>Try again</Button>
			</div>
		);
	}
	if (!settings.data.onboarded) return <Onboarding />;

	return (
		<NavProvider>
			<AppShell />
		</NavProvider>
	);
}

export default function App() {
	return (
		<DialogProvider>
			<Gate />
		</DialogProvider>
	);
}
