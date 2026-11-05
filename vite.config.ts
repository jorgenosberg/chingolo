// @ts-expect-error type error without @types/node package
import { readdirSync, readFileSync } from "node:fs";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

const host = process.env.TAURI_DEV_HOST;

/**
 * PDF.js loads decoders (wasm), colour profiles, CMaps and standard fonts by URL. Serve them
 * from `/pdfjs/` in dev and copy them into the build, so the app needs no network.
 */
function pdfjsAssets(): Plugin {
	const root = "node_modules/pdfjs-dist";
	const dirs = ["wasm", "cmaps", "standard_fonts", "iccs"];
	const files = new Set<string>();
	for (const dir of dirs) {
		for (const f of readdirSync(`${root}/${dir}`) as string[]) {
			files.add(`${dir}/${f}`);
		}
	}
	return {
		name: "pdfjs-assets",
		configureServer(server) {
			server.middlewares.use("/pdfjs", (req, res, next) => {
				const rel = decodeURIComponent((req.url ?? "").split("?")[0]).replace(
					/^\//,
					"",
				);
				if (!files.has(rel)) return next();
				if (rel.endsWith(".wasm"))
					res.setHeader("Content-Type", "application/wasm");
				res.end(readFileSync(`${root}/${rel}`));
			});
		},
		generateBundle() {
			for (const rel of files) {
				this.emitFile({
					type: "asset",
					fileName: `pdfjs/${rel}`,
					source: readFileSync(`${root}/${rel}`),
				});
			}
		},
	};
}

// https://vite.dev/config/
export default defineConfig(() => ({
	plugins: [react(), tailwindcss(), pdfjsAssets()],

	// PDF.js is already ESM and its viewer expects the core to load first, so skip the
	// dev pre-bundler and serve the files as they are.
	optimizeDeps: {
		exclude: ["pdfjs-dist", "pdfjs-dist/web/pdf_viewer.mjs"],
	},

	// Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
	//
	// 1. prevent Vite from obscuring rust errors
	clearScreen: false,
	// 2. tauri expects a fixed port, fail if that port is not available
	server: {
		port: 1420,
		strictPort: true,
		host: host || false,
		hmr: host
			? {
					protocol: "ws",
					host,
					port: 1421,
				}
			: undefined,
		watch: {
			// 3. tell Vite to ignore watching `src-tauri`
			ignored: ["**/src-tauri/**"],
		},
	},
}));
