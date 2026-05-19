import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	build: {
		// Bundles load from local disk in the Tauri WebView — the default 500 kB
		// chunk warning is calibrated for cold network fetches and doesn't
		// apply. Lazy-loaded chunks (e.g. Shiki) can still push past this
		// without us caring.
		chunkSizeWarningLimit: 1024
	}
});
