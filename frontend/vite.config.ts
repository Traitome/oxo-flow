import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  // Relative asset URLs: the app can be mounted under any base path
  // (--base-path /oxoflow) without rebuilding — the server injects a
  // <base> tag + window.__OXO_BASE__ into index.html at serve time
  // (issue #79 deployment modes).
  base: './',
  build: {
    // Output directly into the Rust crate's static directory
    outDir: '../crates/oxo-flow-web/static',
    emptyOutDir: false, // preserve favicon.svg, icons.svg, openapi.json
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      },
    },
  },
})
