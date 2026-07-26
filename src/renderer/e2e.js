const nativePluginReady = import.meta.env.VITE_TAURI_E2E === 'native'
    ? import('@wdio/tauri-plugin')
    : Promise.resolve();

export async function waitForE2eReady() {
    const mode = import.meta.env.VITE_TAURI_E2E;
    if (mode !== 'browser' && mode !== 'native') return;
    await nativePluginReady;
    while (globalThis.window?.__ZHEKARIK_E2E_READY__ !== true) {
        await new Promise((resolve) => setTimeout(resolve, 10));
    }
}
