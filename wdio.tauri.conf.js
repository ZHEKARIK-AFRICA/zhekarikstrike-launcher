import { resolve } from 'node:path';

export const config = {
    runner: 'local',
    specs: ['./tests/e2e/tauri/**/*.e2e.js'],
    maxInstances: 1,
    capabilities: [{ browserName: 'tauri' }],
    services: [[
        '@wdio/tauri-service',
        {
            appBinaryPath: resolve('src-tauri/target/debug/zhekarikstrike_launcher.exe'),
            driverProvider: 'embedded',
            captureBackendLogs: true,
            captureFrontendLogs: true,
            startTimeout: 60_000,
            commandTimeout: 60_000,
            logLevel: 'error'
        }
    ]],
    framework: 'mocha',
    reporters: ['spec'],
    logLevel: 'error',
    waitforTimeout: 15_000,
    before: async () => {
        try {
            await globalThis.browser.tauri.switchWindow('main');
        } catch {
            // The embedded provider already targets `main`; this call also disables
            // the service's global-Tauri-based auto-focus heuristic.
        }
    },
    mochaOpts: { timeout: 60_000 }
};
