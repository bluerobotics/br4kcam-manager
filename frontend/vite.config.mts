// Plugins
import tailwindcss from '@tailwindcss/vite'
import Components from 'unplugin-vue-components/vite'
import Vue from '@vitejs/plugin-vue'

// Utilities
import { defineConfig } from 'vite'
import { fileURLToPath, URL } from 'node:url'

// https://vitejs.dev/config/
export default defineConfig({
  base: './',
  plugins: [
    Vue(),
    tailwindcss(),
    Components(),
  ],
  define: { 'process.env': {} },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
    // A linked package brings its own copy of Vue, and two Vue runtimes in one page break every
    // injection a component library relies on. The app's copy is the only one.
    dedupe: ['vue'],
    extensions: [
      '.js',
      '.json',
      '.jsx',
      '.mjs',
      '.ts',
      '.tsx',
      '.vue',
    ],
  },
  server: {
    port: 3000,
    cors: {
      origin: '*',
    },
  },
})
