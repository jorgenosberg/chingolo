import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "../components/ui";
import { api } from "../lib/api";

/** Last stop for a render error. Reports it (if crash reports are on) and offers a reload. */
export class ErrorBoundary extends Component<
	{ children: ReactNode },
	{ failed: boolean }
> {
	state = { failed: false };

	static getDerivedStateFromError() {
		return { failed: true };
	}

	componentDidCatch(error: Error, info: ErrorInfo) {
		api.reportFrontendError(
			error.message,
			`${error.stack ?? ""}${info.componentStack ?? ""}`,
		);
	}

	render() {
		if (!this.state.failed) return this.props.children;
		return (
			<div className="flex h-screen flex-col items-center justify-center gap-3 bg-background p-6 text-center text-foreground">
				<p className="text-sm font-medium">Something went wrong.</p>
				<p className="max-w-sm text-sm text-muted-foreground">
					Your chats and library are safe. Reload to carry on.
				</p>
				<Button variant="primary" onPress={() => window.location.reload()}>
					Reload
				</Button>
			</div>
		);
	}
}
