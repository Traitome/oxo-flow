import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
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
