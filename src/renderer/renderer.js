import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { setupInputContextMenu } from './context-menu.js';

async function runWindowAction(action, label) {
    try {
        await action();
    } catch (error) {
        console.error(`Failed to ${label} window:`, error);
    }
}

document.addEventListener('DOMContentLoaded', () => {
    setupInputContextMenu();

    document.addEventListener('click', async (event) => {
        const target = event.target.closest('a');
        if (!target?.href || !/^https?:\/\//.test(target.href)) return;
        event.preventDefault();
        try {
            await invoke('open_external_url', { url: target.href });
        } catch (error) {
            console.error('Failed to open external link:', error);
        }
    });

    document.getElementById('close-window')?.addEventListener('click', () => {
        void runWindowAction(() => getCurrentWindow().close(), 'close');
    });
    document.getElementById('minimize-window')?.addEventListener('click', () => {
        void runWindowAction(() => getCurrentWindow().minimize(), 'minimize');
    });

    const settingsButton = document.getElementById('settings-button');
    const settingsModal = document.getElementById('settings-modal');
    const closeSettingsButton = document.getElementById('close-settings');
    settingsButton?.addEventListener('click', () => { settingsModal.style.display = 'block'; });
    closeSettingsButton?.addEventListener('click', () => { settingsModal.style.display = 'none'; });
    window.addEventListener('click', (event) => {
        if (event.target === settingsModal) settingsModal.style.display = 'none';
    });
});
