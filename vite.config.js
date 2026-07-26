import { resolve } from 'node:path';
import { cpSync, existsSync, mkdirSync } from 'node:fs';
import { defineConfig } from 'vite';

function copyExtraStaticAssets() {
  return {
    name: 'copy-extra-static-assets',
    closeBundle() {
      const copies = [
        ['public/assets/3dmodel', 'dist/assets/3dmodel'],
        ['public/assets/js', 'dist/assets/js'],
        ['public/assets/loading.mp4', 'dist/assets/loading.mp4']
      ];

      for (const [from, to] of copies) {
        const source = resolve(__dirname, from);
        const target = resolve(__dirname, to);
        if (!existsSync(source)) continue;
        mkdirSync(resolve(target, '..'), { recursive: true });
        cpSync(source, target, { recursive: true });
      }
    }
  };
}

export default defineConfig({
  root: '.',
  base: './',
  publicDir: false,
  plugins: [copyExtraStaticAssets()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    fs: {
      allow: [resolve(__dirname)]
    }
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      input: {
        index: resolve(__dirname, 'public/index.html'),
        install: resolve(__dirname, 'public/install.html'),
        intro: resolve(__dirname, 'public/intro.html'),
        launcher_update: resolve(__dirname, 'public/launcher_update.html')
      }
    }
  }
});
