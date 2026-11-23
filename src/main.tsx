import { IconContext } from "@phosphor-icons/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import ReactDOM from "react-dom/client";
import "katex/dist/katex.min.css";
import "streamdown/styles.css";
import "./styles/index.css";
import App from "./app/App";
import { ErrorBoundary } from "./app/ErrorBoundary";
import { api } from "./lib/api";
import { initTheme } from "./lib/theme";

initTheme();

// Sent to the reporting server only if the user turned crash reports on.
window.addEventListener("error", (e) => {
	api.reportFrontendError(
		e.message,
		e.error instanceof Error ? e.error.stack : null,
		e.filename ? `${e.filename}:${e.lineno}:${e.colno}` : null,
	);
});
window.addEventListener("unhandledrejection", (e) => {
	const r = e.reason;
	api.reportFrontendError(
		r instanceof Error ? r.message : String(r),
		r instanceof Error ? r.stack : null,
	);
});

const queryClient = new QueryClient({
	defaultOptions: {
		queries: { staleTime: 5_000, refetchOnWindowFocus: false, retry: false },
	},
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
	<React.StrictMode>
		<QueryClientProvider client={queryClient}>
			<IconContext.Provider value={{ size: 20, weight: "regular" }}>
				<ErrorBoundary>
					<App />
				</ErrorBoundary>
			</IconContext.Provider>
		</QueryClientProvider>
	</React.StrictMode>,
);
