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
    emptyOutDir: false, // preserve favicon.svg, icons.svg
    rollupOptions: {
      output: {
        // Split the large vendor bundles so the editor's CodeMirror and the
        // DAG viewer don't block the initial dashboard load.
        manualChunks(id: string) {
          if (!id.includes('node_modules')) return undefined;
          const pkg = /node_modules\/((?:@[^/]+\/)?[^/]+)/.exec(id)?.[1] ?? '';
          if (['react', 'react-dom', 'react-router', 'react-router-dom', 'scheduler'].includes(pkg)) {
            return 'react';
          }
          if (pkg.startsWith('@xyflow/') || pkg === 'd3-dag') return 'dag';
          if (pkg === 'codemirror' || pkg.startsWith('@codemirror/') || pkg.startsWith('@lezer/')) {
            return 'codemirror';
          }
          return 'vendor';
        },
      },
    },
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
