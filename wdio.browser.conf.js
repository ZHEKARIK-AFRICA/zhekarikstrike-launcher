export const config = {
    runner: 'local',
    specs: ['./tests/e2e/browser/**/*.e2e.js'],
    maxInstances: 1,
    capabilities: [{ browserName: 'tauri' }],
    services: [[
        '@wdio/tauri-service',
        {
            mode: 'browser',
            devServerUrl: 'http://127.0.0.1:5173/public/intro.html',
            logLevel: 'error'
        }
    ]],
    framework: 'mocha',
    reporters: ['spec'],
    logLevel: 'error',
    waitforTimeout: 10_000,
    mochaOpts: { timeout: 30_000 }
};
