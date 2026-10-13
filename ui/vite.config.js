import { defineConfig } from 'vite';
export default defineConfig({
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { outDir: 'dist', target: 'es2022', emptyOutDir: true },
});
