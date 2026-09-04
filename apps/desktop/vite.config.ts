import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

// The Tauri shell loads the dev server from a fixed port and the
// built assets from ../dist (tauri.conf.json).
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_'],
  server: {
    port: 1420,
    strictPort: true,
  },
  test: {
    environment: 'jsdom',
  },
})
