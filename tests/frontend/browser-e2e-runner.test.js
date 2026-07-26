import { describe, expect, it } from 'vitest';

let runBrowserE2e;

try {
  ({ runBrowserE2e } = await import('../../scripts/browser-e2e-runner.mjs'));
} catch (error) {
  const missingRunner =
    error?.code === 'ERR_MODULE_NOT_FOUND' &&
    String(error?.message).includes('/scripts/browser-e2e-runner.mjs');

  if (!missingRunner) {
    throw error;
  }
}

describe('runBrowserE2e', () => {
  it('closes the Vite server after successful browser tests', async () => {
    expect(typeof runBrowserE2e).toBe('function');

    const events = [];
    const exitCode = await runBrowserE2e({
      createServer: async () => ({
        listen: async () => events.push('listen'),
        close: async () => events.push('close'),
      }),
      runTests: async () => {
        events.push('test');
        return 0;
      },
    });

    expect({ events, exitCode }).toEqual({
      events: ['listen', 'test', 'close'],
      exitCode: 0,
    });
  });
});
