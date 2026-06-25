import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { viteSingleFile } from 'vite-plugin-singlefile'
import { fileURLToPath } from 'url'
import path from 'path'
const __dirname = fileURLToPath(new URL('.', import.meta.url))
export default defineConfig({
  base: './',
  plugins: [react(), viteSingleFile()],
  build: { target: 'es2020', minify: 'terser', sourcemap: false, assetsInlineLimit: 100000000 },
  resolve: { alias: { '@': path.resolve(__dirname, 'src') } },
})
