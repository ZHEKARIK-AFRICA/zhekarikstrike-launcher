import { spawn } from 'node:child_process';
import { pathToFileURL } from 'node:url';

import { build as buildVite, preview as previewVite } from 'vite';

function runWdio() {
  const isWindows = process.platform === 'win32';
  const executable = isWindows ? process.env.ComSpec || 'cmd.exe' : 'npm';
  const args = isWindows
    ? ['/d', '/s', '/c', 'npm --workspaces=false run wdio:browser']
    : ['--workspaces=false', 'run', 'wdio:browser'];

  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {
      env: process.env,
      stdio: 'inherit',
    });

    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (signal) {
        reject(new Error(`WebDriverIO terminated by ${signal}`));
        return;
      }

      resolve(code ?? 1);
    });
  });
}

export async function runBrowserE2e({
  build = buildVite,
  createPreview = previewVite,
  runTests = runWdio,
} = {}) {
  await build({ clearScreen: false });
  const server = await createPreview({
    clearScreen: false,
    preview: {
      host: '127.0.0.1',
      port: 5173,
      strictPort: true,
    },
  });

  try {
    return await runTests();
  } finally {
    await new Promise((resolve, reject) => {
      server.httpServer.close((error) => error ? reject(error) : resolve());
    });
  }
}

const isMain =
  process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url;

if (isMain) {
  try {
    process.exitCode = await runBrowserE2e();
  } catch (error) {
    console.error(error);
    process.exitCode = 1;
  }
}
