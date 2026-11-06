import { IconContext } from "@phosphor-icons/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import ReactDOM from "react-dom/client";
import "katex/dist/katex.min.css";
import "streamdown/styles.css";
import "./styles/index.css";
import App from "./app/App";
import { initTheme } from "./lib/theme";

initTheme();

const queryClient = new QueryClient({
	defaultOptions: {
		queries: { staleTime: 5_000, refetchOnWindowFocus: false, retry: false },
	},
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
	<React.StrictMode>
		<QueryClientProvider client={queryClient}>
			<IconContext.Provider value={{ size: 20, weight: "regular" }}>
				<App />
			</IconContext.Provider>
		</QueryClientProvider>
	</React.StrictMode>,
);
