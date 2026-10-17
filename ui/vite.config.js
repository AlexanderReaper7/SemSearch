// One build, two pages: app.html for the Tauri window and vscode.html for
// the webview VS Code shows as a modal. They share every component and
// differ in the backend they pass to Search (src/backend-*.js).
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  // Relative asset URLs, since the webview loads them from the extension's
  // directory rather than from a server root.
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    rollupOptions: { input: { app: 'app.html', vscode: 'vscode.html' } },
  },
  server: { port: 5199, strictPort: true },
});
