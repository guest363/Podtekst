import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

const isMobilePlatform = Boolean(
  process.env.TAURI_ENV_PLATFORM === 'android' ||
  process.env.TAURI_ENV_PLATFORM === 'ios' ||
  process.env.MOBILE === '1',
)

// Локальное окно Tauri: та же раздача, что раньше (диск → вшитый fallback в Rust).
// Имена детерминированы (без хешей): CDN-кэширования нет, а Rust-манифест и диффы
// остаются статическими. Новым файлам — дописать include_bytes в mr-app/src/main.rs.
export default defineConfig({
  plugins: [svelte()],
  define: {
    __IS_MOBILE_BUILD__: JSON.stringify(isMobilePlatform),
  },
  base: '/',
  build: {
    outDir: '../static',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: 'js/app.js',
        chunkFileNames: 'js/[name].js',
        assetFileNames: (info) => {
          const n = info.names?.[0] ?? ''
          if (n.endsWith('.css')) return 'css/app[extname]'
          return 'assets/[name][extname]'
        },
      },
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:8765',
      '/audio': 'http://127.0.0.1:8765',
    },
  },
})
