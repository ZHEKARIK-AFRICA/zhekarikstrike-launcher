import { invoke as tauriInvoke } from '@tauri-apps/api/core';

export function normalizePage(page, currentPathname = globalThis.location?.pathname || '') {
    const normalized = String(page || '').replaceAll('\\', '/');
    const prefix = currentPathname.includes('/public/') ? '/public' : '';

    if (normalized.endsWith('intro.html')) return `${prefix}/intro.html`;
    if (normalized.endsWith('install.html')) return `${prefix}/install.html`;
    if (normalized.endsWith('launcher_update.html')) return `${prefix}/launcher_update.html`;
    if (normalized.endsWith('index.html')) return `${prefix}/index.html`;
    return normalized;
}

export function createNavigator({
    invoke = tauriInvoke,
    assign = (page) => globalThis.location.assign(page),
    pathname = globalThis.location?.pathname || ''
} = {}) {
    return async function navigate(page) {
        try {
            await invoke('set_window_layout', { page });
        } catch (error) {
            console.warn('Unable to update window layout before navigation:', error);
        }
        assign(normalizePage(page, pathname));
    };
}

export const navigateToPage = createNavigator();
