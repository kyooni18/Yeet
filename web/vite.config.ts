import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'

export default defineConfig({
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  server: {
    proxy: {
      '/api': {
        target: process.env.YEET_REMOTE_TARGET ?? 'http://127.0.0.1:7331',
        // Keep the browser Host/Origin intact. Remote authentication is origin-bound,
        // so rewriting these headers makes an otherwise same-origin dev proxy fail closed.
        changeOrigin: false,
        ws: true,
        rewriteWsOrigin: false,
      },
    },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    emptyOutDir: true,
    sourcemap: false,
    target: 'es2022',
  },
})
